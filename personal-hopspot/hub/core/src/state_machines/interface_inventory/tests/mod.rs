#![allow(clippy::unwrap_used, clippy::panic)]

extern crate alloc;

mod properties;
mod transitions;

use super::*;
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
    RemoteControlInterfaceContinuation, RemoteControlInterfaceCursor,
    RemoteControlInterfaceInventory, RemoteControlInterfacePage, RemoteControlPairingAttemptId,
    RemoteControlRequest, RemoteControlResponse, RemoteControlTargetIdentity,
};
use prns_core::routing::links::LinkId;

fn connected(seed: u8) -> (DeviceRegistry, Connection) {
    let target = IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([seed; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([seed; 32])),
    };
    let mut registry = DeviceRegistry::try_new(NonZeroU32::MIN).unwrap();
    let CreateDeviceOutcome::Created { device } = registry.step(CreateDevice {
        label: DeviceLabel::new("MCU").unwrap(),
    }) else {
        panic!("creation refused");
    };
    let BeginEnrollmentOutcome::Started { enrollment } = registry.step(BeginEnrollment {
        device,
        target: RemoteControlTargetIdentity::new(target),
        attempt: RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([seed; 32]),
    }) else {
        panic!("enrollment refused");
    };
    assert_eq!(
        registry.step(CompleteEnrollment {
            enrollment,
            target: RemoteControlTargetIdentity::new(target)
        }),
        CompleteEnrollmentOutcome::Recorded { enrollment }
    );
    let BeginConnectionOutcome::Connect { connection } = registry.step(BeginConnection { device })
    else {
        panic!("connection refused");
    };
    assert_eq!(
        registry.step(ConfirmConnection {
            connection,
            target: RemoteControlTargetIdentity::new(target),
            link: LinkId::new([seed; 16])
        }),
        ConfirmConnectionOutcome::Connected {
            connection,
            link: LinkId::new([seed; 16])
        }
    );
    (registry, connection)
}

fn entry(seed: u8) -> RemoteControlInterfaceEntry {
    RemoteControlInterfaceEntry {
        id: InterfaceId::new([0, 0, 0, 0, 0, 0, 0, seed]),
        kind: InterfaceKind::Loopback,
        mode: InterfaceMode::Full,
        connection: InterfaceConnectionState::Connected,
        enabled: true,
        tx_bytes: u64::from(seed),
        rx_bytes: 71,
        links: 3,
        rate_bytes_per_sec: NonZeroU32::new(128),
    }
}

fn page(
    ids: &[u8],
    continuation: RemoteControlInterfaceContinuation,
) -> RemoteControlInterfaceInventory {
    let mut page = RemoteControlInterfaceInventory::empty();
    for &id in ids {
        page.push(entry(id)).unwrap();
    }
    page.set_continuation(continuation).unwrap();
    page
}

fn receive(
    request: InterfacePageRequest,
    ids: &[u8],
    continuation: RemoteControlInterfaceContinuation,
) -> ReceiveInterfacePage {
    ReceiveInterfacePage {
        request,
        page: page(ids, continuation),
    }
}

fn refresh<const N: usize>(inventory: &mut InterfaceInventory<N>) -> InterfacePageRequest {
    let RefreshInterfacesOutcome::Requested { request } = inventory.step(RefreshInterfaces) else {
        panic!("refresh refused");
    };
    request
}

fn more(seed: u8) -> RemoteControlInterfaceContinuation {
    RemoteControlInterfaceContinuation::More(RemoteControlInterfaceCursor::after(entry(seed).id))
}

fn fail(request: InterfacePageRequest, reason: InterfaceRefreshFailure) -> InterfaceRefreshFailed {
    InterfaceRefreshFailed { request, reason }
}

fn wire_roundtrip(input: ReceiveInterfacePage) -> ReceiveInterfacePage {
    let mut bytes = [0; RemoteControlResponse::MAX_ENCODED_LEN];
    let request = input.request.request();
    let count = request.write_into(&mut bytes).unwrap();
    assert_eq!(
        RemoteControlRequest::parse(bytes.get(..count).unwrap()).unwrap(),
        request
    );
    let response = RemoteControlResponse::InventoryInterfaces(input.page);
    let count = response.write_into(&mut bytes).unwrap();
    let RemoteControlResponse::InventoryInterfaces(page) =
        RemoteControlResponse::parse(bytes.get(..count).unwrap()).unwrap()
    else {
        panic!("wrong response");
    };
    ReceiveInterfacePage {
        request: input.request,
        page,
    }
}
