//! One persistent settled-control loop per physical connection.

use super::{BleControl, CloseReason};
use prns_core::interfaces::bluetooth_auto::{
    Control, LivenessAction, LivenessMode, LivenessPolicy,
};
use tokio::time::{Duration, Instant};

#[derive(Debug)]
pub(super) enum ControlError<E> {
    Transport(E),
    LivenessExpired,
    EntropyUnavailable,
}

pub(super) async fn observe_control<C: BleControl>(
    control: &mut Option<C>,
) -> Result<CloseReason, ControlError<C::Error>> {
    let Some(control) = control else {
        return std::future::pending().await;
    };
    let mode = control.liveness_mode();
    crate::diagnostic_log::debug!("bluetooth: settled control liveness {mode:?}");
    observe_with_entropy(control, mode, || {
        let mut seed = [0; 8];
        getrandom::getrandom(&mut seed).ok()?;
        Some(u64::from_be_bytes(seed))
    })
    .await
}

#[cfg(test)]
async fn observe_with_seed<C: BleControl>(
    control: &mut C,
    mode: LivenessMode,
    seed: u64,
) -> Result<CloseReason, ControlError<C::Error>> {
    observe_with_entropy(control, mode, || Some(seed)).await
}

async fn observe_with_entropy<C: BleControl>(
    control: &mut C,
    mode: LivenessMode,
    mut entropy: impl FnMut() -> Option<u64>,
) -> Result<CloseReason, ControlError<C::Error>> {
    let started = Instant::now();
    let now_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let seed = if mode == LivenessMode::Initiator {
        entropy().ok_or(ControlError::EntropyUnavailable)?
    } else {
        0
    };
    let mut seeded = mode != LivenessMode::Listener;
    let mut policy = LivenessPolicy::new(mode, 0, seed);
    loop {
        // Process expiration before ready input: neither a queued reply after
        // suspension nor a greeting flood may renew an expired session.
        match policy.poll(now_ms()) {
            LivenessAction::Retire => return Err(ControlError::LivenessExpired),
            LivenessAction::Send(write) => {
                let message = write.control();
                // The policy caps this deadline at every outstanding hard
                // expiration. Keep this one send future alive until it settles;
                // unrelated data activity must not restart a native operation.
                let deadline = started + Duration::from_millis(write.deadline_ms);
                let result = tokio::time::timeout_at(deadline, control.send(&message)).await;
                match result {
                    Ok(Ok(())) => policy.sent(now_ms(), write, true),
                    Ok(Err(error)) => return Err(ControlError::Transport(error)),
                    Err(_) => return Err(ControlError::LivenessExpired),
                }
            }
            LivenessAction::Wait => {
                let wake = async {
                    match policy.next_deadline_ms() {
                        Some(deadline) => {
                            tokio::time::sleep_until(started + Duration::from_millis(deadline))
                                .await;
                        }
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    // When both are ready, expiration wins over queued input.
                    biased;
                    () = wake => {},
                    received = control.recv() => {
                        match received.map_err(ControlError::Transport)? {
                            Control::Close { reason } => return Ok(reason),
                            message => {
                                let now = now_ms();
                                if !seeded && matches!(message, Control::Probe { .. }) {
                                    // Mere capability publication is not peer opt-in.
                                    // A legacy listener needs neither entropy nor timers.
                                    let seed = entropy().ok_or(ControlError::EntropyUnavailable)?;
                                    policy = LivenessPolicy::new(mode, now, seed);
                                    seeded = true;
                                }
                                policy.receive(now, &message);
                            },
                        }
                    },
                }
            }
        }
        // Cancellation-safe receives can be immediately ready indefinitely.
        // Preserve progress for the independently running data/owner futures.
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests;
