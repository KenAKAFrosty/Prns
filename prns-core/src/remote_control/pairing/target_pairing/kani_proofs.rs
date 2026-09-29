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

#[kani::proof]
fn unexpired_pairing_windows_always_admit_a_bounded_attempt_timeout() {
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
    let Ok(configured) =
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(u64::from(timeout_millis)))
    else {
        return;
    };
    let actual =
        super::state::attempt_timeout_for_remaining_window(configured, started_at, &pairing_window);
    assert!(actual.duration().0 > 0);
    assert!(actual.duration() <= configured.duration());
    assert_eq!(
        actual.duration().0,
        configured
            .duration()
            .0
            .min(pairing_expires_at.0 - started_at.0),
    );
    let window = RemoteControlTargetPairingAttemptWindow::new(started_at, actual, &pairing_window);
    assert!(window.is_ok());
    if let Ok(window) = window {
        assert!(window.expires_at() > started_at);
        assert!(window.expires_at() <= pairing_expires_at);
    }
}
