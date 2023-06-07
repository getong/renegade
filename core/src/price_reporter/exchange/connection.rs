//! Defines abstract connection interfaces that can be streamed from

use async_trait::async_trait;
use core::panic;
use futures::{stream::StreamExt, SinkExt};
use futures_util::{
    stream::{SplitSink, SplitStream},
    Stream,
};
use ring_channel::{ring_channel, RingReceiver, RingSender};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt::{self, Display},
    num::NonZeroUsize,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpStream,
    time::{sleep, Duration},
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::log;
use url::Url;

use crate::price_reporter::{reporter::Price, worker::PriceReporterManagerConfig};

use super::{
    super::{
        errors::ExchangeConnectionError,
        exchange::handlers_centralized::{CentralizedExchangeHandler, OkxHandler},
        exchange::handlers_decentralized::UniswapV3Handler,
        reporter::PriceReport,
        tokens::Token,
    },
    Exchange,
};

/// Each sub-thread spawned by an ExchangeConnection must return a vector WorkerHandles: These are
/// used for error propagation back to the PriceReporter.
pub type WorkerHandles = Vec<tokio::task::JoinHandle<Result<(), ExchangeConnectionError>>>;

// -----------
// | Helpers |
// -----------

/// Build a websocket connection to the given endpoint
pub(super) async fn ws_connect(
    url: Url,
) -> Result<
    (
        SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
        SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    ),
    ExchangeConnectionError,
> {
    let ws_conn = match connect_async(url.clone()).await {
        Ok((conn, _resp)) => conn,
        Err(e) => {
            log::error!("Cannot connect to the remote URL: {}", url);
            return Err(ExchangeConnectionError::HandshakeFailure(e.to_string()));
        }
    };

    let (ws_sink, ws_stream) = ws_conn.split();
    Ok((ws_sink, ws_stream))
}

/// Helper to parse a value from a JSON response
pub(super) fn parse_json_field<T: FromStr>(
    field_name: &str,
    response: &Value,
) -> Result<T, ExchangeConnectionError> {
    match response[field_name].as_str() {
        None => Err(ExchangeConnectionError::InvalidMessage(
            response.to_string(),
        )),
        Some(best_bid_str) => best_bid_str
            .parse()
            .map_err(|_| ExchangeConnectionError::InvalidMessage(response.to_string())),
    }
}

/// Helper to parse a value from a JSON response by index
pub(super) fn parse_json_field_array<T: FromStr>(
    field_index: usize,
    response: &Value,
) -> Result<T, ExchangeConnectionError> {
    match response[field_index].as_str() {
        None => Err(ExchangeConnectionError::InvalidMessage(
            response.to_string(),
        )),
        Some(best_bid_str) => best_bid_str
            .parse()
            .map_err(|_| ExchangeConnectionError::InvalidMessage(response.to_string())),
    }
}

/// Parse an json structure from a websocket message
pub fn parse_json_from_message(message: Message) -> Result<Option<Value>, ExchangeConnectionError> {
    if let Message::Text(message_str) = message {
        // Okx sends some undocumented messages: Empty strings and "Protocol violation" messages.
        if message_str == "Protocol violation" || message_str.is_empty() {
            return Ok(None);
        }

        // Okx sends "pong" messages from our "ping" messages.
        if message_str == "pong" {
            return Ok(None);
        }

        // Okx and Kraken send "CloudFlare WebSocket proxy restarting" messages.
        if message_str == "CloudFlare WebSocket proxy restarting" {
            return Ok(None);
        }

        // Parse into a json blob
        serde_json::from_str(&message_str).map_err(|err| {
            ExchangeConnectionError::InvalidMessage(format!("{} for message: {}", err, message_str))
        })
    } else {
        Ok(None)
    }
}

/// Helper function to get the current UNIX epoch time in milliseconds.
pub fn get_current_time() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

// --------------------------
// | Connection Abstraction |
// --------------------------

/// The state of an ExchangeConnection. Note that the ExchangeConnection itself simply streams news
/// PriceReports, and the task of determining if the PriceReports have yet to arrive is the job of
/// the PriceReporter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ExchangeConnectionState {
    /// The ExchangeConnection is reporting as normal.
    Nominal(PriceReport),
    /// No data has yet to be reported from the ExchangeConnection.
    NoDataReported,
    /// This Exchange is unsupported for the given Token pair
    Unsupported,
}

impl Display for ExchangeConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fmt_str = match self {
            ExchangeConnectionState::Nominal(price_report) => {
                format!("{:.4}", price_report.midpoint_price)
            }
            ExchangeConnectionState::NoDataReported => String::from("NoDataReported"),
            ExchangeConnectionState::Unsupported => String::from("Unsupported"),
        };
        write!(f, "{}", fmt_str)
    }
}

/// A trait representing a connection to an exchange
#[async_trait]
pub trait ExchangeConnection: Stream<Item = Price> {
    /// Create a new connection to the exchange on a given asset pair
    async fn connect(
        base_token: Token,
        quote_token: Token,
        config: PriceReporterManagerConfig,
    ) -> Result<Self, ExchangeConnectionError>
    where
        Self: Sized;
    /// Send a keepalive signal on the connection if necessary
    async fn send_keepalive(&mut self) -> Result<(), ExchangeConnectionError> {
        Ok(())
    }
}

/// A connection to an `Exchange`. Note that creating an `ExchangeConnection` via
/// `ExchangeConnection::new(exchange: Exchange)` only returns a ring buffer channel receiver; the
/// ExchangeConnection is never directly accessed, and all data is reported only via this receiver.
#[derive(Clone, Debug)]
pub struct ExchangeConnectionOld {
    /// The CentralizedExchangeHandler for Okx.
    okx_handler: Option<OkxHandler>,
}

impl ExchangeConnectionOld {
    /// Create a new ExchangeConnection, returning the RingReceiver of PriceReports. Note that the
    /// role of the ExchangeConnection is to simply stream PriceReports as they come, and does not
    /// do any staleness testing or cross-Exchange deviation checks.
    pub async fn create_receiver(
        base_token: Token,
        quote_token: Token,
        exchange: Exchange,
        config: PriceReporterManagerConfig,
    ) -> Result<(RingReceiver<PriceReport>, WorkerHandles), ExchangeConnectionError> {
        // Create the vector of JoinHandles for all spawned threads.
        let mut worker_handles: WorkerHandles = vec![];

        // Create the ring buffer.
        let (mut price_report_sender, price_report_receiver) =
            ring_channel::<PriceReport>(NonZeroUsize::new(1).unwrap());

        // UniswapV3 logic is slightly different, as we use the web3 API wrapper for convenience,
        // rather than interacting directly over websockets.
        if exchange == Exchange::UniswapV3 {
            let worker_handles = UniswapV3Handler::start_price_stream(
                base_token,
                quote_token,
                price_report_sender,
                config,
            )
            .await?;
            return Ok((price_report_receiver, worker_handles));
        }

        // Get initial ExchangeHandler state and include in a new ExchangeConnection.
        let mut exchange_connection = match exchange {
            Exchange::Binance => ExchangeConnectionOld { okx_handler: None },
            Exchange::Coinbase => ExchangeConnectionOld { okx_handler: None },
            Exchange::Kraken => ExchangeConnectionOld { okx_handler: None },
            Exchange::Okx => ExchangeConnectionOld {
                okx_handler: Some(OkxHandler::new(base_token, quote_token, config)),
            },
            _ => unreachable!(),
        };

        // Retrieve the optional pre-stream PriceReport.
        let pre_stream_price_report = match exchange {
            Exchange::Binance => panic!(""),
            Exchange::Coinbase => panic!(""),
            Exchange::Kraken => panic!(""),
            Exchange::Okx => exchange_connection
                .okx_handler
                .as_mut()
                .unwrap()
                .pre_stream_price_report(),
            _ => unreachable!(),
        }
        .await;
        if let Some(pre_stream_price_report) = pre_stream_price_report? {
            let mut price_report_sender_clone = price_report_sender.clone();
            tokio::spawn(async move {
                // TODO: Sleeping is a somewhat hacky way of ensuring that the
                // pre_stream_price_report is received.
                sleep(Duration::from_secs(5)).await;
                price_report_sender_clone
                    .send(pre_stream_price_report)
                    .map_err(|err| ExchangeConnectionError::ConnectionHangup(err.to_string()))?;
                Ok::<(), ExchangeConnectionError>(())
            });
        }

        // Retrieve the websocket URL and connect to it.
        let wss_url = match exchange {
            Exchange::Binance => panic!(""),
            Exchange::Coinbase => panic!(""),
            Exchange::Kraken => panic!(""),
            Exchange::Okx => exchange_connection
                .okx_handler
                .as_ref()
                .unwrap()
                .websocket_url(),
            _ => unreachable!(),
        };
        let url = Url::parse(&wss_url).unwrap();
        let (mut socket, _response) = {
            let connection = connect_async(url).await;
            if let Ok(connection) = connection {
                connection
            } else {
                if exchange == Exchange::Binance {
                    println!(
                        "You are likely attempting to connect from an IP address \
                        blacklisted by Binance (e.g., anything US-based)"
                    );
                }
                println!("Cannot connect to the remote URL: {}", wss_url);
                return Err(ExchangeConnectionError::HandshakeFailure(
                    connection.unwrap_err().to_string(),
                ));
            }
        };

        // Send initial subscription message(s).
        match exchange {
            Exchange::Binance => panic!(""),
            Exchange::Coinbase => panic!(""),
            Exchange::Kraken => panic!(""),
            Exchange::Okx => exchange_connection
                .okx_handler
                .as_ref()
                .unwrap()
                .websocket_subscribe(&mut socket),
            _ => unreachable!(),
        }
        .await?;

        // Start listening for inbound messages.
        let (mut socket_sink, mut socket_stream) = socket.split();
        let worker_handle = tokio::spawn(async move {
            loop {
                let message =
                    socket_stream.next().await.unwrap().map_err(|err| {
                        ExchangeConnectionError::ConnectionHangup(err.to_string())
                    })?;
                exchange_connection.handle_exchange_message(&mut price_report_sender, message)?;
            }
        });
        worker_handles.push(worker_handle);

        // Periodically send a ping to prevent websocket hangup
        let worker_handle = tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(15)).await;
                if exchange == Exchange::Okx {
                    socket_sink
                        .send(Message::Text("ping".to_string()))
                        .await
                        .unwrap();
                } else {
                    socket_sink.send(Message::Ping(vec![])).await.unwrap();
                }
            }
        });
        worker_handles.push(worker_handle);

        Ok((price_report_receiver, worker_handles))
    }

    /// Simple wrapper around each individual ExchangeConnection handle_exchange_message.
    fn handle_exchange_message(
        &mut self,
        price_report_sender: &mut RingSender<PriceReport>,
        message: Message,
    ) -> Result<(), ExchangeConnectionError> {
        let message_str = message.into_text().unwrap();
        // Okx sends some undocumented messages: Empty strings and "Protocol violation" messages.
        if message_str == "Protocol violation" || message_str.is_empty() {
            return Ok(());
        }
        // Okx sends "pong" messages from our "ping" messages.
        if message_str == "pong" {
            return Ok(());
        }
        // Okx and Kraken send "CloudFlare WebSocket proxy restarting" messages.
        if message_str == "CloudFlare WebSocket proxy restarting" {
            return Ok(());
        }
        let message_json = serde_json::from_str(&message_str).map_err(|err| {
            ExchangeConnectionError::InvalidMessage(format!("{} for message: {}", err, message_str))
        })?;

        let price_report = {
            if let Some(okx_handler) = &mut self.okx_handler {
                okx_handler.handle_exchange_message(message_json)
            } else {
                unreachable!();
            }
        }?;

        if let Some(mut price_report) = price_report {
            price_report.local_timestamp = get_current_time();
            price_report_sender.send(price_report).unwrap();
        }

        Ok(())
    }
}