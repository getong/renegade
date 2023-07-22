//! Implements the `Worker` trait for the handshake manager

use std::thread::{Builder, JoinHandle};

use common::types::{chain_id::ChainId, CancelChannel};
use common::worker::Worker;
use crossbeam::channel::Sender as CrossbeamSender;
use external_api::bus_message::SystemBusMessage;
use gossip_api::gossip::GossipOutbound;
use job_types::{
    handshake_manager::HandshakeExecutionJob, price_reporter::PriceReporterManagerJob,
    proof_manager::ProofManagerJob,
};
use starknet_client::client::StarknetClient;
use state::RelayerState;
use system_bus::SystemBus;
use task_driver::driver::TaskDriver;
use tokio::{
    runtime::Builder as RuntimeBuilder,
    sync::mpsc::{UnboundedReceiver, UnboundedSender as TokioSender},
};
use tracing::log;

use crate::manager::{
    init_price_streams, HandshakeExecutor, HandshakeScheduler, HANDSHAKE_EXECUTOR_N_THREADS,
};

use super::{error::HandshakeManagerError, manager::HandshakeManager};

/// The config type for the handshake manager
pub struct HandshakeManagerConfig {
    /// The chain that the local node targets
    pub chain_id: ChainId,
    /// The relayer-global state
    pub global_state: RelayerState,
    /// The channel on which to send outbound network requests
    pub network_channel: TokioSender<GossipOutbound>,
    /// The price reporter's job queue
    pub price_reporter_job_queue: TokioSender<PriceReporterManagerJob>,
    /// A starknet client for interacting with the contract
    pub starknet_client: StarknetClient,
    /// A sender on the handshake manager's job queue, used by the timer
    /// thread to enqueue outbound handshakes
    pub job_sender: TokioSender<HandshakeExecutionJob>,
    /// The job queue on which to receive handshake requests
    pub job_receiver: Option<UnboundedReceiver<HandshakeExecutionJob>>,
    /// A sender to forward jobs to the proof manager on
    pub proof_manager_sender: CrossbeamSender<ProofManagerJob>,
    /// The task driver, used to manage long-running async tasks
    pub task_driver: TaskDriver,
    /// The system bus to which all workers have access
    pub system_bus: SystemBus<SystemBusMessage>,
    /// The channel on which the coordinator may mandate that the
    /// handshake manager cancel its execution
    pub cancel_channel: CancelChannel,
}

impl Worker for HandshakeManager {
    type WorkerConfig = HandshakeManagerConfig;
    type Error = HandshakeManagerError;

    fn new(mut config: Self::WorkerConfig) -> Result<Self, Self::Error> {
        // Start a timer thread, periodically asks workers to begin handshakes with peers
        let scheduler = HandshakeScheduler::new(
            config.job_sender.clone(),
            config.global_state.clone(),
            config.cancel_channel.clone(),
        );
        let executor = HandshakeExecutor::new(
            config.chain_id,
            config.job_receiver.take().unwrap(),
            config.network_channel.clone(),
            config.price_reporter_job_queue.clone(),
            config.starknet_client.clone(),
            config.proof_manager_sender.clone(),
            config.global_state.clone(),
            config.task_driver.clone(),
            config.system_bus.clone(),
            config.cancel_channel.clone(),
        )?;

        Ok(HandshakeManager {
            config,
            executor: Some(executor),
            executor_handle: None,
            scheduler: Some(scheduler),
            scheduler_handle: None,
        })
    }

    fn is_recoverable(&self) -> bool {
        false
    }

    fn name(&self) -> String {
        "handshake-manager-main".to_string()
    }

    fn join(&mut self) -> Vec<JoinHandle<Self::Error>> {
        vec![
            self.executor_handle.take().unwrap(),
            self.scheduler_handle.take().unwrap(),
        ]
    }

    fn start(&mut self) -> Result<(), Self::Error> {
        log::info!("Starting executor loop for handshake protocol executor...");

        // Instruct the price reporter to being streaming prices for the default pairs
        init_price_streams(self.config.price_reporter_job_queue.clone())?;

        // Spawn both the executor and the scheduler in a thread
        let executor = self.executor.take().unwrap();
        let executor_handle = Builder::new()
            .name("handshake-executor-main".to_string())
            .spawn(move || {
                // Build a Tokio runtime for the handshake manager
                let runtime = RuntimeBuilder::new_multi_thread()
                    .enable_all()
                    .max_blocking_threads(HANDSHAKE_EXECUTOR_N_THREADS)
                    .build()
                    .unwrap();

                runtime.block_on(executor.execution_loop())
            })
            .map_err(|err| HandshakeManagerError::SetupError(err.to_string()))?;

        let scheduler = self.scheduler.take().unwrap();
        let scheduler_handle = Builder::new()
            .name("handshake-scheduler-main".to_string())
            .spawn(move || {
                let runtime = RuntimeBuilder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(scheduler.execution_loop())
            })
            .map_err(|err| HandshakeManagerError::SetupError(err.to_string()))?;

        self.executor_handle = Some(executor_handle);
        self.scheduler_handle = Some(scheduler_handle);

        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), Self::Error> {
        unimplemented!()
    }
}