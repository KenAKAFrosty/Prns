//! One bounded replay owner; no process-resident copy of the durable backlog.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayLimits {
    pub max_active_attempts: usize,
    pub max_page_rows: usize,
}
impl Default for ReplayLimits {
    fn default() -> Self {
        Self {
            max_active_attempts: 8,
            max_page_rows: 32,
        }
    }
}
impl ReplayLimits {
    pub fn validate(self) -> Result<(), DurableDirectServiceStartError> {
        if self.max_active_attempts == 0
            || self.max_page_rows == 0
            || self.max_active_attempts > isize::MAX as usize
            || self.max_page_rows > u32::MAX as usize
        {
            return Err(DurableDirectServiceStartError::InvalidLimits);
        }
        Ok(())
    }
}

pub(super) async fn run(
    shared: Arc<DurableShared>,
    weak: Weak<DurableLifecycle>,
    transition: Arc<Mutex<()>>,
    wake: Arc<Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut after = None;
    let mut continue_scan = false;
    loop {
        if !continue_scan {
            tokio::select! {
                ()=wake.notified()=>{},
                // An admitted insertion may outlive its result waiter, losing
                // only the wake. Bounded polling recovers that durable row.
                ()=tokio::time::sleep(Duration::from_secs(1))=>{},
                changed=shutdown.changed()=>{if changed.is_err() || *shutdown.borrow() {return;}}
            }
        }
        continue_scan = false;
        if *shutdown.borrow() {
            return;
        }
        // Never hold a strong lifecycle reference across an await in its own
        // task: dropping the last owner must still abort this retained handle.
        let gate = transition.lock().await;
        let Some(lifecycle) = weak.upgrade() else {
            return;
        };
        if lifecycle.stopped.load(Ordering::Acquire) {
            return;
        }
        if !lifecycle.activated.load(Ordering::Acquire) {
            continue;
        }
        let free = lifecycle
            .limits
            .max_active_attempts
            .saturating_sub(lifecycle.attempts().len());
        if free == 0 {
            continue;
        }
        let limit = free.min(lifecycle.limits.max_page_rows);
        drop(lifecycle);
        let reply = tokio::select! {
            reply=shared.mailbox.submit(MailboxRequest::Replay(MailboxReplayRequest {after,limit}))=>reply,
            changed=shutdown.changed()=>{if changed.is_err() || *shutdown.borrow() {return;}else{continue;}}
        };
        let Some(lifecycle) = weak.upgrade() else {
            return;
        };
        if lifecycle.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut retry = false;
        match reply {
            Ok(MailboxReply::QueuedPage { messages, next, .. }) if messages.len() <= limit => {
                shared.set_mailbox_read_ready();
                for message in messages {
                    spawn_attempt(&shared, &lifecycle, message);
                }
                after = next;
                continue_scan = next.is_some();
            }
            Err(failure) => {
                shared.set_mailbox_read_failure(&failure);
                if matches!(
                    failure,
                    MailboxFailure::ResetRequired(_) | MailboxFailure::Capacity
                ) {
                    lifecycle.activated.store(false, Ordering::Release);
                } else {
                    retry = true;
                }
            }
            Ok(_) => {
                shared.set_unexpected_mailbox_read_failure("invalid bounded replay reply");
                lifecycle.activated.store(false, Ordering::Release);
            }
        }
        drop(lifecycle);
        drop(gate);
        if retry {
            tokio::select! {
                ()=tokio::time::sleep(SETTLEMENT_RETRY_DELAY)=>{continue_scan=true;},
                changed=shutdown.changed()=>{if changed.is_err() || *shutdown.borrow() {return;}}
            }
        } else if continue_scan {
            tokio::task::yield_now().await;
        }
    }
}
