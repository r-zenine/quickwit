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

use core::fmt;
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Duration;
use std::time::Instant;
use async_trait::async_trait;
use tokio::task::JoinHandle;
use tracing::info;

use crate::Actor;
use crate::ActorContext;
use crate::AsyncActor;

pub struct Callback(Box<dyn FnOnce() + Send + Sync + 'static>); //Box<dyn Send + Sync + 'static>;

struct TimeoutEvent {
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
        timeout: Duration,
        callback: Callback
    },
    Timeout,
    SimulateAdvanceTime {
        time_shift: Duration
    }
}

impl fmt::Debug for SchedulerMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchedulerMessage::ScheduleEvent { timeout, callback: _ } => {
                f.debug_struct("ScheduleEvent")
                 .field("timeout", timeout)
                 .finish()
            }
            SchedulerMessage::Timeout => {
                f.write_str("Timeout")
            },
            SchedulerMessage::SimulateAdvanceTime { time_shift } => {
                f.debug_struct("SimulateAdvanceTime")
                 .field("time_shift", time_shift)
                 .finish()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerCounters {
    num_pending_events: usize,
    total_num_events: u64,
}

#[derive(Default)]
pub struct Scheduler {
    event_id_generator: u64,
    simulated_time_shift: Duration,
    future_events: BinaryHeap<Reverse<TimeoutEvent>>,
    next_timeout: Option<JoinHandle<()>>,
}

impl Actor for Scheduler {

    type Message = SchedulerMessage;

    type ObservableState = SchedulerCounters;

    fn observable_state(&self) -> Self::ObservableState {
        SchedulerCounters {
            num_pending_events: self.future_events.len(),
            total_num_events: self.event_id_generator,
        }
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
                timeout,
                callback
            } => {
                self.process_schedule_event(timeout, callback, ctx).await;
            },
            SchedulerMessage::Timeout => {
                self.process_timeout(ctx).await
            }
            SchedulerMessage::SimulateAdvanceTime { time_shift } => { self.process_simulate_advance_time(time_shift, ctx).await }
        }
        Ok(())
    }
}

impl Scheduler {
    async fn process_timeout(&mut self, ctx: &ActorContext<Self>) {
        let now = self.simulated_now();
        while let Some(next_evt) = self.find_next_event_before_now(now) {
            next_evt.0()
        }
        self.schedule_next_timeout(ctx);
    }

    async fn process_schedule_event(&mut self, timeout: Duration, callback: Callback, ctx: &ActorContext<Self>) {
        let new_evt_deadline  = self.simulated_now() + timeout;
        let current_next_deadline = self.future_events.peek().map(|evt| evt.0.deadline);
        let is_new_next_deadline = current_next_deadline
            .map(|next_evt_deadline| new_evt_deadline < next_evt_deadline)
            .unwrap_or(true);
        let timeout_event = self.timeout_event(new_evt_deadline, callback);
        self.future_events.push(Reverse(timeout_event));
        if is_new_next_deadline {
            self.schedule_next_timeout(ctx);
        }
    }

    async fn process_simulate_advance_time(&mut self, time_shift: Duration, ctx: &ActorContext<Self>) {
        info!(time_shift=?time_shift, "advance-time");
        self.simulated_time_shift += time_shift;
        self.process_timeout(ctx).await;
    }

    fn find_next_event_before_now(&mut self, simulated_now: Instant) -> Option<Callback> {
        let next_event_deadline: Instant = self.future_events.peek().map(|rev| rev.0.deadline)?;
        if next_event_deadline < simulated_now {
            self.future_events.pop().map(|rev| rev.0.callback)
        } else {
            None
        }
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

    fn schedule_next_timeout(&mut self, ctx: &ActorContext<Self>) {
        let simulated_now = self.simulated_now();
        let next_deadline_opt = self.future_events.peek().map(|evt| evt.0.deadline);
        let timeout = if let Some(next_deadline) = next_deadline_opt {
            next_deadline - simulated_now
        } else {
            return;
        };
        let mailbox = ctx.mailbox().clone();
        let timeout = async move {
            tokio::time::sleep(timeout).await;
            let _ = mailbox.send_message(SchedulerMessage::Timeout).await;
        };
        let new_join_handle: JoinHandle<()> = tokio::task::spawn(timeout);
        if let Some(previous_join_handle) = self.next_timeout.take() {
            previous_join_handle.abort();
        }
        // n.b.: Dropping the previous timeout cancels it.
        self.next_timeout = Some(new_join_handle);
    }

}


#[cfg(test)]
mod tests {
    use super::{Scheduler, SchedulerMessage, Callback};
    use std::sync::Arc;
    use std::sync::atomic::{Ordering, AtomicBool};
    use crate::Universe;
    use crate::scheduler::SchedulerCounters;
    use crate::AsyncActor;
    use std::time::Duration;

    fn create_test_callback() -> (Arc<AtomicBool>, Callback) {
        let cb_called = Arc::new(AtomicBool::default());
        let cb_called_clone = cb_called.clone();
        let callback = Callback(Box::new(move || {
            cb_called_clone.store(true, Ordering::SeqCst);
        }));
        (cb_called, callback)
    }

    #[tokio::test]
    async fn test_scheduler_advance_time() {
        quickwit_common::setup_logging_for_tests();
        let universe = Universe::new().await;
        // It might be a bit confusing. We spawn a scheduler like a regular actor to test it.
        // The scheduler is usually spawned from within the universe.
        let (scheduler_mailbox, scheduler_handler) = universe.spawn(Scheduler::default());
        let (cb_called, callback) = create_test_callback();
        universe.send_message(&scheduler_mailbox, SchedulerMessage::ScheduleEvent { timeout: Duration::from_secs(30), callback  }).await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!cb_called.load(Ordering::SeqCst));
        universe.send_message(&scheduler_mailbox, SchedulerMessage::SimulateAdvanceTime { time_shift: Duration::from_secs(31) }).await.unwrap();
        let scheduler_counters: SchedulerCounters= scheduler_handler.process_pending_and_observe().await.into_inner();
        assert_eq!(scheduler_counters, SchedulerCounters { total_num_events: 1, num_pending_events: 0});
        assert!(cb_called.load(Ordering::SeqCst));
        scheduler_handler.finish().await;
    }

    #[tokio::test]
    async fn test_scheduler_simple() {
        quickwit_common::setup_logging_for_tests();
        let universe = Universe::new().await;
        let (scheduler_mailbox, scheduler_handler) = universe.spawn(Scheduler::default());
        let (cb_called1, callback1) = create_test_callback();
        let (cb_called2, callback2) = create_test_callback();
        universe.send_message(&scheduler_mailbox, SchedulerMessage::ScheduleEvent { timeout: Duration::from_secs(20), callback: callback2  }).await.unwrap();
        universe.send_message(&scheduler_mailbox, SchedulerMessage::ScheduleEvent { timeout: Duration::from_millis(2), callback: callback1  }).await.unwrap();
        let scheduler_counters = scheduler_handler.process_pending_and_observe().await.into_inner();
        assert_eq!(scheduler_counters, SchedulerCounters { total_num_events: 2, num_pending_events: 2});
        assert!(!cb_called1.load(Ordering::SeqCst));
        assert!(!cb_called2.load(Ordering::SeqCst));
        tokio::time::sleep(Duration::from_millis(10)).await;
        let scheduler_counters = scheduler_handler.process_pending_and_observe().await.into_inner();
        assert_eq!(scheduler_counters, SchedulerCounters { total_num_events: 2, num_pending_events: 1});
        assert!(cb_called1.load(Ordering::SeqCst));
        assert!(!cb_called2.load(Ordering::SeqCst));
        universe.send_message(&scheduler_mailbox, SchedulerMessage::SimulateAdvanceTime { time_shift: Duration::from_secs(10) }).await.unwrap();
        assert!(cb_called1.load(Ordering::SeqCst));
        assert!(!cb_called2.load(Ordering::SeqCst));
        universe.send_message(&scheduler_mailbox, SchedulerMessage::SimulateAdvanceTime { time_shift: Duration::from_secs(10) }).await.unwrap();
        let scheduler_counters: SchedulerCounters= scheduler_handler.process_pending_and_observe().await.into_inner();
        assert!(cb_called1.load(Ordering::SeqCst));
        assert!(cb_called2.load(Ordering::SeqCst));
        assert_eq!(scheduler_counters, SchedulerCounters { total_num_events: 2, num_pending_events: 0});
        scheduler_handler.finish().await;
    }
}
