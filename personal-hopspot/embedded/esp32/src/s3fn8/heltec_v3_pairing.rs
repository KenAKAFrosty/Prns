//! Heltec V3 adapter for the shared pairing composition on the SRAM runtime.
//! The protocol, durable authorization and UI transitions remain shared with S3.
use core::cell::RefCell;

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use personal_hopspot_core as screen;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, ApproveRemoteControlTargetPairing,
    OpenRemoteControlPairing, RejectRemoteControlTargetPairing, RemoteControlTargetPairingApproval,
};
use personal_rns::remote_control::{
    RemoteControlPairingAttemptId, RemoteControlPairingAttemptTimeout,
    RemoteControlPairingExpiresAfter, RemoteControlPairingPublicAppDataBytes,
};
use personal_rns::runtime::{Diagnostic, Message, PrnsEvent, RemoteControlPairingControl};
use portable_atomic::{AtomicBool, Ordering};

use super::Mtx;
use crate::remote_control_composition::{
    RemoteControlComposition, RemoteControlCompositionEffects, StableTargetAnnouncementSettlement,
};

type Handle = personal_rns::runtime::PrnsNodeHandle<
    'static,
    Mtx,
    { super::COMMANDS_CAP },
    { super::COMPLETIONS_CAP },
>;
static REMOTE_CONTROL_COMPOSITION: BlockingMutex<
    Mtx,
    RefCell<RemoteControlComposition<RemoteControlPairingAttemptId>>,
> = BlockingMutex::new(RefCell::new(RemoteControlComposition::new()));
static REMOTE_CONTROL_CLOCK: BlockingMutex<Mtx, RefCell<(u64, u64)>> =
    BlockingMutex::new(RefCell::new((0, 0)));
static REMOTE_CONTROL_UI_WAKE: Signal<Mtx, ()> = Signal::new();
static STABLE_TARGET_ANNOUNCER_WAKE: Signal<Mtx, ()> = Signal::new();
static PAIRING_CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);
static PAIRING_CLOSE_WAKE: Signal<Mtx, ()> = Signal::new();
static ENABLED: AtomicBool = AtomicBool::new(true);
static RENDERED_CONFIRMATION: BlockingMutex<Mtx, RefCell<Option<RemoteControlPairingAttemptId>>> =
    BlockingMutex::new(RefCell::new(None));

pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub(super) fn rendered_confirmation() -> Option<RemoteControlPairingAttemptId> {
    RENDERED_CONFIRMATION.lock(|value| *value.borrow())
}

pub(super) fn set_rendered_confirmation(attempt: Option<RemoteControlPairingAttemptId>) {
    RENDERED_CONFIRMATION.lock(|value| *value.borrow_mut() = attempt);
}

pub(super) fn display_failed() {
    ENABLED.store(false, Ordering::Release);
    set_rendered_confirmation(None);
    let _ = update_remote_control_state(|state| {
        state.operation_failed(
            state.attempt_id(),
            screen::RemoteControlTargetPairingFailure::Projection,
        )
    });
    // A display failure permanently disables this boot's local approval path.
    // Close independently of the UI task, even if a controller is already connected.
    PAIRING_CLOSE_REQUESTED.store(true, Ordering::Release);
    PAIRING_CLOSE_WAKE.signal(());
}

pub(super) async fn wait_for_refresh() {
    match select(
        REMOTE_CONTROL_UI_WAKE.wait(),
        Timer::after(Duration::from_millis(500)),
    )
    .await
    {
        Either::First(()) | Either::Second(()) => {}
    }
}

#[cfg(feature = "remote-control-pairing")]
fn apply_remote_control_effects(effects: RemoteControlCompositionEffects) {
    if effects.wake_ui() {
        REMOTE_CONTROL_UI_WAKE.signal(());
    }
    if effects.wake_announcer() {
        STABLE_TARGET_ANNOUNCER_WAKE.signal(());
    }
    if effects.close_pairing() {
        PAIRING_CLOSE_REQUESTED.store(true, Ordering::Release);
        PAIRING_CLOSE_WAKE.signal(());
    }
}

#[cfg(feature = "remote-control-pairing")]
fn update_remote_control_state(
    transition: impl FnOnce(
        &mut screen::RemoteControlTargetPairingState,
    ) -> screen::RemoteControlTargetPairingUpdate,
) -> screen::RemoteControlTargetPairingUpdate {
    let output = REMOTE_CONTROL_COMPOSITION
        .lock(|composition| composition.borrow_mut().update_pairing(transition));
    let (update, effects) = output.into_parts();
    apply_remote_control_effects(effects);
    update
}

#[cfg(feature = "remote-control-pairing")]
pub(super) fn state() -> screen::RemoteControlTargetPairingState {
    REMOTE_CONTROL_COMPOSITION.lock(|composition| composition.borrow_mut().take_current_pairing())
}

#[cfg(feature = "remote-control-pairing")]
fn remote_control_authorization_persisted(
    attempt_id: personal_rns::remote_control::RemoteControlPairingAttemptId,
) {
    let output = REMOTE_CONTROL_COMPOSITION
        .lock(|composition| composition.borrow_mut().authorization_persisted(attempt_id));
    let (_, effects) = output.into_parts();
    apply_remote_control_effects(effects);
}

#[cfg(feature = "remote-control-pairing")]
pub(super) fn on_event(event: PrnsEvent<'_>, _state: &super::AppState) {
    match event {
        PrnsEvent::Message(Message::RemoteControlTargetPairingConfirmationRequired(pairing)) => {
            let confirmation = pairing.confirmation();
            let attempt_id = confirmation.attempt_id();
            let confirmation_code = confirmation.confirmation_code().value();
            let expires_at = pairing.window().expires_at();
            let _ = update_remote_control_state(|state| {
                state.confirmation_required(attempt_id, confirmation_code, expires_at)
            });
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingControllerCommitted {
            attempt_id,
        }) => {
            let _ = update_remote_control_state(|state| state.controller_committed(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingAuthorizationRequired {
            attempt_id,
            ..
        }) => {
            let _ = update_remote_control_state(|state| state.authorizing(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingAuthorizationPersisted {
            attempt_id,
        }) => {
            remote_control_authorization_persisted(attempt_id);
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingExpiredDuringAuthorization {
            attempt_id,
        }) => {
            let _ = update_remote_control_state(|state| state.expired(Some(attempt_id)));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingExpired { aborted }) => {
            let attempt_id = aborted.attempt_id();
            let _ = update_remote_control_state(|state| state.expired(Some(attempt_id)));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingLinkClosed { aborted }) => {
            let attempt_id = aborted.attempt_id();
            let _ = update_remote_control_state(|state| state.terminal_link_closed(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingCompletionRetentionExpired {
            attempt_id,
        }) => {
            let _ = update_remote_control_state(|state| state.completion_expired(attempt_id));
        }
        PrnsEvent::Message(Message::RemoteControlTargetPairingCompletionLinkClosed {
            attempt_id,
        }) => {
            let _ = update_remote_control_state(|state| state.terminal_link_closed(attempt_id));
        }
        PrnsEvent::Diagnostic(Diagnostic::RemoteControlPairingExpired { .. }) => {
            let _ = update_remote_control_state(|state| state.expired(None));
        }
        PrnsEvent::Diagnostic(Diagnostic::RemoteControlPairingExpiryFailed { .. }) => {
            let _ = update_remote_control_state(|state| {
                state.operation_failed(
                    None,
                    screen::RemoteControlTargetPairingFailure::PairingExpiry,
                )
            });
        }
        _ => {}
    }
}

#[cfg(feature = "remote-control-pairing")]
pub(super) fn set_clock(logical_now: personal_rns::units::InstantMillis) {
    let raw_now = Instant::now().as_millis();
    REMOTE_CONTROL_CLOCK.lock(|clock| *clock.borrow_mut() = (raw_now, logical_now.0));
}

#[cfg(feature = "remote-control-pairing")]
pub(super) fn now() -> personal_rns::units::InstantMillis {
    let raw_now = Instant::now().as_millis();
    REMOTE_CONTROL_CLOCK.lock(|clock| {
        let (raw_start, logical_start) = *clock.borrow();
        personal_rns::units::InstantMillis(
            logical_start.saturating_add(raw_now.saturating_sub(raw_start)),
        )
    })
}

#[cfg(feature = "remote-control-pairing")]
pub(super) fn transmit_ready(ready: bool) {
    let effects = REMOTE_CONTROL_COMPOSITION
        .lock(|composition| composition.borrow_mut().set_transmit_ready(ready));
    apply_remote_control_effects(effects);
}

#[cfg(feature = "remote-control-pairing")]
pub(super) fn manual_announcements() {
    let effects = REMOTE_CONTROL_COMPOSITION
        .lock(|composition| composition.borrow_mut().request_manual_announcements());
    apply_remote_control_effects(effects);
}

#[cfg(feature = "remote-control-pairing")]
fn poll_stable_target_announcement(
    now_millis: u64,
) -> Option<screen::StableTargetAnnouncementAction> {
    let output = REMOTE_CONTROL_COMPOSITION
        .lock(|composition| composition.borrow_mut().poll_announcement(now_millis));
    let (action, effects) = output.into_parts();
    apply_remote_control_effects(effects);
    action
}

#[cfg(feature = "remote-control-pairing")]
fn settle_stable_target_announcement(
    action: screen::StableTargetAnnouncementAction,
    succeeded: bool,
) -> bool {
    let settlement = if succeeded {
        StableTargetAnnouncementSettlement::Succeeded
    } else {
        StableTargetAnnouncementSettlement::Failed
    };
    let output = REMOTE_CONTROL_COMPOSITION.lock(|composition| {
        composition
            .borrow_mut()
            .settle_announcement(action, settlement)
    });
    let (settled, effects) = output.into_parts();
    apply_remote_control_effects(effects);
    settled
}

#[cfg(feature = "remote-control-pairing")]
fn next_stable_target_announcement_deadline_millis() -> Option<u64> {
    REMOTE_CONTROL_COMPOSITION
        .lock(|composition| composition.borrow().next_announcement_deadline_millis())
}

#[cfg(feature = "remote-control-pairing")]
pub(crate) fn persistence_failed(
    failure: personal_rns::runtime::EmbeddedRemoteControlPairingPersistenceFailure,
) {
    use personal_rns::runtime::EmbeddedRemoteControlPairingPersistenceFailure as Failure;

    let attempt_id = match failure {
        Failure::CommittedActivation { attempt_id, .. }
        | Failure::AuthorizationTransaction { attempt_id, .. }
        | Failure::Storage { attempt_id, .. }
        | Failure::TargetSettlement { attempt_id, .. }
        | Failure::ControllerSettlement { attempt_id, .. }
        | Failure::UnexpectedTargetFinalization { attempt_id, .. }
        | Failure::UnexpectedControllerFinalization { attempt_id, .. }
        | Failure::RollbackSnapshotMismatch { attempt_id }
        | Failure::SettlementBusy { attempt_id, .. }
        | Failure::NodeStopped { attempt_id, .. } => attempt_id,
    };
    let output = REMOTE_CONTROL_COMPOSITION.lock(|composition| {
        composition
            .borrow_mut()
            .target_persistence_failed(attempt_id)
    });
    let (_, effects) = output.into_parts();
    apply_remote_control_effects(effects);
}

/// False only when closing failed and the UI must restore its retry action.
pub(super) async fn handle_action(handle: &Handle, action: screen::UiAction) -> bool {
    match action {
        screen::UiAction::OpenRemoteControlPairing => {
            if !enabled() {
                return true;
            }
            let _ =
                update_remote_control_state(screen::RemoteControlTargetPairingState::begin_opening);
            let permissions = screen::full_remote_control_pairing_permissions(
                &super::remote_control::capabilities().requests(),
            )
            .expect("RemoteControl supports Describe");
            let open = OpenRemoteControlPairing {
                target: AnnounceTarget::AllInterfaces,
                expires_after: RemoteControlPairingExpiresAfter::try_from(
                    personal_rns::units::DurationMillis(120_000),
                )
                .expect("valid window"),
                attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(
                    personal_rns::units::DurationMillis(60_000),
                )
                .expect("valid attempt timeout"),
                permissions,
                public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                    b"Personal Hopspot HeltecV3".as_slice(),
                )
                .expect("board name fits"),
            };
            match handle.open_remote_control_pairing(open).await {
                Ok(opened) => {
                    let _ = update_remote_control_state(|state| {
                        state.opened(opened.invitation_code.value(), opened.expires_at)
                    });
                }
                Err(_) => {
                    let _ = update_remote_control_state(|state| {
                        state
                            .operation_failed(None, screen::RemoteControlTargetPairingFailure::Open)
                    });
                }
            }
        }
        screen::UiAction::CloseRemoteControlPairing => {
            match handle.close_remote_control_pairing().await {
                Ok(_) => {
                    let _ = update_remote_control_state(
                        screen::RemoteControlTargetPairingState::cancelled,
                    );
                }
                Err(_) => {
                    let _ = update_remote_control_state(|state| {
                        state.operation_failed(
                            state.attempt_id(),
                            screen::RemoteControlTargetPairingFailure::Close,
                        )
                    });
                    return false;
                }
            }
        }
        screen::UiAction::ApproveRemoteControlTargetPairing(attempt_id) => {
            if !enabled() {
                return true;
            }
            match handle
                .approve_remote_control_target_pairing(ApproveRemoteControlTargetPairing {
                    attempt_id,
                })
                .await
            {
                Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit { attempt_id }) => {
                    let _ = update_remote_control_state(|state| {
                        state.awaiting_controller_commit(attempt_id)
                    });
                }
                Ok(RemoteControlTargetPairingApproval::AuthorizationOwed {
                    attempt_id, ..
                }) => {
                    let _ = update_remote_control_state(|state| state.authorizing(attempt_id));
                }
                Err(_) => {
                    let _ = update_remote_control_state(|state| {
                        state.operation_failed(
                            Some(attempt_id),
                            screen::RemoteControlTargetPairingFailure::Approval,
                        )
                    });
                }
            }
        }
        screen::UiAction::RejectRemoteControlTargetPairing(attempt_id) => {
            match handle
                .reject_remote_control_target_pairing(RejectRemoteControlTargetPairing {
                    attempt_id,
                })
                .await
            {
                Ok(_) => {
                    let _ = update_remote_control_state(|state| state.rejected(attempt_id));
                }
                Err(_) => {
                    let _ = update_remote_control_state(|state| {
                        state.operation_failed(
                            Some(attempt_id),
                            screen::RemoteControlTargetPairingFailure::Rejection,
                        )
                    });
                }
            }
        }
        _ => {}
    }
    true
}
#[cfg(feature = "remote-control-pairing")]
#[embassy_executor::task]
pub(super) async fn announcer_task(
    handle: Handle,
    node_page_destination: personal_rns::wire::DestinationHash,
    stable_target_destination: personal_rns::wire::DestinationHash,
) -> ! {
    loop {
        let now = Instant::now().as_millis();
        if let Some(action) = poll_stable_target_announcement(now) {
            let (destination, kind) = match action {
                screen::StableTargetAnnouncementAction::AutomaticStableTarget { .. } => {
                    (stable_target_destination, "automatic-stable")
                }
                screen::StableTargetAnnouncementAction::ManualNodePage => {
                    (node_page_destination, "manual-node")
                }
                screen::StableTargetAnnouncementAction::ManualStableTarget => {
                    (stable_target_destination, "manual-stable")
                }
            };
            let result = handle
                .announce_now(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                })
                .await;
            match &result {
                Ok(()) => log::info!(
                    "announce settled kind={kind} destination={destination:?} outcome=sent"
                ),
                Err(error) => log::error!(
                    "announce settled kind={kind} destination={destination:?} outcome=failed error={error:?}"
                ),
            }
            let settled = settle_stable_target_announcement(action, result.is_ok());
            debug_assert!(settled, "the serialized announce action remains in flight");
            continue;
        }

        if let Some(deadline) = next_stable_target_announcement_deadline_millis() {
            let deadline = Instant::try_from_millis(deadline).unwrap_or(Instant::MAX);
            match select(STABLE_TARGET_ANNOUNCER_WAKE.wait(), Timer::at(deadline)).await {
                Either::First(()) | Either::Second(()) => {}
            }
        } else {
            STABLE_TARGET_ANNOUNCER_WAKE.wait().await;
        }
    }
}

#[cfg(feature = "remote-control-pairing")]
#[embassy_executor::task]
pub(super) async fn close_task(handle: Handle) -> ! {
    loop {
        PAIRING_CLOSE_WAKE.wait().await;
        if !PAIRING_CLOSE_REQUESTED.swap(false, Ordering::AcqRel) {
            continue;
        }
        match handle.close_remote_control_pairing().await {
            Ok(outcome) => {
                log::warn!("remote-control pairing failed closed: {outcome:?}");
            }
            Err(error) => {
                log::error!("remote-control pairing fail-close command failed: {error:?}");
            }
        }
    }
}
