use tokio::sync::watch::{self, Sender};
use tokio::task::spawn_blocking;
use tracing::{debug, error, info};

use crate::actor::ActorTermination;
use crate::actor_state::ActorState;
use crate::mailbox::{create_mailbox, Command, Inbox};
use crate::{Actor, ActorContext, ActorHandle, KillSwitch, ReceptionResult};

/// An sync actor is executed on a tokio blocking task.
///
/// It may block and perform CPU heavy computation.
/// (See also [`AsyncActor`])
pub trait SyncActor: Actor + Sized {
    /// Processes a message.
    ///
    /// If true is returned, the actors will continue processing messages.
    /// If false is returned, the actor will terminate "gracefully".
    ///
    /// If an error is returned, the actor will be killed, as well as all of the actor
    /// under the same kill switch.
    fn process_message(
        &mut self,
        message: Self::Message,
        ctx: &ActorContext<Self>,
    ) -> Result<(), ActorTermination>;

    /// Hook  that can be set up to define what should happen upon actor termination.
    /// This hook is called only once.
    ///
    /// It is always called regardless of the type of termination.
    /// termination is passed as an argument to make it possible to act conditionnally
    /// to the type of Termination.
    fn finalize(&mut self, _termination: &ActorTermination, _ctx: &ActorContext<Self>) -> anyhow::Result<()> {
        Ok(())
    }

    #[doc(hidden)]
    fn spawn(self, kill_switch: KillSwitch) -> ActorHandle<Self> {
        let actor_name = self.name();
        debug!(actor_name=%actor_name,"spawning-sync-actor");
        let queue_capacity = self.queue_capacity();
        let (mailbox, inbox) = create_mailbox(actor_name, queue_capacity);
        let (state_tx, state_rx) = watch::channel(self.observable_state());
        let ctx = ActorContext::new(mailbox, kill_switch);
        let ctx_clone = ctx.clone();
        let join_handle = spawn_blocking::<_, ActorTermination>(move || {
            let actor_name = self.name();
            let termination = sync_actor_loop(self, inbox, ctx, state_tx);
            info!(cause=?termination, actor=%actor_name, "termination");
            termination
        });
        ActorHandle::new(
            state_rx,
            join_handle,
            ctx_clone,
        )
    }
}

fn process_msg<A: Actor + SyncActor>(
    actor: &mut A,
    inbox: &Inbox<A::Message>,
    ctx: &mut ActorContext<A>,
    state_tx: &Sender<A::ObservableState>
) -> Option<ActorTermination> {
    if !ctx.kill_switch().is_alive() {
        return Some(ActorTermination::KillSwitch);
    }

    ctx.progress().record_progress();
    let default_message_opt = actor.default_message();
    ctx.progress().record_progress();

    let reception_result = inbox.try_recv_msg_blocking(ctx.get_state() == ActorState::Running, default_message_opt);

    ctx.progress().record_progress();
    if !ctx.kill_switch().is_alive() {
        return Some(ActorTermination::KillSwitch);
    }
    match reception_result {
        ReceptionResult::Command(cmd) => {
            match cmd {
                Command::Pause => {
                    ctx.pause();
                    None
                }
                Command::Stop(cb) => {
                    let _ = cb.send(());
                    Some(ActorTermination::OnDemand)
                }
                Command::Start => {
                    ctx.resume();
                    None
                }
                Command::Observe(cb) => {
                    let state = actor.observable_state();
                    let _ = state_tx.send(state);
                    // We voluntarily ignore the error here. (An error only occurs if the
                    // sender dropped its receiver.)
                    let _ = cb.send(());
                    None
                }
            }
        }
        ReceptionResult::Message(msg) => {
            debug!(msg=?msg, actor=%actor.name(),"message-received");
            actor.process_message(msg, &ctx).err()
        }
        ReceptionResult::None => {
            if ctx.mailbox().is_last_mailbox() {
                Some(ActorTermination::Terminated)
            } else {
                None
            }
        }
        ReceptionResult::Disconnect => Some(ActorTermination::Terminated),
    }
}

fn sync_actor_loop<A: SyncActor>(
    mut actor: A,
    inbox: Inbox<A::Message>,
    mut ctx: ActorContext<A>,
    state_tx: Sender<A::ObservableState>
) -> ActorTermination {
    loop {
        let termination_opt = process_msg(&mut actor, &inbox, &mut ctx, &state_tx);
        if let Some(termination) = termination_opt {
            ctx.terminate();
            if termination.is_failure() {
                error!(actor=?actor.name(), termination=?termination, "actor termination (failure)");
                ctx.kill_switch().kill();
            } else {
                info!(actor=?actor.name(), termination=?termination, "actor termination (not a failure)");
            }
            if let Err(error) = actor.finalize(&termination, &ctx) {
                error!(error=?error, "Finalizing failed");
            }
            let final_state = actor.observable_state();
            let _ = state_tx.send(final_state);
            return termination;
        }
    }
}
