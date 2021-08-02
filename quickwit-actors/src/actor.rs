use std::any::type_name;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

use crate::{Message, SendError};

#[derive(Error, Debug)]
pub enum MessageProcessError {
    #[error("On Demand")]
    OnDemand,
    #[error("Downstream actor closed connection")]
    DownstreamClosed,
    #[error("Failure")]
    Error(#[from] anyhow::Error),
}

impl From<SendError> for MessageProcessError {
    fn from(_: SendError) -> Self {
        MessageProcessError::DownstreamClosed
    }
}

/// An actor has an internal state and processes a stream of message.
///
/// While processing a message, the actor typically
/// - Update its state
/// - emit one or more message to other actors.
///
/// Actors exists in two flavor:
/// - async actors, are executed in event thread in tokio runtime.
/// - sync actors, executed on the blocking thread pool of tokio runtime.
pub trait Actor: Send + Sync + 'static {
    /// Type of message that can be received by the actor.
    type Message: Message;
    /// Piece of state that can be copied for assert in unit test, admin, etc.
    type ObservableState: Send + Clone + Sync + fmt::Debug;
    /// A name identifying the type of actor.
    /// It does not need to be "instance-unique", and can be the name of
    /// the actor implementation.
    fn name(&self) -> String {
        type_name::<Self>().to_string()
    }

    fn default_message(&self) -> Option<Self::Message> {
        None
    }

    /// Extracts an observable state. Useful for unit test, and admin UI.
    ///
    /// This function should return fast, but it is not called after receiving
    /// single message. Snapshotting happens when the actor is terminated, or
    /// in an on demand fashion by calling `ActorHandle::observe()`.
    fn observable_state(&self) -> Self::ObservableState;
}

/// Makes it possible to register some progress.
///
/// If no progress is observed until the next heartbeat, the actor will be killed.
#[derive(Clone)]
pub struct Progress(Arc<AtomicBool>);

impl Default for Progress {
    fn default() -> Progress {
        Progress(Arc::new(AtomicBool::new(false)))
    }
}

impl Progress {
    pub fn record_progress(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn has_changed(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct KillSwitch {
    alive: Arc<AtomicBool>,
}

impl Default for KillSwitch {
    fn default() -> Self {
        KillSwitch {
            alive: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl KillSwitch {
    pub fn kill(&self) {
        self.alive.store(false, Ordering::Relaxed);
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::KillSwitch;

    #[test]
    fn test_kill_switch() {
        let kill_switch = KillSwitch::default();
        assert_eq!(kill_switch.is_alive(), true);
        kill_switch.kill();
        assert_eq!(kill_switch.is_alive(), false);
        kill_switch.kill();
        assert_eq!(kill_switch.is_alive(), false);
    }
}
