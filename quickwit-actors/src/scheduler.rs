use core::fmt;
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Duration;
use std::time::Instant;
use async_trait::async_trait;

use crate::Actor;
use crate::ActorContext;
use crate::AsyncActor;

// Quickwit
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


pub struct Callback(Box<dyn FnOnce() + Send + Sync + 'static>); //Box<dyn Send + Sync + 'static>;

pub struct TimeoutEvent {
    deadline: Instant,
    event_id: u64, //< only useful to break ties in a deterministic way.
    callback: Callback
}

impl PartialEq for TimeoutEvent {
    fn eq(&self, other: &Self) -> bool {
        self.event_id == other.event_id
    }
}

impl Eq for TimeoutEvent {}

impl PartialOrd for TimeoutEvent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimeoutEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.deadline
            .cmp(&other.deadline)
            .then_with(|| self.event_id.cmp(&other.event_id))
    }
}

pub enum SchedulerMessage {
    ScheduleEvent {
        deadline: Duration,
        callback: Callback
    },
    Timeout,
}

impl fmt::Debug for SchedulerMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchedulerMessage::ScheduleEvent { deadline, callback } => {
                f.debug_struct("ScheduleEvent")
                 .field("deadline", deadline)
                 .finish()
            }
            SchedulerMessage::Timeout => {
                f.write_str("Timeout")
            },
        }
    }
}

#[derive(Default)]
pub struct Scheduler {
    event_id_generator: u64,
    simulated_time_shift: Duration,
    future_events: BinaryHeap<Reverse<TimeoutEvent>>,
    next_timeout: Option<Box<dyn Send + Sync>>
}

impl Actor for Scheduler {

    type Message = SchedulerMessage;

    type ObservableState = usize;

    fn observable_state(&self) -> Self::ObservableState {
        self.future_events.len()
    }
}

#[async_trait]
impl AsyncActor for Scheduler {
    async fn process_message(
        &mut self,
        message: SchedulerMessage,
        ctx: &crate::ActorContext<Self>,
    ) -> Result<(), crate::ActorTermination> {
        match message {
            SchedulerMessage::ScheduleEvent {
                deadline,
                callback
            } => {
                self.process_schedule_event(deadline, callback, ctx).await;
            },
            SchedulerMessage::Timeout => { self.process_timeout().await }
        }
        Ok(())
    }
}

impl Scheduler {
    async fn process_timeout(&self) {
        let now = self.simulated_now();
        while let Some(next_evt) = self.find_next_event_before_now(now) {
            next_evt.0()
        }
    }

    async fn process_schedule_event(&mut self, timeout: Duration, callback: Callback, ctx: &ActorContext<Self>) {
        let new_evt_deadline  = self.simulated_now() + timeout;
        let current_next_deadline = self.future_events.peek().map(|evt| evt.0.deadline);
        let is_new_next_deadline = current_next_deadline
            .map(|next_evt_deadline| new_evt_deadline > next_evt_deadline)
            .unwrap_or(true);
        let timeout_event = self.timeout_event(new_evt_deadline, callback);
        self.future_events.push(Reverse(timeout_event));
        if is_new_next_deadline {
            self.schedule_next_timeout(timeout, ctx);
        }
    }

    fn find_next_event_before_now(&self, simulated_now: Instant) -> Option<Callback> {
        let next_event_peek: &TimeoutEvent = self.future_events.peek().map(|rev| &rev.0)?;
        None
    }

    fn simulated_now(&self) -> Instant {
        Instant::now() + self.simulated_time_shift
    }


    fn timeout_event(&mut self, deadline: Instant, callback: Callback) -> TimeoutEvent {
        let event_id = self.event_id_generator;
        self.event_id_generator += 1;
        TimeoutEvent {
            deadline,
            event_id,
            callback,
        }
    }

    fn schedule_next_timeout(&mut self, timeout: Duration, ctx: &ActorContext<Self>) {
        let mailbox = ctx.mailbox().clone();
        let timeout = tokio::time::timeout(timeout,async move {
            mailbox.send_message(SchedulerMessage::Timeout).await;
        });
        // n.b.: Dropping the previous timeout cancels it.
        self.next_timeout = Some(Box::new(timeout));
    }

}


