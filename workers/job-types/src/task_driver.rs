//! Job types for the task driver

use common::types::tasks::{QueuedTask, TaskDescriptor, TaskIdentifier};
use crossbeam::channel::Sender as CrossbeamSender;
use tokio::sync::oneshot::{
    channel as oneshot_channel, Receiver as OneshotReceiver, Sender as OneshotSender,
};
use util::metered_channels::MeteredCrossbeamReceiver;

/// The name of the task driver queue, used to label queue length metrics
const TASK_DRIVER_QUEUE_NAME: &str = "task_driver";

/// The queue sender type to send jobs to the task driver
pub type TaskDriverQueue = CrossbeamSender<TaskDriverJob>;
/// The queue receiver type to receive jobs for the task driver
pub type TaskDriverReceiver = MeteredCrossbeamReceiver<TaskDriverJob>;
/// The sender type of a task notification channel
pub type TaskNotificationSender = OneshotSender<Result<(), String>>;
/// The receiver type of a task notification channel
pub type TaskNotificationReceiver = OneshotReceiver<Result<(), String>>;

/// Create a new task driver queue
pub fn new_task_driver_queue() -> (TaskDriverQueue, TaskDriverReceiver) {
    let (send, recv) = crossbeam::channel::unbounded();
    (send, MeteredCrossbeamReceiver::new(recv, TASK_DRIVER_QUEUE_NAME))
}

/// Create a new notification channel and job for the task driver
pub fn new_task_notification(task_id: TaskIdentifier) -> (TaskNotificationReceiver, TaskDriverJob) {
    let (sender, receiver) = oneshot_channel();
    (receiver, TaskDriverJob::Notify { task_id, channel: sender })
}

/// The job type for the task driver
#[derive(Debug)]
pub enum TaskDriverJob {
    /// Run a task
    Run(QueuedTask),
    /// Run a task immediately, bypassing the task queue
    ///
    /// This is used for tasks which need immediate settlement, e.g. matches
    ///
    /// Other tasks on a shared wallet will be preempted and the queue paused
    RunImmediate {
        /// The ID to assign the task
        task_id: TaskIdentifier,
        /// The task to run
        task: TaskDescriptor,
        /// The response channel on which to send the task result
        resp: Option<TaskNotificationSender>,
    },
    /// Request that the task driver notify a worker when a task is complete
    Notify {
        /// The task id to notify the worker about
        task_id: TaskIdentifier,
        /// The channel on which to notify the worker
        channel: TaskNotificationSender,
    },
}

impl TaskDriverJob {
    /// Create a new immediate task without a notification channel
    pub fn new_immediate(task: TaskDescriptor) -> Self {
        let id = TaskIdentifier::new_v4();
        Self::RunImmediate { task_id: id, task, resp: None }
    }

    /// Create a new immediate task with a notification channel
    pub fn new_immediate_with_notification(
        task: TaskDescriptor,
    ) -> (Self, TaskNotificationReceiver) {
        let id = TaskIdentifier::new_v4();
        let (sender, receiver) = oneshot_channel();
        (Self::RunImmediate { task_id: id, task, resp: Some(sender) }, receiver)
    }
}
