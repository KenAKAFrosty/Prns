use super::{AppState, Handle, Mtx};
use crate::remote_control_composition::{
    RemoteControlComposition, StableTargetAnnouncementSettlement,
};
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Instant;
use personal_hopspot_core as hopspot;
use personal_rns::engine::{
    AnnounceTarget, ApproveRemoteControlTargetPairing, OpenRemoteControlPairing,
    RejectRemoteControlTargetPairing, RemoteControlTargetPairingApproval,
};
use personal_rns::remote_control::RemoteControlPairingAttemptId;
use personal_rns::remote_control::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingExpiresAfter,
    RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
};
use personal_rns::runtime::{Diagnostic, Message, PrnsEvent, RemoteControlPairingControl};
use personal_rns::units::DurationMillis;
use personal_rns::units::InstantMillis;

const PAIRING_WINDOW_MILLIS: u64 = 120_000;
const PAIRING_ATTEMPT_MILLIS: u64 = 60_000;
static COMPOSITION: Mutex<Mtx, RefCell<RemoteControlComposition<RemoteControlPairingAttemptId>>> =
    Mutex::new(RefCell::new(RemoteControlComposition::new()));
static CLOCK: Mutex<Mtx, RefCell<(u64, u64)>> = Mutex::new(RefCell::new((0, 0)));
static ENABLED: AtomicBool = AtomicBool::new(true);
static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

fn update(
    transition: impl FnOnce(
        &mut hopspot::RemoteControlTargetPairingState,
    ) -> hopspot::RemoteControlTargetPairingUpdate,
) -> hopspot::RemoteControlTargetPairingUpdate {
    let (result, effects) = COMPOSITION
        .lock(|state| state.borrow_mut().update_pairing(transition))
        .into_parts();
    if effects.close_pairing() {
        CLOSE_REQUESTED.store(true, Ordering::Release);
    }
    result
}
pub(super) fn state() -> hopspot::RemoteControlTargetPairingState {
    COMPOSITION.lock(|state| state.borrow_mut().take_current_pairing())
}
pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}
pub(super) fn persistence_failed() {
    ENABLED.store(false, Ordering::Release);
    update(|state| {
        state.operation_failed(
            state.attempt_id(),
            hopspot::RemoteControlTargetPairingFailure::Persistence,
        )
    });
    CLOSE_REQUESTED.store(true, Ordering::Release);
}
pub(super) fn display_failed() {
    persistence_failed();
}

pub(super) fn on_event(event: PrnsEvent<'_>, _state: &AppState) {
    match event {
        PrnsEvent::Message(Message::RemoteControlTargetPairingConfirmationRequired(pairing)) => {
            let confirmation = pairing.confirmation();
            let attempt_id = confirmation.attempt_id();
            let confirmation_code = confirmation.confirmation_code().value();
            let expires_at = pairing.window().expires_at();
            let _ = update(|state| {
                state.confirmation_required(attempt_id, confirmation_code, expires_at)
            });
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingControllerCommitted {
            attempt_id,
        }) => {
            let _ = update(|state| state.controller_committed(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingAuthorizationRequired {
            attempt_id,
            ..
        }) => {
            let _ = update(|state| state.authorizing(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingAuthorizationPersisted {
            attempt_id,
        }) => {
            let _ =
                COMPOSITION.lock(|state| state.borrow_mut().authorization_persisted(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingExpiredDuringAuthorization {
            attempt_id,
        }) => {
            let _ = update(|state| state.expired(Some(attempt_id)));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingExpired { aborted }) => {
            let attempt_id = aborted.attempt_id();
            let _ = update(|state| state.expired(Some(attempt_id)));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingLinkClosed { aborted }) => {
            let attempt_id = aborted.attempt_id();
            let _ = update(|state| state.terminal_link_closed(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingCompletionRetentionExpired {
            attempt_id,
        }) => {
            let _ = update(|state| state.completion_expired(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingCompletionLinkClosed {
            attempt_id,
        }) => {
            let _ = update(|state| state.terminal_link_closed(attempt_id));
        }
        PrnsEvent::Diagnostic(Diagnostic::RemoteControlPairingExpired { .. }) => {
            let _ = update(|state| state.expired(None));
        }
        PrnsEvent::Diagnostic(Diagnostic::RemoteControlPairingExpiryFailed { .. }) => {
            let _ = update(|state| {
                state.operation_failed(
                    None,
                    hopspot::RemoteControlTargetPairingFailure::PairingExpiry,
                )
            });
        }
        _ => {}
    }
}

pub(super) async fn poll(handle: &Handle) {
    if CLOSE_REQUESTED.swap(false, Ordering::AcqRel) {
        if handle.close_remote_control_pairing().await.is_err() {
            CLOSE_REQUESTED.store(true, Ordering::Release);
        }
    }
}

pub(super) async fn execute(
    action: hopspot::UiAction,
    handle: &Handle,
    permissions: &RemoteControlPairingPermissions,
    public_app_data: &RemoteControlPairingPublicAppDataBytes,
) -> Result<(), hopspot::RemoteControlTargetPairingFailure> {
    use hopspot::{RemoteControlTargetPairingFailure as Failure, UiAction};
    match action {
        UiAction::OpenRemoteControlPairing => {
            if !enabled() {
                return Err(Failure::Persistence);
            }
            update(hopspot::RemoteControlTargetPairingState::begin_opening);
            match handle
                .open_remote_control_pairing(OpenRemoteControlPairing {
                    target: AnnounceTarget::AllInterfaces,
                    expires_after: RemoteControlPairingExpiresAfter::try_from(DurationMillis(
                        PAIRING_WINDOW_MILLIS,
                    ))
                    .expect("pairing window valid"),
                    attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(DurationMillis(
                        PAIRING_ATTEMPT_MILLIS,
                    ))
                    .expect("pairing attempt valid"),
                    permissions: permissions.clone(),
                    public_app_data: public_app_data.clone(),
                })
                .await
            {
                Ok(opened) => {
                    update(|state| state.opened(opened.invitation_code.value(), opened.expires_at));
                }
                Err(_) => {
                    update(|state| state.operation_failed(None, Failure::Open));
                    return Err(Failure::Open);
                }
            }
        }
        UiAction::CloseRemoteControlPairing => {
            if handle.close_remote_control_pairing().await.is_err() {
                update(|state| state.operation_failed(state.attempt_id(), Failure::Close));
                return Err(Failure::Close);
            }
            update(hopspot::RemoteControlTargetPairingState::cancelled);
        }
        UiAction::ApproveRemoteControlTargetPairing(attempt_id) => {
            if !enabled() {
                return Err(Failure::Persistence);
            }
            match handle
                .approve_remote_control_target_pairing(ApproveRemoteControlTargetPairing {
                    attempt_id,
                })
                .await
            {
                Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit { attempt_id }) => {
                    update(|state| state.awaiting_controller_commit(attempt_id));
                }
                Ok(RemoteControlTargetPairingApproval::AuthorizationOwed {
                    attempt_id, ..
                }) => {
                    update(|state| state.authorizing(attempt_id));
                }
                Err(_) => {
                    update(|state| state.operation_failed(Some(attempt_id), Failure::Approval));
                    return Err(Failure::Approval);
                }
            }
        }
        UiAction::RejectRemoteControlTargetPairing(attempt_id) => {
            if handle
                .reject_remote_control_target_pairing(RejectRemoteControlTargetPairing {
                    attempt_id,
                })
                .await
                .is_err()
            {
                update(|state| state.operation_failed(Some(attempt_id), Failure::Rejection));
                return Err(Failure::Rejection);
            }
            update(|state| state.rejected(attempt_id));
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn restored(logical_start: InstantMillis, count: u32) {
    CLOCK.lock(|clock| *clock.borrow_mut() = (logical_start.0, Instant::now().as_millis()));
    COMPOSITION.lock(|state| {
        state.borrow_mut().observe_restored_controller_grants(count);
    });
}
pub(super) fn now() -> InstantMillis {
    CLOCK.lock(|clock| {
        let (logical, elapsed) = *clock.borrow();
        InstantMillis(logical.saturating_add(Instant::now().as_millis().saturating_sub(elapsed)))
    })
}
pub(super) fn request_announce() {
    COMPOSITION.lock(|state| {
        state.borrow_mut().request_manual_announcements();
    });
}
pub(super) async fn announce(
    handle: &Handle,
    node: personal_rns::wire::DestinationHash,
    stable: personal_rns::wire::DestinationHash,
    ready: bool,
) {
    let action = COMPOSITION.lock(|state| {
        let mut state = state.borrow_mut();
        state.set_transmit_ready(ready);
        state
            .poll_announcement(Instant::now().as_millis())
            .into_parts()
            .0
    });
    if let Some(action) = action {
        let destination = match action {
            hopspot::StableTargetAnnouncementAction::ManualNodePage => node,
            hopspot::StableTargetAnnouncementAction::ManualStableTarget
            | hopspot::StableTargetAnnouncementAction::AutomaticStableTarget { .. } => stable,
        };
        let result = handle
            .announce_now(personal_rns::engine::AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: personal_rns::engine::AnnounceAppData::Registered,
            })
            .await;
        let settlement = if result.is_ok() {
            StableTargetAnnouncementSettlement::Succeeded
        } else {
            StableTargetAnnouncementSettlement::Failed
        };
        COMPOSITION.lock(|state| {
            state.borrow_mut().settle_announcement(action, settlement);
        });
    }
}
