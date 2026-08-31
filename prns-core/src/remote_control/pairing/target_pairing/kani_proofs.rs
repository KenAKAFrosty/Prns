use super::*;
use crate::remote_control::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingWindow,
    MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
};
use crate::units::{DurationMillis, InstantMillis};

#[kani::proof]
fn target_attempt_deadlines_are_strictly_future_and_pairing_bounded() {
    let opened_at = InstantMillis(kani::any());
    let pairing_expires_at = InstantMillis(kani::any());
    let started_at = InstantMillis(kani::any());
    let timeout_millis: u32 = kani::any();
    kani::assume(opened_at < pairing_expires_at);
    kani::assume(started_at < pairing_expires_at);
    kani::assume(timeout_millis > 0);
    kani::assume(u64::from(timeout_millis) <= MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT.0);

    let Ok(pairing_window) = RemoteControlPairingWindow::new(opened_at, pairing_expires_at) else {
        return;
    };
    let Ok(attempt_timeout) =
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(u64::from(timeout_millis)))
    else {
        return;
    };
    let Ok(attempt_window) =
        RemoteControlTargetPairingAttemptWindow::new(started_at, attempt_timeout, &pairing_window)
    else {
        return;
    };

    assert!(attempt_window.expires_at() > started_at);
    assert!(attempt_window.expires_at() <= pairing_expires_at);
}
