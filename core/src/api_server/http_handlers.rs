//! Groups handlers for the HTTP API

use crossbeam::channel::{self, Sender};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot::channel as oneshot_channel;

use crate::{
    api::http::{
        CreateWalletRequest, GetExchangeHealthStatesRequest, GetExchangeHealthStatesResponse,
        GetReplicasRequest, GetReplicasResponse, PingRequest, PingResponse,
    },
    price_reporter::jobs::PriceReporterManagerJob,
    proof_generation::jobs::{ProofJob, ProofManagerJob},
    state::RelayerState,
};

use super::{error::ApiServerError, routes::TypedHandler, worker::ApiServerConfig};

// ----------------
// | Generic APIs |
// ----------------

/// Handler for the ping route, returns a pong
#[derive(Clone, Debug)]
pub struct PingHandler;
impl PingHandler {
    /// Create a new handler for "/ping"
    pub fn new() -> Self {
        Self {}
    }
}

impl TypedHandler for PingHandler {
    type Request = PingRequest;
    type Response = PingResponse;
    type Error = ApiServerError;

    fn handle_typed(&self, _req: Self::Request) -> Result<Self::Response, Self::Error> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        Ok(PingResponse { timestamp })
    }
}

// --------------------------
// | Wallet Operations APIs |
// --------------------------

/// Handler for the /wallet/create route
#[derive(Debug)]
pub struct WalletCreateHandler {
    /// The channel to enqueue a proof generation request of `VALID WALLET CREATE` on
    proof_job_queue: Sender<ProofManagerJob>,
}

impl WalletCreateHandler {
    /// Create a new handler for the /wallet/create route
    pub fn new(proof_manager_job_queue: Sender<ProofManagerJob>) -> Self {
        Self {
            proof_job_queue: proof_manager_job_queue,
        }
    }
}

impl TypedHandler for WalletCreateHandler {
    type Request = CreateWalletRequest;
    type Response = (); // TODO: Define a response type
    type Error = ApiServerError;

    fn handle_typed(&self, req: Self::Request) -> Result<Self::Response, Self::Error> {
        let (response_sender, _) = oneshot_channel();
        self.proof_job_queue
            .send(ProofManagerJob {
                type_: ProofJob::ValidWalletCreate(),
                response_channel: response_sender,
            })
            .map_err(|err| ApiServerError::EnqueueJob(err.to_string()))
    }
}

// ------------------------
// | Price Reporting APIs |
// ------------------------

/// Handler for the / route, returns the health report for each individual
/// exchange and the aggregate median
#[derive(Clone, Debug)]
pub(crate) struct ExchangeHealthStatesHandler {
    /// The config for the API server
    config: ApiServerConfig,
}

impl ExchangeHealthStatesHandler {
    /// Create a new handler for "/exchange/health"
    pub fn new(config: ApiServerConfig) -> Self {
        Self { config }
    }
}

impl TypedHandler for ExchangeHealthStatesHandler {
    type Request = GetExchangeHealthStatesRequest;
    type Response = GetExchangeHealthStatesResponse;
    type Error = ApiServerError;

    fn handle_typed(&self, req: Self::Request) -> Result<Self::Response, Self::Error> {
        let (price_reporter_state_sender, price_reporter_state_receiver) = channel::unbounded();
        self.config
            .price_reporter_work_queue
            .send(PriceReporterManagerJob::PeekMedian {
                base_token: req.base_token.clone(),
                quote_token: req.quote_token.clone(),
                channel: price_reporter_state_sender,
            })
            .unwrap();
        let (exchange_connection_state_sender, exchange_connection_state_receiver) =
            channel::unbounded();
        self.config
            .price_reporter_work_queue
            .send(PriceReporterManagerJob::PeekAllExchanges {
                base_token: req.base_token,
                quote_token: req.quote_token,
                channel: exchange_connection_state_sender,
            })
            .unwrap();
        Ok(GetExchangeHealthStatesResponse {
            median: price_reporter_state_receiver.recv().unwrap(),
            all_exchanges: exchange_connection_state_receiver.recv().unwrap(),
        })
    }
}

// ---------------------
// | Cluster Info APIs |
// ---------------------

/// Handler for the replicas route, returns the number of replicas a given wallet has
#[derive(Clone, Debug)]
pub struct ReplicasHandler {
    /// The global state of the relayer, used to query information for requests
    global_state: RelayerState,
}

impl ReplicasHandler {
    /// Create a new handler for "/replicas"
    pub fn new(global_state: RelayerState) -> Self {
        Self { global_state }
    }
}

impl TypedHandler for ReplicasHandler {
    type Request = GetReplicasRequest;
    type Response = GetReplicasResponse;
    type Error = ApiServerError;

    fn handle_typed(&self, req: Self::Request) -> Result<Self::Response, Self::Error> {
        let replicas = if let Some(wallet_info) =
            self.global_state.read_managed_wallets().get(&req.wallet_id)
        {
            wallet_info.metadata.replicas.clone().into_iter().collect()
        } else {
            vec![]
        };

        Ok(GetReplicasResponse { replicas })
    }
}