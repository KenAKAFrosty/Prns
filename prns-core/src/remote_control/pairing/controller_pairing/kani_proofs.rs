use super::*;
use crate::remote_control::{
    RemoteControlPairingAttemptTimeout, MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
};
use crate::units::{DurationMillis, InstantMillis};

#[kani::proof]
fn controller_attempt_deadlines_are_strictly_future_and_pairing_bounded() {
    let started_at = InstantMillis(kani::any());
    let pairing_expires_at = InstantMillis(kani::any());
    let offered_at = InstantMillis(kani::any());
    let timeout_millis: u32 = kani::any();
    kani::assume(started_at < pairing_expires_at);
    kani::assume(offered_at < pairing_expires_at);
    kani::assume(timeout_millis > 0);
    kani::assume(u64::from(timeout_millis) <= MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT.0);

    let Ok(pairing_window) =
        RemoteControlControllerPairingWindow::new(started_at, pairing_expires_at)
    else {
        return;
    };
    let Ok(attempt_timeout) =
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(u64::from(timeout_millis)))
    else {
        return;
    };
    let Ok(attempt_window) = RemoteControlControllerPairingAttemptWindow::new(
        offered_at,
        attempt_timeout,
        pairing_window,
    ) else {
        return;
    };

    assert!(attempt_window.expires_at() > offered_at);
    assert!(attempt_window.expires_at() <= pairing_expires_at);
}
