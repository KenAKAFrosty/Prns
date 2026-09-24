use personal_rns::interfaces::*;
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use personal_rns::runtime::{Fleet, InterfaceSupervisor};
use std::future::pending;
use std::sync::Arc;

fn policy() -> EffectiveInterfacePolicy {
    EffectiveInterfacePolicy {
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Disabled,
            egress: EgressCapability::Disabled,
        },
        mode: InterfaceMode::Full,
        gravity: InterfaceGravity::ZERO,
        bitrate: BitrateBps::guess(1_000_000),
        mtu: MtuPolicy::fixed(personal_rns::wire::BROADCAST_MTU),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: InterfaceCommonPolicy::RNS_DEFAULT,
    }
}

pub struct IdleInterface {
    pub tag: Vec<u8>,
}
impl ReportsStatus for IdleInterface {
    fn status_view(&self) -> Option<StatusView> {
        let id = self.descriptor().id;
        Some(Arc::new(move || {
            vec![vitals(id, RadioIndication::NotRadio)]
        }))
    }
}
impl Interface for IdleInterface {
    const HW_MTU: usize = personal_rns::wire::BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Loopback;
    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }
    fn descriptor(&self) -> InterfaceDescriptor {
        policy().descriptor(InterfaceId::from_channel_tag(Self::KIND, &self.tag))
    }
    async fn run<S: InterfaceSeam>(self, _: S) {
        pending::<()>().await;
    }
}

pub struct PeerFleet {
    pub members: usize,
}
impl PeerFleet {
    pub const TAG: &[u8] = b"remote-control-inventory-fleet";
    pub fn id() -> InterfaceId {
        InterfaceId::from_channel_tag(Self::KIND, Self::TAG)
    }
}
impl ReportsStatus for PeerFleet {
    fn status_view(&self) -> Option<StatusView> {
        Some(Arc::new(|| {
            vec![InterfaceVitals {
                id: Self::id(),
                connection: ConnectionState::Connected,
                failure_reason: None,
                rx_bytes: 0,
                tx_bytes: 0,
                transfer_rates: None,
                frame_accounting: None,
                radio: RadioIndication::HaLow(WifiIndication::Unavailable),
                details: PeerDetails::Unknown,
            }]
        }))
    }
}
impl InterfaceSupervisor for PeerFleet {
    const KIND: InterfaceKind = InterfaceKind::WifiHaLow;
    fn channel_tag(&self) -> &[u8] {
        Self::TAG
    }
    fn policy(&self) -> EffectiveInterfacePolicy {
        policy()
    }
    async fn run(self, fleet: Fleet) {
        for member in 0..self.members {
            fleet.add(IdlePeer {
                tag: member.to_be_bytes().to_vec(),
            });
        }
        pending::<()>().await;
    }
}
struct IdlePeer {
    tag: Vec<u8>,
}
impl ReportsStatus for IdlePeer {
    fn status_view(&self) -> Option<StatusView> {
        let id = self.descriptor().id;
        Some(Arc::new(move || {
            vec![vitals(
                id,
                RadioIndication::HaLow(WifiIndication::Unavailable),
            )]
        }))
    }
}
impl Interface for IdlePeer {
    const HW_MTU: usize = personal_rns::wire::BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::WifiHaLowPeer;
    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }
    fn descriptor(&self) -> InterfaceDescriptor {
        policy().descriptor(InterfaceId::from_channel_tag(Self::KIND, &self.tag))
    }
    async fn run<S: InterfaceSeam>(self, _: S) {
        pending::<()>().await;
    }
}

fn vitals(id: InterfaceId, radio: RadioIndication) -> InterfaceVitals {
    InterfaceVitals {
        id,
        connection: ConnectionState::Disabled,
        failure_reason: None,
        rx_bytes: 0,
        tx_bytes: 0,
        transfer_rates: None,
        frame_accounting: None,
        radio,
        details: PeerDetails::Unknown,
    }
}

pub struct ConfiguredInterface {
    pub tag: Vec<u8>,
    pub bitrate: BitrateBps,
    pub connection: std::sync::Arc<std::sync::Mutex<ConnectionState>>,
}
impl ReportsStatus for ConfiguredInterface {
    fn status_view(&self) -> Option<StatusView> {
        let id = self.descriptor().id;
        let connection = self.connection.clone();
        Some(Arc::new(move || {
            let mut status = vitals(id, RadioIndication::NotRadio);
            status.connection = *connection.lock().expect("scenario connection");
            vec![status]
        }))
    }
}
impl Interface for ConfiguredInterface {
    const HW_MTU: usize = personal_rns::wire::BROADCAST_MTU;
    const KIND: InterfaceKind = InterfaceKind::Loopback;
    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }
    fn descriptor(&self) -> InterfaceDescriptor {
        EffectiveInterfacePolicy {
            bitrate: self.bitrate,
            ..policy()
        }
        .descriptor(InterfaceId::from_channel_tag(Self::KIND, &self.tag))
    }
    async fn run<S: InterfaceSeam>(self, _: S) {
        pending::<()>().await;
    }
}
