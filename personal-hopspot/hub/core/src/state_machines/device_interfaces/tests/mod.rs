#![allow(clippy::unwrap_used, clippy::panic)]

extern crate alloc;

mod properties;
mod transitions;

use crate::*;
use core::num::NonZeroU32;
use pipecircuit::StateMachine;
use prns_core::crypto::{Ed25519PublicKey, X25519PublicKey};
use prns_core::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
};
use prns_core::interfaces::{
    ConnectionState as InterfaceConnectionState, InterfaceId, InterfaceKind, InterfaceMode,
};
use prns_core::remote_control::{
    RemoteControlInterfaceContinuation, RemoteControlInterfaceCursor, RemoteControlInterfaceEntry,
    RemoteControlInterfaceInventory, RemoteControlPairingAttemptId, RemoteControlTargetIdentity,
};
use prns_core::routing::links::LinkId;

const FIRST_LINK: LinkId = LinkId::new([1; 16]);
const SECOND_LINK: LinkId = LinkId::new([2; 16]);

fn registry() -> DeviceRegistry {
    DeviceRegistry::try_new(NonZeroU32::new(2).unwrap()).unwrap()
}

fn create(registry: &mut DeviceRegistry) -> DeviceId {
    let Ok(CreateDeviceOutcome::Created { device }) = registry.step(CreateDevice {
        label: DeviceLabel::new("MCU").unwrap(),
    }) else {
        panic!("creation refused");
    };
    device
}

fn target(seed: u8) -> RemoteControlTargetIdentity {
    RemoteControlTargetIdentity::new(IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([seed; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([seed; 32])),
    })
}

fn pair(registry: &mut DeviceRegistry, device: DeviceId, seed: u8) {
    let Ok(BeginEnrollmentOutcome::Started { enrollment }) = registry.step(BeginEnrollment {
        device,
        target: target(seed),
        attempt: RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([seed; 32]),
    }) else {
        panic!("enrollment refused");
    };
    assert_eq!(
        registry.step(CompleteEnrollment {
            enrollment,
            target: target(seed)
        }),
        CompleteEnrollmentOutcome::Recorded { enrollment }
    );
}

fn begin(registry: &mut DeviceRegistry, device: DeviceId) -> Connection {
    let Ok(BeginConnectionOutcome::Connect { connection }) =
        registry.step(BeginConnection { device })
    else {
        panic!("connection refused");
    };
    connection
}

fn confirm(registry: &mut DeviceRegistry, connection: Connection, link: LinkId) {
    assert_eq!(
        registry.step(ConfirmConnection {
            connection,
            target: connection.target(),
            link
        }),
        ConfirmConnectionOutcome::Connected { connection, link }
    );
}

fn end(registry: &mut DeviceRegistry, connection: Connection, link: LinkId) {
    assert_eq!(
        registry.step(EndConnection {
            connection,
            reason: DisconnectionReason::TransportLost
        }),
        EndConnectionOutcome::SessionEnded {
            connection,
            link,
            reason: DisconnectionReason::TransportLost
        }
    );
}

fn start(
    interfaces: &mut DeviceInterfaces<4>,
    registry: &mut DeviceRegistry,
    connection: Connection,
    link: LinkId,
    cancelled: Option<InterfacePageRequest>,
) -> InterfacePageRequest {
    let SynchronizeDeviceInterfacesOutcome::Started {
        request,
        link: actual_link,
        cancelled: actual_cancelled,
    } = interfaces.step(SynchronizeDeviceInterfaces { registry })
    else {
        panic!("inventory did not start");
    };
    assert_eq!(
        (request.connection(), actual_link, actual_cancelled),
        (connection, link, cancelled)
    );
    assert_eq!(
        interfaces.step(ReadDeviceInterfaces),
        ReadDeviceInterfacesOutcome::Found {
            device: connection.device(),
            inventory: InterfaceInventorySnapshot {
                connection,
                status: InterfaceInventoryStatus::Receiving { pending: request },
                interfaces: None
            },
        }
    );
    request
}

fn entry(seed: u8) -> RemoteControlInterfaceEntry {
    RemoteControlInterfaceEntry {
        id: InterfaceId::new([seed; 8]),
        kind: InterfaceKind::Loopback,
        mode: InterfaceMode::Full,
        connection: InterfaceConnectionState::Connected,
        enabled: true,
        tx_bytes: 0,
        rx_bytes: 0,
        links: 1,
        rate_bytes_per_sec: None,
    }
}

fn response(
    request: InterfacePageRequest,
    ids: &[u8],
    continuation: RemoteControlInterfaceContinuation,
) -> ReceiveInterfacePage {
    let mut page = RemoteControlInterfaceInventory::empty();
    for &id in ids {
        page.push(entry(id)).unwrap();
    }
    page.set_continuation(continuation).unwrap();
    ReceiveInterfacePage { request, page }
}

fn empty_response(request: InterfacePageRequest) -> ReceiveInterfacePage {
    response(request, &[], RemoteControlInterfaceContinuation::Complete)
}

fn failure(request: InterfacePageRequest) -> InterfaceRefreshFailed {
    InterfaceRefreshFailed {
        request,
        reason: InterfaceRefreshFailure::TransportLost,
    }
}

fn assert_stale(interfaces: &mut DeviceInterfaces<4>, request: InterfacePageRequest) {
    let before = interfaces.step(ReadDeviceInterfaces);
    assert_eq!(
        interfaces.step(empty_response(request)),
        ReceiveInterfacePageOutcome::StaleRequest {
            rejected: empty_response(request)
        }
    );
    assert_eq!(
        interfaces.step(failure(request)),
        InterfaceRefreshFailedOutcome::StaleRequest {
            rejected: failure(request)
        }
    );
    assert_eq!(interfaces.step(ReadDeviceInterfaces), before);
}
