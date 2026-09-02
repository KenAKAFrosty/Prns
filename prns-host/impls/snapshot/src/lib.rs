#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use personal_rns::interfaces::{ConnectionState, Membership};
use personal_rns::node_introspection::{
    logical_interface_inventory, EngineInspectionSnapshot, InterfaceInventoryEntry,
};
use personal_rns::routing::routes::NextHop;
use prns_host::{
    BackendInfo, DestinationIdentitySnapshot, HostSnapshot, IdentityHash, InterfaceHealth,
    InterfaceKind, InterfaceSnapshot, PersistenceSnapshot, RouteSnapshot, RuntimeHealthSnapshot,
};

/// Attachment metadata needed to project one runtime interface into a Host snapshot.
///
/// `interface` may identify either an independent interface, a fleet supervisor, or one of its
/// raw members. [`assemble_host_snapshot`] resolves fleet members to the logical supervisor before
/// it joins this metadata to the folded inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostInterfaceAttachment {
    interface: personal_rns::interfaces::InterfaceId,
    kind: InterfaceKind,
}

impl HostInterfaceAttachment {
    #[must_use]
    pub const fn new(
        interface: personal_rns::interfaces::InterfaceId,
        kind: InterfaceKind,
    ) -> Self {
        Self { interface, kind }
    }

    #[must_use]
    pub const fn interface(&self) -> personal_rns::interfaces::InterfaceId {
        self.interface
    }

    #[must_use]
    pub const fn kind(&self) -> InterfaceKind {
        self.kind
    }
}

/// Assemble the canonical Host snapshot from one capture of the runtime inspection state.
///
/// The raw inventory is folded exactly once. Fleet membership is retained first so both attachment
/// metadata and engine routes can be remapped to the logical supervisor after the fold consumes
/// the membership markers. `backend` is supplied by the actual owner, allowing a narrowly composed
/// host to report narrower capabilities than the full native Host implementation.
#[must_use]
pub fn assemble_host_snapshot(
    raw_interfaces: Vec<InterfaceInventoryEntry>,
    attachments: impl IntoIterator<Item = HostInterfaceAttachment>,
    engine: EngineInspectionSnapshot,
    backend: BackendInfo,
    persistence: PersistenceSnapshot,
    revision: u64,
    uptime: Duration,
) -> HostSnapshot {
    let member_to_supervisor = raw_interfaces
        .iter()
        .filter_map(|entry| match entry.snapshot.membership {
            Membership::Independent => None,
            Membership::FleetMember { supervisor_id } => Some((entry.snapshot.id, supervisor_id)),
        })
        .collect::<BTreeMap<_, _>>();
    let mut attached_kinds = BTreeMap::new();
    for attachment in attachments {
        let logical_interface = member_to_supervisor
            .get(&attachment.interface)
            .copied()
            .unwrap_or(attachment.interface);
        attached_kinds
            .entry(logical_interface)
            .or_insert(attachment.kind);
    }

    let interfaces = logical_interface_inventory(raw_interfaces)
        .into_iter()
        .filter_map(|entry| {
            let kind = attached_kinds.get(&entry.snapshot.id).copied()?;
            let rates = entry.snapshot.transfer_rates;
            Some(InterfaceSnapshot {
                interface_id: host_interface(entry.snapshot.id),
                name: entry.name,
                kind: Some(kind),
                health: host_interface_health(entry.snapshot.connection),
                failure_detail: entry.snapshot.failure_reason.map(str::to_string),
                rx_bytes: entry.snapshot.rx_bytes,
                tx_bytes: entry.snapshot.tx_bytes,
                rx_bps: rates.map(|rates| u64::from(rates.rx_bps)),
                tx_bps: rates.map(|rates| u64::from(rates.tx_bps)),
                route_count: entry.snapshot.destinations,
                link_count: entry.snapshot.links,
                transported_link_count: entry.snapshot.transported_links,
            })
        })
        .collect::<Vec<_>>();

    let EngineInspectionSnapshot {
        link_count,
        routes: engine_routes,
        destination_identities: engine_destination_identities,
    } = engine;
    let routes = engine_routes
        .into_iter()
        .map(|route| RouteSnapshot {
            destination: host_destination(route.destination),
            hops: route.hops,
            via_identity: match route.via {
                NextHop::Direct => None,
                NextHop::Via(identity) => Some(IdentityHash::new(*identity.as_bytes())),
            },
            interface_id: host_interface(
                member_to_supervisor
                    .get(&route.interface)
                    .copied()
                    .unwrap_or(route.interface),
            ),
            learned_at_millis: route.learned_at.0,
            last_route_activity_at_millis: route.last_route_activity_at.0,
            expires_at_millis: route.expires_at.0,
        })
        .collect::<Vec<_>>();
    let destination_identities = engine_destination_identities
        .into_iter()
        .map(|association| DestinationIdentitySnapshot {
            destination: host_destination(association.destination),
            identity: IdentityHash::new(*association.identity.as_bytes()),
        })
        .collect::<Vec<_>>();

    let runtime = RuntimeHealthSnapshot {
        running: true,
        uptime_millis: u64::try_from(uptime.as_millis()).unwrap_or(u64::MAX),
        interface_count: u32::try_from(interfaces.len()).unwrap_or(u32::MAX),
        online_interface_count: u32::try_from(
            interfaces
                .iter()
                .filter(|interface| {
                    matches!(
                        interface.health,
                        InterfaceHealth::Connected | InterfaceHealth::Degraded
                    )
                })
                .count(),
        )
        .unwrap_or(u32::MAX),
        route_count: u32::try_from(routes.len()).unwrap_or(u32::MAX),
        link_count,
        transported_link_count: interfaces.iter().fold(0u32, |sum, interface| {
            sum.saturating_add(interface.transported_link_count)
        }),
        rx_bytes: interfaces.iter().fold(0u64, |sum, interface| {
            sum.saturating_add(interface.rx_bytes)
        }),
        tx_bytes: interfaces.iter().fold(0u64, |sum, interface| {
            sum.saturating_add(interface.tx_bytes)
        }),
        rx_bps: interfaces.iter().fold(0u64, |sum, interface| {
            sum.saturating_add(interface.rx_bps.unwrap_or_default())
        }),
        tx_bps: interfaces.iter().fold(0u64, |sum, interface| {
            sum.saturating_add(interface.tx_bps.unwrap_or_default())
        }),
    };

    HostSnapshot {
        revision,
        backend,
        interfaces,
        routes,
        active_link_count: link_count,
        destination_identities,
        runtime,
        persistence,
    }
}

fn host_interface_health(health: ConnectionState) -> InterfaceHealth {
    match health {
        ConnectionState::Initializing => InterfaceHealth::Initializing,
        ConnectionState::Connected => InterfaceHealth::Connected,
        ConnectionState::Degraded => InterfaceHealth::Degraded,
        ConnectionState::Reconnecting => InterfaceHealth::Reconnecting,
        ConnectionState::Failed => InterfaceHealth::Failed,
        ConnectionState::Disconnected => InterfaceHealth::Disconnected,
        ConnectionState::Disabled => InterfaceHealth::Disabled,
        ConnectionState::Unknown => InterfaceHealth::Unknown,
    }
}

fn host_destination(value: personal_rns::wire::DestinationHash) -> prns_host::DestinationHash {
    prns_host::DestinationHash::new(*value.as_bytes())
}

fn host_interface(value: personal_rns::interfaces::InterfaceId) -> prns_host::InterfaceId {
    prns_host::InterfaceId::new(*value.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::identity::PublicIdentityMaterial;
    use personal_rns::interfaces::{
        FrameAccounting, InterfaceGravity, InterfaceMode, InterfaceOriginKind,
        InterfaceSnapshot as EngineInterfaceSnapshot, TransferRates,
    };
    use personal_rns::node_introspection::{
        DestinationIdentitySnapshot as EngineDestinationIdentitySnapshot, FrameAccountingCoverage,
    };
    use personal_rns::routing::routes::RouteRetention;
    use personal_rns::units::InstantMillis;
    use personal_rns::wire::{DestinationHash as EngineDestinationHash, TransportId};
    use prns_host::{BackendKind, Capability, DestinationHash, InterfaceId};

    fn inventory_entry(
        name: &'static str,
        snapshot: EngineInterfaceSnapshot,
    ) -> InterfaceInventoryEntry {
        InterfaceInventoryEntry {
            name: Some(name.to_string()),
            origin: InterfaceOriginKind::Configured,
            attachment_epoch: 1,
            frame_accounting: FrameAccountingCoverage::Complete(FrameAccounting::default()),
            snapshot,
            ifac: None,
        }
    }

    #[test]
    fn folds_fleet_inventory_once_and_remaps_routes_to_the_supervisor() {
        let supervisor = personal_rns::interfaces::InterfaceId::new([0x10; 8]);
        let member_a = personal_rns::interfaces::InterfaceId::new([0x21; 8]);
        let member_b = personal_rns::interfaces::InterfaceId::new([0x22; 8]);
        let destination = EngineDestinationHash::new([0x31; 16]);
        let next_hop = TransportId::new([0x41; 16]);
        let associated_identity = personal_rns::identity::IdentityHash::new([0x42; 16]);
        let raw_interfaces = vec![
            inventory_entry(
                "member-a",
                EngineInterfaceSnapshot {
                    id: member_a,
                    mode: InterfaceMode::Full,
                    gravity: InterfaceGravity::ZERO,
                    connection: ConnectionState::Connected,
                    failure_reason: None,
                    rx_bytes: u64::MAX - 5,
                    tx_bytes: 7,
                    transfer_rates: Some(TransferRates {
                        rx_bps: u32::MAX,
                        tx_bps: 8,
                    }),
                    destinations: u32::MAX,
                    links: 3,
                    transported_links: u32::MAX,
                    membership: Membership::FleetMember {
                        supervisor_id: supervisor,
                    },
                },
            ),
            inventory_entry(
                "member-b",
                EngineInterfaceSnapshot {
                    id: member_b,
                    mode: InterfaceMode::Full,
                    gravity: InterfaceGravity::ZERO,
                    connection: ConnectionState::Failed,
                    failure_reason: Some("radio unavailable"),
                    rx_bytes: 10,
                    tx_bytes: u64::MAX,
                    transfer_rates: Some(TransferRates {
                        rx_bps: 1,
                        tx_bps: u32::MAX,
                    }),
                    destinations: 1,
                    links: u32::MAX,
                    transported_links: 1,
                    membership: Membership::FleetMember {
                        supervisor_id: supervisor,
                    },
                },
            ),
        ];
        let engine = EngineInspectionSnapshot {
            link_count: 9,
            routes: vec![personal_rns::engine::RouteSnapshot {
                destination,
                hops: 4,
                via: NextHop::Via(next_hop),
                learned_at: InstantMillis(11),
                last_route_activity_at: InstantMillis(12),
                expires_at: InstantMillis(13),
                interface: member_b,
                retention: RouteRetention::Network,
            }],
            destination_identities: vec![EngineDestinationIdentitySnapshot {
                destination,
                identity: associated_identity,
                public: PublicIdentityMaterial::from_bytes([0x51; 64]),
            }],
        };
        let backend = BackendInfo::new(
            BackendKind::Native,
            [Capability::Bluetooth],
            [InterfaceKind::AutomaticBluetoothLe],
        );
        let persistence = PersistenceSnapshot::persistent();

        let snapshot = assemble_host_snapshot(
            raw_interfaces,
            [HostInterfaceAttachment::new(
                member_a,
                InterfaceKind::AutomaticBluetoothLe,
            )],
            engine,
            backend.clone(),
            persistence.clone(),
            7,
            Duration::new(u64::MAX, 0),
        );

        assert_eq!(snapshot.revision, 7);
        assert_eq!(snapshot.backend, backend);
        assert_eq!(snapshot.persistence, persistence);
        assert_eq!(snapshot.interfaces.len(), 1);
        let interface = &snapshot.interfaces[0];
        assert_eq!(interface.interface_id, InterfaceId::new([0x10; 8]));
        assert_eq!(interface.name.as_deref(), Some("member-a"));
        assert_eq!(interface.kind, Some(InterfaceKind::AutomaticBluetoothLe));
        assert_eq!(interface.health, InterfaceHealth::Connected);
        assert_eq!(
            interface.failure_detail.as_deref(),
            Some("radio unavailable")
        );
        assert_eq!(interface.rx_bytes, u64::MAX);
        assert_eq!(interface.tx_bytes, u64::MAX);
        assert_eq!(interface.rx_bps, Some(u64::from(u32::MAX)));
        assert_eq!(interface.tx_bps, Some(u64::from(u32::MAX)));
        assert_eq!(interface.route_count, u32::MAX);
        assert_eq!(interface.link_count, u32::MAX);
        assert_eq!(interface.transported_link_count, u32::MAX);

        assert_eq!(snapshot.routes.len(), 1);
        let route = &snapshot.routes[0];
        assert_eq!(route.destination, DestinationHash::new([0x31; 16]));
        assert_eq!(route.hops, 4);
        assert_eq!(route.via_identity, Some(IdentityHash::new([0x41; 16])));
        assert_eq!(route.interface_id, InterfaceId::new([0x10; 8]));
        assert_eq!(route.learned_at_millis, 11);
        assert_eq!(route.last_route_activity_at_millis, 12);
        assert_eq!(route.expires_at_millis, 13);

        assert_eq!(snapshot.active_link_count, 9);
        assert_eq!(snapshot.destination_identities.len(), 1);
        assert_eq!(
            snapshot.destination_identities[0],
            DestinationIdentitySnapshot {
                destination: DestinationHash::new([0x31; 16]),
                identity: IdentityHash::new([0x42; 16]),
            }
        );
        assert_eq!(snapshot.runtime.uptime_millis, u64::MAX);
        assert_eq!(snapshot.runtime.interface_count, 1);
        assert_eq!(snapshot.runtime.online_interface_count, 1);
        assert_eq!(snapshot.runtime.route_count, 1);
        assert_eq!(snapshot.runtime.link_count, 9);
        assert_eq!(snapshot.runtime.transported_link_count, u32::MAX);
        assert_eq!(snapshot.runtime.rx_bytes, u64::MAX);
        assert_eq!(snapshot.runtime.tx_bytes, u64::MAX);
        assert_eq!(snapshot.runtime.rx_bps, u64::from(u32::MAX));
        assert_eq!(snapshot.runtime.tx_bps, u64::from(u32::MAX));
    }
}
