//  Quickwit
//  Copyright (C) 2021 Quickwit Inc.
//
//  Quickwit is offered under the AGPL v3.0 and as commercial software.
//  For commercial licensing, contact us at hello@quickwit.io.
//
//  AGPL:
//  This program is free software: you can redistribute it and/or modify
//  it under the terms of the GNU Affero General Public License as
//  published by the Free Software Foundation, either version 3 of the
//  License, or (at your option) any later version.
//
//  This program is distributed in the hope that it will be useful,
//  but WITHOUT ANY WARRANTY; without even the implied warranty of
//  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//  GNU Affero General Public License for more details.
//
//  You should have received a copy of the GNU Affero General Public License
//  along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use flume::RecvTimeoutError;
use std::hash::Hash;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::actor_handle::ActorMessage;

#[derive(Clone)]
pub struct Mailbox<M: Message> {
    inner: Arc<Inner<M>>,
}

impl<M: Message> Mailbox<M> {
    pub(crate) fn is_last_mailbox(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }
}

impl<M: Message> Deref for Mailbox<M> {
    type Target = Inner<M>;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref()
    }
}

pub struct Inner<M: Message> {
    sender: flume::Sender<ActorMessage<M>>,
    command_sender: flume::Sender<Command>,
    id: Uuid,
    actor_name: String,
}

pub enum Command {
    Pause,
    Stop(oneshot::Sender<()>),
    Start,
    Observe(oneshot::Sender<()>),
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::Pause => write!(f, "Pause"),
            Command::Stop(_) => write!(f, "Stop"),
            Command::Start => write!(f, "Start"),
            Command::Observe(_) => write!(f, "Observe"),
        }
    }
}

impl<M: Message> fmt::Debug for Mailbox<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Mailbox({})", self.actor_name())
    }
}

impl<M: Message> Hash for Mailbox<M> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state)
    }
}

impl<M: Message> PartialEq for Mailbox<M> {
    fn eq(&self, other: &Self) -> bool {
        self.id.eq(&other.id)
    }
}

pub struct SendError;

impl<M: Message> Eq for Mailbox<M> {}

impl<M: Message> Mailbox<M> {
    pub fn actor_name(&self) -> String {
        format!("{}:{}", self.actor_name, self.id)
    }

    pub(crate) async fn send_actor_message(&self, msg: ActorMessage<M>) -> Result<(), SendError> {
        self.sender.send_async(msg).await.map_err(|_| SendError)
    }

    /// Send a message to the actor synchronously.
    ///
    /// SendError is returned if the user is already terminated.
    ///
    /// (See also [Self::send_blocking()])
    pub async fn send_async(&self, msg: M) -> Result<(), SendError> {
        self.send_actor_message(ActorMessage::Message(msg)).await
    }

    /// Send a message to the actor in a blocking fashion.
    /// When possible, prefer using [Self::send_async()].
    ///
    // TODO do we need a version with a deadline?
    pub fn send_blocking(&self, msg: M) -> Result<(), SendError> {
        self.sender
            .send(ActorMessage::Message(msg))
            .map_err(|_e| SendError)
    }

    pub fn send_command_blocking(&self, command: Command) -> Result<(), SendError> {
        self.command_sender.send(command).map_err(|_e| SendError)
    }

    pub async fn send_command(&self, command: Command) -> Result<(), SendError> {
        self.command_sender
            .send_async(command)
            .await
            .map_err(|_e| SendError)
    }
}

pub struct Inbox<M: Message> {
    rx: flume::Receiver<ActorMessage<M>>,
    command_rx: flume::Receiver<Command>,
}

#[derive(Debug)]
pub enum ReceptionResult<M> {
    Command(Command),
    Message(M),
    None,
    Disconnect,
}

impl<M: Message> Inbox<M> {
    fn get_command_if_available(&self) -> Option<ReceptionResult<M>> {
        match self.command_rx.try_recv() {
            Ok(command) => Some(ReceptionResult::Command(command)),
            Err(flume::TryRecvError::Disconnected) => Some(ReceptionResult::Disconnect),
            Err(flume::TryRecvError::Empty) => None,
        }
    }

    pub async fn try_recv_msg_async(&self, message_enabled: bool) -> ReceptionResult<M> {
        if let Some(command) = self.get_command_if_available() {
            tokio::task::yield_now().await;
            return command;
        }
        if !message_enabled {
            tokio::task::yield_now().await;
            return ReceptionResult::None;
        }
        match tokio::time::timeout(crate::message_timeout(), self.rx.recv_async()).await {
            Ok(Ok(ActorMessage::Message(msg))) => ReceptionResult::Message(msg),
            Ok(Ok(ActorMessage::Observe(cb))) => ReceptionResult::Command(Command::Observe(cb)),
            Ok(Err(_recv_error)) => ReceptionResult::Disconnect,
            Err(_timeout_error) => ReceptionResult::None,
        }
    }

    pub fn try_recv_msg(&self, message_enabled: bool) -> ReceptionResult<M> {
        if let Some(command) = self.get_command_if_available() {
            return command;
        }
        if !message_enabled {
            return ReceptionResult::None;
        }
        let msg = self.rx.recv_timeout(crate::message_timeout());
        match msg {
            Ok(ActorMessage::Message(msg)) => ReceptionResult::Message(msg),
            Ok(ActorMessage::Observe(cb)) => ReceptionResult::Command(Command::Observe(cb)),
            Err(RecvTimeoutError::Disconnected) => ReceptionResult::Disconnect,
            Err(RecvTimeoutError::Timeout) => ReceptionResult::None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Capacity {
    Bounded(usize),
    Unbounded,
}

pub trait Message: Send + Sync + Clone + fmt::Debug {}

impl Capacity {
    fn create_channel<M>(&self) -> (flume::Sender<M>, flume::Receiver<M>) {
        match *self {
            Capacity::Bounded(cap) => flume::bounded(cap),
            Capacity::Unbounded => flume::unbounded(),
        }
    }
}

pub fn create_mailbox<M: Message>(
    actor_name: String,
    capacity: Capacity,
) -> (Mailbox<M>, Inbox<M>) {
    let (msg_tx, msg_rx) = capacity.create_channel();
    let (cmd_tx, cmd_rx) = Capacity::Unbounded.create_channel();
    let mailbox = Mailbox {
        inner: Arc::new(Inner {
            sender: msg_tx,
            command_sender: cmd_tx,
            id: Uuid::new_v4(),
            actor_name,
        }),
    };
    let inbox = Inbox {
        rx: msg_rx,
        command_rx: cmd_rx,
    };
    (mailbox, inbox)
}
