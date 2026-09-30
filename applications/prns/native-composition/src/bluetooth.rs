//! Application preference and presentation over the existing public Bluetooth supervisor.

use personal_rns::interfaces::{
    BluetoothIndication, ConnectionState, InterfaceKind, Membership, PeerDetails, RadioIndication,
};
use personal_rns::node_introspection::InterfaceInventoryEntry;

use crate::contract::{LocalBluetoothPeerSnapshot, LocalBluetoothSnapshot, LocalBluetoothState};

pub(crate) struct BluetoothControl {
    set_enabled: Box<dyn Fn(bool) + Send + Sync>,
    radio_state: Box<dyn Fn() -> Option<LocalBluetoothState> + Send + Sync>,
    is_disabled: Box<dyn Fn() -> bool + Send + Sync>,
}

impl BluetoothControl {
    #[cfg(any(
        all(feature = "apple", any(target_os = "ios", target_os = "macos")),
        all(feature = "android", target_os = "android")
    ))]
    pub(crate) fn new(status: personal_rns::bluetooth_auto::BluetoothAutoStatus) -> Self {
        let read_status = status.clone();
        let disabled_status = status.clone();
        Self {
            set_enabled: Box::new(move |enabled| {
                if enabled {
                    status.enable();
                } else {
                    status.disable();
                }
            }),
            radio_state: Box::new(move || {
                use personal_rns::interfaces::bluetooth_auto::BluetoothRadioState;
                match read_status.radio_state() {
                    BluetoothRadioState::PoweredOn => None,
                    BluetoothRadioState::PoweredOff => Some(LocalBluetoothState::RadioOff),
                    // Android's existing platform status owns OS radio/permission facts;
                    // the UI combines those with this authoritative supervisor state.
                    BluetoothRadioState::Unknown if cfg!(target_os = "android") => None,
                    BluetoothRadioState::Unknown | BluetoothRadioState::Resetting => {
                        Some(LocalBluetoothState::Starting)
                    }
                    BluetoothRadioState::Unsupported => Some(LocalBluetoothState::Unavailable {
                        detail: "Bluetooth is not supported by this device.".into(),
                    }),
                    BluetoothRadioState::Unauthorized => Some(LocalBluetoothState::Unavailable {
                        detail: "Bluetooth permission is unavailable.".into(),
                    }),
                }
            }),
            is_disabled: Box::new(move || disabled_status.is_disabled()),
        }
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        (self.set_enabled)(enabled);
    }

    pub(crate) fn project(
        &self,
        interfaces: &[InterfaceInventoryEntry],
        desired: Option<bool>,
    ) -> LocalBluetoothSnapshot {
        project(
            interfaces,
            desired,
            (self.radio_state)(),
            (self.is_disabled)(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test(set_enabled: impl Fn(bool) + Send + Sync + 'static) -> Self {
        Self {
            set_enabled: Box::new(set_enabled),
            radio_state: Box::new(|| None),
            is_disabled: Box::new(|| false),
        }
    }
}

fn project(
    interfaces: &[InterfaceInventoryEntry],
    desired: Option<bool>,
    radio: Option<LocalBluetoothState>,
    disabled: bool,
) -> LocalBluetoothSnapshot {
    let supervisors: Vec<_> = interfaces
        .iter()
        .filter(|entry| entry.snapshot.id.kind() == Some(InterfaceKind::BluetoothAuto))
        .collect();
    let mut peers: Vec<_> = interfaces
        .iter()
        .filter(|entry| {
            let Membership::FleetMember { supervisor_id } = entry.snapshot.membership else {
                return false;
            };
            entry.snapshot.id.kind() == Some(InterfaceKind::BluetoothPeer)
                && supervisors
                    .iter()
                    .any(|parent| parent.snapshot.id == supervisor_id)
        })
        .map(|entry| {
            let peer = &entry.snapshot;
            LocalBluetoothPeerSnapshot {
                interface_id: peer.id.as_bytes().to_vec(),
                name: entry.name.clone(),
                connected: peer.connection.is_online(),
                rx_bytes: peer.rx_bytes,
                tx_bytes: peer.tx_bytes,
                details: match peer.details {
                    PeerDetails::NotApplicable | PeerDetails::Unknown => None,
                    value => Some(value.to_string()),
                },
                rssi_dbm: match peer.radio {
                    RadioIndication::Bluetooth(BluetoothIndication::Rssi(rssi)) => Some(rssi.get()),
                    _ => None,
                },
            }
        })
        .collect();
    peers.sort_by(|left, right| left.interface_id.cmp(&right.interface_id));
    let state = if desired == Some(false) {
        if disabled && peers.is_empty() {
            LocalBluetoothState::Disabled
        } else {
            LocalBluetoothState::Disabling
        }
    } else if supervisors.is_empty() {
        LocalBluetoothState::Unavailable {
            detail: "This native build has no local Bluetooth interface.".into(),
        }
    } else if let Some(failed) = supervisors
        .iter()
        .find(|entry| entry.snapshot.connection == ConnectionState::Failed)
    {
        match radio {
            Some(
                state @ (LocalBluetoothState::RadioOff | LocalBluetoothState::Unavailable { .. }),
            ) => state,
            _ => LocalBluetoothState::Unavailable {
                detail: failed
                    .snapshot
                    .failure_reason
                    .unwrap_or("The Bluetooth interface failed.")
                    .into(),
            },
        }
    } else if let Some(radio) = radio {
        radio
    } else if peers.iter().any(|peer| peer.connected) {
        LocalBluetoothState::Connected
    } else if supervisors
        .iter()
        .any(|entry| entry.snapshot.connection == ConnectionState::Reconnecting)
    {
        LocalBluetoothState::Connecting
    } else if supervisors.iter().all(|entry| {
        matches!(
            entry.snapshot.connection,
            ConnectionState::Disconnected | ConnectionState::Degraded | ConnectionState::Connected
        )
    }) {
        LocalBluetoothState::WaitingForPeers
    } else {
        LocalBluetoothState::Starting
    };
    LocalBluetoothSnapshot {
        desired_enabled: desired,
        state,
        peers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::{
        InterfaceGravity, InterfaceId, InterfaceMode, InterfaceOriginKind, InterfaceSnapshot,
    };
    use personal_rns::node_introspection::FrameAccountingCoverage;

    fn project(
        interfaces: &[InterfaceInventoryEntry],
        desired: Option<bool>,
        radio: Option<LocalBluetoothState>,
    ) -> LocalBluetoothSnapshot {
        super::project(interfaces, desired, radio, true)
    }

    fn row(id: InterfaceId, membership: Membership) -> InterfaceInventoryEntry {
        InterfaceInventoryEntry {
            name: None,
            origin: InterfaceOriginKind::Configured,
            attachment_epoch: 1,
            frame_accounting: FrameAccountingCoverage::Unavailable,
            ifac: None,
            snapshot: InterfaceSnapshot {
                id,
                membership,
                mode: InterfaceMode::Full,
                gravity: InterfaceGravity::ZERO,
                connection: ConnectionState::Connected,
                failure_reason: None,
                rx_bytes: 17,
                tx_bytes: 23,
                transfer_rates: None,
                destinations: 90,
                links: 40,
                transported_links: 30,
                radio: RadioIndication::from_bluetooth_rssi(Some(-60)),
                details: PeerDetails::BleGatt,
            },
        }
    }

    #[test]
    fn physical_members_are_independent_of_routes_and_rns_links() {
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, b"auto");
        let peer = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, b"peer");
        let foreign = InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, b"other");
        let tcp = InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, b"tcp");
        let snapshot = project(
            &[
                row(supervisor, Membership::Independent),
                row(
                    peer,
                    Membership::FleetMember {
                        supervisor_id: supervisor,
                    },
                ),
                row(
                    InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, b"unrelated"),
                    Membership::FleetMember {
                        supervisor_id: foreign,
                    },
                ),
                row(
                    tcp,
                    Membership::FleetMember {
                        supervisor_id: supervisor,
                    },
                ),
            ],
            Some(true),
            None,
        );
        assert_eq!(snapshot.peers.len(), 1);
        assert_eq!(snapshot.state, LocalBluetoothState::Connected);
        assert_eq!(snapshot.peers[0].interface_id, peer.as_bytes());
        assert_eq!(snapshot.peers[0].rx_bytes, 17);
        assert_eq!(snapshot.peers[0].details.as_deref(), Some("GATT"));
        assert_eq!(snapshot.peers[0].rssi_dbm, Some(-60));
    }

    #[test]
    fn radio_and_disable_progress_do_not_fabricate_connected_peers() {
        let supervisor = InterfaceId::from_channel_tag(InterfaceKind::BluetoothAuto, b"auto");
        let peer = InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, b"peer");
        let mut parent = row(supervisor, Membership::Independent);
        parent.snapshot.connection = ConnectionState::Disconnected;
        assert_eq!(
            super::project(&[parent.clone()], Some(false), None, false).state,
            LocalBluetoothState::Disabling
        );
        assert_eq!(
            project(&[parent.clone()], Some(true), None).state,
            LocalBluetoothState::WaitingForPeers
        );
        assert_eq!(
            project(
                &[parent.clone()],
                Some(true),
                Some(LocalBluetoothState::RadioOff)
            )
            .state,
            LocalBluetoothState::RadioOff
        );
        assert_eq!(
            project(
                &[parent.clone()],
                Some(true),
                Some(LocalBluetoothState::Starting)
            )
            .state,
            LocalBluetoothState::Starting
        );
        let connected = row(
            peer,
            Membership::FleetMember {
                supervisor_id: supervisor,
            },
        );
        assert_eq!(
            project(
                &[parent.clone(), connected.clone()],
                Some(true),
                Some(LocalBluetoothState::RadioOff)
            )
            .state,
            LocalBluetoothState::RadioOff
        );
        let mut failed = parent.clone();
        failed.snapshot.connection = ConnectionState::Failed;
        failed.snapshot.failure_reason = Some("Backend unavailable");
        assert_eq!(
            project(&[failed], Some(true), Some(LocalBluetoothState::Starting)).state,
            LocalBluetoothState::Unavailable {
                detail: "Backend unavailable".into()
            }
        );
        assert_eq!(
            project(&[parent.clone(), connected], Some(false), None).state,
            LocalBluetoothState::Disabling
        );
        let disabled = project(&[parent], Some(false), None);
        assert_eq!(disabled.state, LocalBluetoothState::Disabled);
        assert!(disabled.peers.is_empty());
    }
}
