use super::super::*;
use personal_hopspot_core::{
    ApproveUsbFirstOwner, ApproveUsbFirstOwnerOutcome, BindUsbFirstOwnerWindow,
    BindUsbFirstOwnerWindowOutcome, PrepareUsbFirstOwnerWindow, PrepareUsbFirstOwnerWindowOutcome,
    UsbFirstOwner, UsbOwnershipRestore,
};
use personal_rns::engine::{
    ApproveRemoteControlTargetPairing, OpenRemoteControlPairing, RemoteControlTargetPairingApproval,
};
use personal_rns::remote_control::{
    RemoteControlPairingAdmissionMode, RemoteControlPairingAttemptId,
    RemoteControlPairingAttemptTimeout, RemoteControlPairingEndpoint,
    RemoteControlPairingExpiresAfter, RemoteControlPairingPermissions,
    RemoteControlPairingPublicAppDataBytes,
};
use personal_rns::runtime::EmbeddedPersistenceRestoreReport;
use personal_rns::units::{DurationMillis, InstantMillis};
use pipecircuit::StateMachine;

const WINDOW_MILLIS: u64 = 120_000;
const ATTEMPT_MILLIS: u64 = 60_000;
static RESTORED: Signal<Mtx, UsbFirstOwner> = Signal::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CONFIRMATIONS: Channel<
    Mtx,
    (RemoteControlPairingEndpoint, RemoteControlPairingAttemptId),
    1,
> = Channel::new();

pub(crate) fn restored(interface: InterfaceId, report: EmbeddedPersistenceRestoreReport) {
    let ownership = if report.remote_control_controller_grants_restored_count > 0 {
        UsbOwnershipRestore::Owned
    } else if report.warning.is_some()
        || report.remote_control_controller_grants_refused_count > 0
        || report.remote_control_controller_grants_dropped_count > 0
    {
        UsbOwnershipRestore::Uncertain
    } else {
        UsbOwnershipRestore::Unowned
    };
    let Some(deadline) = report.logical_start.0.checked_add(WINDOW_MILLIS) else {
        log::error!("USB first owner clock exhausted");
        return;
    };
    RESTORED.signal(UsbFirstOwner::new(
        interface,
        report.logical_start,
        InstantMillis(deadline),
        ownership,
    ));
}

pub(crate) fn confirmation(
    endpoint: RemoteControlPairingEndpoint,
    attempt: RemoteControlPairingAttemptId,
) {
    if ACTIVE.load(Ordering::Acquire) && CONFIRMATIONS.try_send((endpoint, attempt)).is_err() {
        ACTIVE.store(false, Ordering::Release);
        PAIRING_CLOSE_REQUESTED.store(true, Ordering::Release);
        PAIRING_CLOSE_WAKE.signal(());
    }
}

#[embassy_executor::task]
pub(crate) async fn run(
    handle: Handle,
    usb: &'static EmbassyInterfaceStatus,
    permissions: RemoteControlPairingPermissions,
    public_app_data: RemoteControlPairingPublicAppDataBytes,
) {
    let mut owner = RESTORED.wait().await;
    while !usb.connection().is_online() {
        Timer::after(Duration::from_millis(100)).await;
    }
    let now = remote_control_now();
    let Ok(PrepareUsbFirstOwnerWindowOutcome::Open {
        interface,
        expires_at,
    }) = owner.step(PrepareUsbFirstOwnerWindow {
        now,
        connected_interface: usb.id(),
    })
    else {
        return;
    };
    let remaining = expires_at.0 - now.0;
    let Ok(expires_after) = RemoteControlPairingExpiresAfter::try_from(DurationMillis(remaining))
    else {
        return;
    };
    let Ok(attempt_timeout) =
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(remaining.min(ATTEMPT_MILLIS)))
    else {
        return;
    };
    let _ = update_remote_control_state(screen::RemoteControlTargetPairingState::begin_opening);
    ACTIVE.store(true, Ordering::Release);
    let result = handle
        .open_remote_control_pairing(OpenRemoteControlPairing {
            admission: RemoteControlPairingAdmissionMode::DirectPhysical,
            target: AnnounceTarget::Interface(interface),
            expires_after,
            attempt_timeout,
            permissions,
            public_app_data,
        })
        .await;
    let opened = match result {
        Ok(opened) => opened,
        Err(error) => {
            ACTIVE.store(false, Ordering::Release);
            log::error!("USB first owner window failed: {error:?}");
            let _ = update_remote_control_state(|state| {
                state.operation_failed(None, screen::RemoteControlTargetPairingFailure::Open)
            });
            return;
        }
    };
    if owner.step(BindUsbFirstOwnerWindow {
        endpoint: opened.endpoint,
    }) != BindUsbFirstOwnerWindowOutcome::Bound
    {
        ACTIVE.store(false, Ordering::Release);
        return;
    }
    let _ = update_remote_control_state(|state| state.opened_direct(opened.expires_at));
    log::info!(
        "USB first owner window opened endpoint={:?}",
        opened.endpoint
    );
    loop {
        let remaining = expires_at.0.saturating_sub(remote_control_now().0);
        let next = with_timeout(Duration::from_millis(remaining), CONFIRMATIONS.receive()).await;
        let Ok((endpoint, attempt)) = next else {
            ACTIVE.store(false, Ordering::Release);
            return;
        };
        if !ACTIVE.load(Ordering::Acquire) {
            return;
        }
        match owner.step(ApproveUsbFirstOwner {
            now: remote_control_now(),
            endpoint,
            attempt,
        }) {
            Ok(ApproveUsbFirstOwnerOutcome::WrongEndpoint) => continue,
            Ok(ApproveUsbFirstOwnerOutcome::Approve { attempt }) => {
                ACTIVE.store(false, Ordering::Release);
                match handle
                    .approve_remote_control_target_pairing(ApproveRemoteControlTargetPairing {
                        attempt_id: attempt,
                    })
                    .await
                {
                    Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit {
                        attempt_id,
                    }) => {
                        let _ = update_remote_control_state(|state| {
                            state.awaiting_controller_commit(attempt_id)
                        });
                    }
                    Ok(RemoteControlTargetPairingApproval::AuthorizationOwed {
                        attempt_id,
                        ..
                    }) => {
                        let _ = update_remote_control_state(|state| state.authorizing(attempt_id));
                    }
                    Err(error) => {
                        log::error!("USB first owner approval failed: {error:?}");
                        let _ = update_remote_control_state(|state| {
                            state.operation_failed(
                                Some(attempt),
                                screen::RemoteControlTargetPairingFailure::Approval,
                            )
                        });
                    }
                }
                return;
            }
            Ok(ApproveUsbFirstOwnerOutcome::Unavailable | ApproveUsbFirstOwnerOutcome::Expired)
            | Err(_) => {
                ACTIVE.store(false, Ordering::Release);
                return;
            }
        }
    }
}
