use super::Host;
use crate::engine::{InstantMillis, NextWake, WakeReason};

pub async fn wait_for_due_reason<H: Host>(host: &H, scheduled_wake: NextWake) -> WakeReason {
    match scheduled_wake {
        NextWake::Idle => core::future::pending().await,
        NextWake::Due(reason) => reason,
        NextWake::At { at, reason } => {
            host.sleep_until(at).await;
            reason
        }
    }
}

pub async fn wait_for_pacer<H: Host>(host: &H, deadline: Option<InstantMillis>) {
    match deadline {
        Some(at) => host.sleep_until(at).await,
        None => core::future::pending().await,
    }
}
