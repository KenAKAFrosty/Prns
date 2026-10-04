#![allow(clippy::unwrap_used, clippy::panic)]

extern crate alloc;
use super::*;
use crate::*;
use core::num::NonZeroU32;
use prns_core::crypto::{Ed25519PublicKey, X25519PublicKey};
use prns_core::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
};
use prns_core::remote_control::*;
use prns_core::routing::links::LinkId;
use proptest::prelude::*;

const DISPLAY: DeviceControlCommand =
    DeviceControlCommand::DisplayVisibility(RemoteControlDisplayVisibility::Hidden);
const GNSS: DeviceControlCommand = DeviceControlCommand::GnssPower(RemoteControlGnssPower::On);

fn fixture() -> (DeviceRegistry, DeviceId, DeviceControls) {
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(2).unwrap()).unwrap();
    let CreateDeviceOutcome::Created { device } = registry
        .step(CreateDevice {
            label: DeviceLabel::new("MCU").unwrap(),
        })
        .unwrap()
    else {
        panic!("capacity")
    };
    let target = RemoteControlTargetIdentity::new(IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([1; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([2; 32])),
    });
    let BeginEnrollmentOutcome::Started { enrollment } = registry
        .step(BeginEnrollment {
            device,
            target: RemoteControlTargetIdentity::new(*target.public_keys()),
            attempt: RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([1; 32]),
        })
        .unwrap()
    else {
        panic!("enrollment")
    };
    assert_eq!(
        registry.step(CompleteEnrollment { enrollment, target }),
        CompleteEnrollmentOutcome::Recorded { enrollment }
    );
    (registry, device, DeviceControls::new(device))
}

fn connect(registry: &mut DeviceRegistry, device: DeviceId) -> Connection {
    let BeginConnectionOutcome::Connect { connection } =
        registry.step(BeginConnection { device }).unwrap()
    else {
        panic!("connection")
    };
    assert_eq!(
        registry.step(ConfirmConnection {
            connection,
            target: connection.target(),
            link: LinkId::new([1; 16])
        }),
        ConfirmConnectionOutcome::Connected {
            connection,
            link: LinkId::new([1; 16])
        }
    );
    connection
}

fn request(control: &mut DeviceControls, command: DeviceControlCommand) -> DeviceControlRequest {
    let RequestDeviceControlOutcome::Requested { request } =
        control.step(RequestDeviceControl { command }).unwrap()
    else {
        panic!("request")
    };
    assert_eq!(request.command(), command);
    request
}

#[test]
fn requests_require_a_confirmed_connection_and_preserve_the_exact_command() {
    let (mut registry, device, mut control) = fixture();
    assert_eq!(
        control.step(ReadDeviceControls),
        DeviceControlsSnapshot {
            connection: None,
            pending: None
        }
    );
    assert_eq!(
        control.step(SynchronizeDeviceControls {
            registry: &mut registry
        }),
        None
    );
    assert_eq!(
        control.step(RequestDeviceControl { command: DISPLAY }),
        Ok(RequestDeviceControlOutcome::NotConnected)
    );
    let BeginConnectionOutcome::Connect { connection } =
        registry.step(BeginConnection { device }).unwrap()
    else {
        panic!("connection")
    };
    assert_eq!(
        control.step(SynchronizeDeviceControls {
            registry: &mut registry
        }),
        None
    );
    assert_eq!(
        control.step(RequestDeviceControl { command: GNSS }),
        Ok(RequestDeviceControlOutcome::NotConnected)
    );
    let _confirmed = registry.step(ConfirmConnection {
        connection,
        target: connection.target(),
        link: LinkId::new([1; 16]),
    });
    assert_eq!(
        control.step(SynchronizeDeviceControls {
            registry: &mut registry
        }),
        None
    );
    let first = request(&mut control, DISPLAY);
    assert_eq!(first.connection(), connection);
    assert_eq!(
        DISPLAY.request_kind(),
        RemoteControlRequestKind::SetDisplayVisibility
    );
    assert_eq!(GNSS.request_kind(), RemoteControlRequestKind::SetGnssPower);
    assert_eq!(
        control.step(ReadDeviceControls),
        DeviceControlsSnapshot {
            connection: Some(connection),
            pending: Some(first)
        }
    );
    assert_eq!(
        control.step(SynchronizeDeviceControls {
            registry: &mut registry
        }),
        None
    );
    assert_eq!(
        control.step(RequestDeviceControl { command: GNSS }),
        Ok(RequestDeviceControlOutcome::Busy { pending: first })
    );
    assert_eq!(
        control.step(SettleDeviceControl { request: first }),
        SettleDeviceControlOutcome::Settled { request: first }
    );
    let next = request(&mut control, GNSS);
    assert_ne!(next, first);
    assert_eq!(
        control.step(SettleDeviceControl { request: first }),
        SettleDeviceControlOutcome::StaleRequest { request: first }
    );
    assert_eq!(control.step(ReadDeviceControls).pending, Some(next));
    let forged = DeviceControlRequest {
        command: DISPLAY,
        ..next
    };
    assert_eq!(
        control.step(SettleDeviceControl { request: forged }),
        SettleDeviceControlOutcome::StaleRequest { request: forged }
    );
    assert_eq!(control.step(ReadDeviceControls).pending, Some(next));
}

#[test]
fn disconnect_forget_and_reconnect_cancel_work_without_reusing_request_ids() {
    let (mut registry, device, mut control) = fixture();
    let connection = connect(&mut registry, device);
    let _cancelled = control.step(SynchronizeDeviceControls {
        registry: &mut registry,
    });
    let first = request(&mut control, DISPLAY);
    let _ended = registry.step(EndConnection {
        connection,
        reason: DisconnectionReason::Cancelled,
    });
    assert_eq!(
        control.step(SynchronizeDeviceControls {
            registry: &mut registry
        }),
        Some(first)
    );
    assert_eq!(
        control.step(ReadDeviceControls),
        DeviceControlsSnapshot {
            connection: None,
            pending: None
        }
    );
    let next_connection = connect(&mut registry, device);
    let _cancelled = control.step(SynchronizeDeviceControls {
        registry: &mut registry,
    });
    let next = request(&mut control, DISPLAY);
    assert_eq!(next.connection(), next_connection);
    assert_ne!(first.generation, next.generation);
    assert_eq!(
        control.step(SettleDeviceControl { request: first }),
        SettleDeviceControlOutcome::StaleRequest { request: first }
    );
    let _forgotten = registry.step(ForgetDevice { device });
    assert_eq!(
        control.step(SynchronizeDeviceControls {
            registry: &mut registry
        }),
        Some(next)
    );
    assert_eq!(
        control.step(SynchronizeDeviceControls {
            registry: &mut registry
        }),
        None
    );
    assert_eq!(
        control.step(RequestDeviceControl { command: DISPLAY }),
        Ok(RequestDeviceControlOutcome::NotConnected)
    );
}

#[test]
fn request_exhaustion_never_wraps_or_changes_pending_work() {
    let (mut registry, device, mut control) = fixture();
    let _connection = connect(&mut registry, device);
    let _cancelled = control.step(SynchronizeDeviceControls {
        registry: &mut registry,
    });
    control.next_request = NonZeroU64::new(u64::MAX);
    let last = request(&mut control, GNSS);
    assert_eq!(last.generation.get(), u64::MAX);
    assert_eq!(
        control.step(RequestDeviceControl { command: DISPLAY }),
        Ok(RequestDeviceControlOutcome::Busy { pending: last })
    );
    assert_eq!(
        control.step(SettleDeviceControl { request: last }),
        SettleDeviceControlOutcome::Settled { request: last }
    );
    assert_eq!(
        control.step(RequestDeviceControl { command: DISPLAY }),
        Err(RequestDeviceControlError::IdentifiersExhausted)
    );
    assert_eq!(control.step(ReadDeviceControls).pending, None);
}

proptest! {
    #[test]
    fn arbitrary_requests_and_reconnections_never_accept_retired_replies(actions in prop::collection::vec(0u8..5, 0..80)) {
        let (mut registry, device, mut control) = fixture();
        let mut connection = connect(&mut registry, device);
        let _cancelled = control.step(SynchronizeDeviceControls { registry: &mut registry });
        let mut pending = None;
        let mut retired = alloc::vec::Vec::new();
        let mut generation = 1u64;
        for action in actions {
            match action {
                0 | 1 => {
                    let command = if action == 0 { DISPLAY } else { GNSS };
                    let outcome = control.step(RequestDeviceControl { command }).unwrap();
                    if let Some(request) = pending {
                        prop_assert_eq!(outcome, RequestDeviceControlOutcome::Busy { pending: request });
                    } else {
                        let RequestDeviceControlOutcome::Requested { request } = outcome else { panic!("request") };
                        prop_assert_eq!(request.connection(), connection);
                        prop_assert_eq!(request.command(), command);
                        prop_assert_eq!(request.generation.get(), generation);
                        generation = generation.checked_add(1).unwrap();
                        pending = Some(request);
                    }
                }
                2 => {
                    if let Some(request) = pending.take() {
                        prop_assert_eq!(control.step(SettleDeviceControl { request }), SettleDeviceControlOutcome::Settled { request });
                        retired.push(request);
                    }
                }
                3 => {
                    let _ended = registry.step(EndConnection { connection, reason: DisconnectionReason::TransportLost });
                    connection = connect(&mut registry, device);
                    prop_assert_eq!(control.step(SynchronizeDeviceControls { registry: &mut registry }), pending);
                    if let Some(request) = pending.take() { retired.push(request); }
                }
                _ => { prop_assert_eq!(control.step(SynchronizeDeviceControls { registry: &mut registry }), None); }
            }
            for &request in &retired {
                prop_assert_eq!(control.step(SettleDeviceControl { request }), SettleDeviceControlOutcome::StaleRequest { request });
            }
            prop_assert_eq!(control.step(ReadDeviceControls), DeviceControlsSnapshot { connection: Some(connection), pending });
        }
    }
}
