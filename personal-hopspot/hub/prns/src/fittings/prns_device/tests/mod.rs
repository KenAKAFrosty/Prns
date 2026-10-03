#![allow(clippy::unwrap_used, clippy::panic)]

mod native;
mod properties;
mod transitions;

use super::*;
use crate::{PrnsDevice, PrnsDeviceIn, PrnsDeviceOut};
use alloc::sync::Arc;
use core::num::NonZeroU32;
use hopspot_hub_core::*;
use personal_rns::crypto::{Ed25519PublicKey, X25519PublicKey};
use personal_rns::engine::{EstablishLinkFailure, IdentifyFailure};
use personal_rns::identity::{
    IdentityEncryptionPublicKey, IdentityHash, IdentityPublicKeys, IdentitySigningPublicKey,
};
use personal_rns::interfaces::InterfaceId;
use personal_rns::remote_control::*;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::*;
use personal_rns::units::RttMillis;
use personal_rns::wire::DestinationHash;
use pipecircuit::{DuplexFitting, ReceiveFromOutcome, StateMachine, TransportFrom, TransportTo};
use std::sync::Mutex;
use tokio::sync::Notify;

const LINK: LinkId = LinkId::new([41; 16]);

fn target(seed: u8) -> RemoteControlTargetIdentity {
    RemoteControlTargetIdentity::new(IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([seed; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([seed; 32])),
    })
}

fn paired(registry: &mut DeviceRegistry, target: RemoteControlTargetIdentity) -> DeviceId {
    let CreateDeviceOutcome::Created { device } = registry
        .step(CreateDevice {
            label: DeviceLabel::new("MCU").unwrap(),
        })
        .unwrap()
    else {
        panic!("create refused")
    };
    let BeginEnrollmentOutcome::Started { enrollment } = registry
        .step(BeginEnrollment {
            device,
            target: RemoteControlTargetIdentity::new(*target.public_keys()),
            attempt: RemoteControlPairingAttemptId::from_test_transcript_digest_bytes([1; 32]),
        })
        .unwrap()
    else {
        panic!("pair refused")
    };
    assert_eq!(
        registry.step(CompleteEnrollment { enrollment, target }),
        CompleteEnrollmentOutcome::Recorded { enrollment }
    );
    device
}

fn begin(registry: &mut DeviceRegistry, device: DeviceId) -> Connection {
    let BeginConnectionOutcome::Connect { connection } =
        registry.step(BeginConnection { device }).unwrap()
    else {
        panic!("begin refused")
    };
    connection
}

fn fixture() -> (DeviceRegistry, Connection, Mock) {
    let mut registry = DeviceRegistry::try_new(NonZeroU32::new(3).unwrap()).unwrap();
    let device = paired(&mut registry, target(31));
    let connection = begin(&mut registry, device);
    (registry, connection, Mock::new(connection.target()))
}

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Resolve(IdentityHash),
    Establish(DestinationHash),
    Identify(LinkId, IdentityHash),
    Inventory(LinkId, RemoteControlInterfacePage),
    Close(LinkId),
}

#[derive(Clone, Copy)]
enum Failure {
    None,
    Resolve,
    Establish,
    Identify,
    Inventory,
    Panic,
}

struct Shared {
    calls: Mutex<alloc::vec::Vec<Call>>,
    identified: Notify,
    release: Notify,
    closed: Notify,
}

struct Mock {
    target: RemoteControlTargetIdentity,
    failure: Failure,
    permitted: RemoteControlRequestSet,
    settlement: CloseRemoteControlTargetOutcome,
    block: bool,
    shared: Arc<Shared>,
}

impl Mock {
    fn new(target: RemoteControlTargetIdentity) -> Self {
        Self {
            target,
            failure: Failure::None,
            permitted: RemoteControlRequestSet::only(RemoteControlRequestKind::InventoryInterfaces),
            settlement: CloseRemoteControlTargetOutcome::Queued,
            block: false,
            shared: Arc::new(Shared {
                calls: Mutex::new(alloc::vec::Vec::new()),
                identified: Notify::new(),
                release: Notify::new(),
                closed: Notify::new(),
            }),
        }
    }

    fn record(&self, call: Call) {
        self.shared.calls.lock().unwrap().push(call);
    }
}

fn controller() -> RemoteControlControllerIdentity {
    RemoteControlControllerIdentity::new(*target(21).public_keys())
}

impl RemoteControlTargetAccessControl for Mock {
    async fn remote_control_target_inventory(
        &self,
    ) -> Result<RemoteControlTargetInventory, RemoteControlTargetInventoryControlError> {
        panic!("unexpected inventory")
    }
    async fn set_remote_control_target_access(
        &self,
        _: RemoteControlTargetAccess,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SetRemoteControlTargetAccessControlError> {
        panic!("unexpected authorization")
    }
    async fn forget_remote_control_target(
        &self,
        _: IdentityHash,
    ) -> Result<ForgetRemoteControlTargetOutcome, ForgetRemoteControlTargetControlError> {
        panic!("unexpected forgetting")
    }
    async fn resolve_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> Result<ResolvedRemoteControlTarget, ResolveRemoteControlTargetControlError> {
        self.record(Call::Resolve(target));
        if matches!(self.failure, Failure::Panic) {
            panic!("worker failed")
        }
        if matches!(self.failure, Failure::Resolve) {
            return Err(ResolveRemoteControlTargetControlError::TargetNotAuthorized);
        }
        let access = RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(*self.target.public_keys()),
            RemoteControlControllerAuthority::Operator,
            self.permitted,
        )
        .unwrap();
        Ok(ResolvedRemoteControlTarget::from((&controller(), &access)))
    }
}

impl RemoteControlTargetConnectionTransport for Mock {
    async fn establish_remote_control_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        self.record(Call::Establish(destination));
        if matches!(self.failure, Failure::Establish) {
            return Err(SendError::Busy);
        }
        Ok(LINK)
    }
    async fn identify_remote_control_link(
        &self,
        link: LinkId,
        identity: IdentityHash,
    ) -> Result<(), SendError<IdentifyFailure>> {
        self.record(Call::Identify(link, identity));
        self.shared.identified.notify_one();
        if self.block {
            self.shared.release.notified().await;
        }
        if matches!(self.failure, Failure::Identify) {
            return Err(SendError::NodeStopped);
        }
        Ok(())
    }
    fn close_remote_control_link(&self, link: LinkId) -> CloseRemoteControlTargetOutcome {
        self.record(Call::Close(link));
        self.shared.closed.notify_one();
        self.settlement
    }
}

impl PrnsInventoryTransport for Mock {
    async fn inventory_interfaces(
        &self,
        link: LinkId,
        page: RemoteControlInterfacePage,
    ) -> Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlError> {
        self.record(Call::Inventory(link, page));
        if matches!(self.failure, Failure::Inventory) {
            return Err(RemoteControlError::Request(SendError::NodeStopped));
        }
        let mut inventory = RemoteControlInterfaceInventory::empty();
        let id = match page {
            RemoteControlInterfacePage::First => 1,
            RemoteControlInterfacePage::After(_) => 2,
        };
        inventory
            .push(RemoteControlInterfaceEntry {
                id: InterfaceId::new([id; 8]),
                kind: personal_rns::interfaces::InterfaceKind::Loopback,
                mode: personal_rns::interfaces::InterfaceMode::Full,
                connection: personal_rns::interfaces::ConnectionState::Connected,
                enabled: true,
                tx_bytes: 17,
                rx_bytes: 23,
                links: 1,
                rate_bytes_per_sec: None,
            })
            .unwrap();
        if matches!(page, RemoteControlInterfacePage::First) {
            inventory
                .set_continuation(RemoteControlInterfaceContinuation::More(
                    RemoteControlInterfaceCursor::after(InterfaceId::new([1; 8])),
                ))
                .unwrap();
        }
        Ok((inventory, RttMillis::new(42)))
    }
}

async fn perform<B: PrnsInventoryTransport>(
    fitting: &mut PrnsDeviceFitting<B>,
    input: PrnsDeviceIn,
) -> PrnsDeviceOut {
    let (mut incoming, mut outgoing) =
        <PrnsDeviceFitting<B> as DuplexFitting<PrnsDevice>>::split(fitting);
    let result = outgoing.send(input).complete().await;
    let ReceiveFromOutcome::Received { output } = incoming.receive_from(result) else {
        panic!("fitting invariant failed")
    };
    output
}

async fn connect<B: PrnsInventoryTransport>(
    fitting: &mut PrnsDeviceFitting<B>,
    connection: Connection,
) -> ConfirmConnection {
    let PrnsDeviceOut::Connected { confirmation } =
        perform(fitting, PrnsDeviceIn::Connect { connection }).await
    else {
        panic!("connect refused")
    };
    assert_eq!(confirmation.connection, connection);
    assert_eq!(confirmation.target, connection.target());
    confirmation
}
