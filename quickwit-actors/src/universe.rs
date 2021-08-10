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

use crate::ActorHandle;
use crate::Actor;
use crate::Mailbox;
use crate::KillSwitch;
use crate::Scheduler;
use crate::AsyncActor;

pub struct Universe {
    scheduler_handle: Option<ActorHandle<Scheduler>>,
    scheduler_mailbox: Mailbox<<Scheduler as Actor>::Message>,
    kill_switch: KillSwitch,
}

impl Drop for Universe {
    fn drop(&mut self) {
        self.kill_switch.kill();
    }
}

impl Universe {
    /// Creates a new universe.
    pub async fn new() -> Universe {
        let scheduler = Scheduler::default();
        let kill_switch = KillSwitch::default();
        let (scheduler_mailbox, scheduler_handler) = scheduler.spawn(kill_switch.clone());
        Universe {
            scheduler_handle: Some(scheduler_handler),
            scheduler_mailbox,
            kill_switch,
        }
    }

    pub fn spawn<A: AsyncActor>(&self, actor: A) -> (Mailbox<A::Message>, ActorHandle<A>) {
        actor.spawn(self.kill_switch.clone(),) // self.scheduler_mailbox.clone()
    }

    /// `async` version of `send_message`
    pub async fn send_message<M>(
        &self,
        mailbox: &Mailbox<M>,
        msg: M,
    ) -> Result<(), crate::SendError> {
        mailbox.send_message(msg).await
    }
}
