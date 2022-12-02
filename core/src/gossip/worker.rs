//! Implements the `Worker` trait for the GossipServer

use std::thread::JoinHandle;

use crossbeam::channel::{Receiver, Sender};
use ed25519_dalek::Keypair;
use tokio::sync::mpsc::UnboundedSender as TokioSender;

use crate::{
    api::gossip::GossipOutbound, state::GlobalRelayerState, worker::Worker, CancelChannel,
};

use super::{
    errors::GossipError,
    heartbeat_executor::HeartbeatProtocolExecutor,
    jobs::HeartbeatExecutorJob,
    server::GossipServer,
    types::{ClusterId, WrappedPeerId},
};

/// The configuration passed from the coordinator to the GossipServer
#[derive(Debug)]
pub struct GossipServerConfig {
    /// The libp2p PeerId of the local peer
    pub(crate) local_peer_id: WrappedPeerId,
    /// The cluster ID of the local peer
    pub(crate) cluster_id: ClusterId,
    /// The keypair of the local peer's cluster
    pub(crate) cluster_keypair: Keypair,
    /// A reference to the relayer-global state
    pub(crate) global_state: GlobalRelayerState,
    /// A job queue to send outbound heartbeat requests on
    pub(crate) heartbeat_worker_sender: Sender<HeartbeatExecutorJob>,
    /// A job queue to receive inbound heartbeat requests on
    pub(crate) heartbeat_worker_receiver: Receiver<HeartbeatExecutorJob>,
    /// A job queue to send outbound network requests on
    pub(crate) network_sender: TokioSender<GossipOutbound>,
    /// The channel on which the coordinator may mandate that the
    /// gossip server cancel its execution
    pub(crate) cancel_channel: CancelChannel,
}

impl Worker for GossipServer {
    type WorkerConfig = GossipServerConfig;
    type Error = GossipError;

    fn new(config: Self::WorkerConfig) -> Result<Self, Self::Error> {
        // Register self as replicator of owned wallets using peer info from network manager
        {
            let global_copy = config.global_state.clone();
            let mut locked_global_state = global_copy.write().expect("global state lock poisoned");

            for (_, wallet) in locked_global_state.managed_wallets.iter_mut() {
                wallet.metadata.replicas.push(config.local_peer_id);
            }
        } // locked_global_state released

        Ok(Self {
            config,
            heartbeat_executor: None,
        })
    }

    fn is_recoverable(&self) -> bool {
        true
    }

    fn join(&mut self) -> Vec<JoinHandle<Self::Error>> {
        self.heartbeat_executor.take().unwrap().join()
    }

    fn start(&mut self) -> Result<(), Self::Error> {
        // Start the heartbeat executor, this worker manages pinging peers and responding to
        // heartbeat requests from peers
        let heartbeat_executor = HeartbeatProtocolExecutor::new(
            self.config.local_peer_id,
            self.config.network_sender.clone(),
            self.config.heartbeat_worker_sender.clone(),
            self.config.heartbeat_worker_receiver.clone(),
            self.config.global_state.clone(),
            self.config.cancel_channel.clone(),
        )?;
        self.heartbeat_executor = Some(heartbeat_executor);

        // Wait for the local peer to handshake with known other peers
        // before sending a cluster membership message
        self.warmup_then_join_cluster();

        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), Self::Error> {
        unimplemented!()
    }
}