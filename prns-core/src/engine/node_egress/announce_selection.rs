use crate::engine::FanTarget;
use crate::interfaces::{
    AttachedInterfaces, InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceMode,
};

pub(in crate::engine) fn allows_announce_rebroadcast(
    descriptor: &InterfaceDescriptor,
    source: InterfaceId,
    next_hop_mode: Option<InterfaceMode>,
) -> bool {
    let transport_allowed = if descriptor.id == source {
        descriptor.capabilities.allows_same_interface_repeat()
    } else {
        descriptor.capabilities.allows_transport()
    };
    transport_allowed
        && mode_allows_announce_egress(
            descriptor.mode,
            next_hop_mode,
            descriptor.common.forwarding.announces_from_internal,
        )
}

/// RNS 1.4.0 `Transport.outbound` announce mode gating.
fn mode_allows_announce_egress(
    egress: InterfaceMode,
    next_hop_mode: Option<InterfaceMode>,
    announces_from_internal: bool,
) -> bool {
    use InterfaceMode::{AccessPoint, Boundary, Full, Gateway, Internal, PointToPoint, Roaming};
    if !announces_from_internal && next_hop_mode == Some(Internal) {
        return false;
    }
    match egress {
        AccessPoint => false,
        Roaming => match next_hop_mode {
            None | Some(Roaming | Boundary) => false,
            Some(Full | PointToPoint | AccessPoint | Gateway | Internal) => true,
        },
        Boundary => match next_hop_mode {
            None | Some(Roaming) => false,
            Some(Full | PointToPoint | AccessPoint | Gateway | Boundary | Internal) => true,
        },
        Internal => !matches!(next_hop_mode, Some(Boundary)),
        Full | PointToPoint | Gateway => true,
    }
}

pub(in crate::engine) fn fleet_announce_fan_target(
    interfaces: AttachedInterfaces<'_>,
    supervisor: InterfaceKind,
    source: InterfaceId,
    directed_to: Option<InterfaceId>,
) -> FanTarget {
    if let Some(target) = directed_to {
        return FanTarget::Only(target);
    }
    if source.kind() != supervisor.member_kind() {
        return FanTarget::All;
    }
    let source_repeats = interfaces
        .iter()
        .find(|descriptor| descriptor.id == source)
        .is_some_and(|descriptor| descriptor.capabilities.allows_same_interface_repeat());
    if source_repeats {
        FanTarget::All
    } else {
        FanTarget::AllExcept(source)
    }
}

pub(in crate::engine) fn fleet_fan_target_reaches_any_member(
    interfaces: AttachedInterfaces<'_>,
    supervisor: InterfaceKind,
    fan_target: FanTarget,
) -> bool {
    let Some(member_kind) = supervisor.member_kind() else {
        return false;
    };
    interfaces
        .iter()
        .filter(|descriptor| descriptor.id.kind() == Some(member_kind))
        .any(|descriptor| match fan_target {
            FanTarget::All => true,
            FanTarget::Only(target) => descriptor.id == target,
            FanTarget::AllExcept(excluded) => descriptor.id != excluded,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{repeating_descriptor, routable_descriptor};

    const MODES: [InterfaceMode; 7] = [
        InterfaceMode::Full,
        InterfaceMode::PointToPoint,
        InterfaceMode::AccessPoint,
        InterfaceMode::Roaming,
        InterfaceMode::Boundary,
        InterfaceMode::Gateway,
        InterfaceMode::Internal,
    ];

    #[test]
    fn a_fleet_flood_to_a_lone_source_member_reaches_nobody() {
        let source = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x42, 0, 0, 0, 0, 0, 0]);
        let other = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x77, 0, 0, 0, 0, 0, 0]);

        let lone = [routable_descriptor(source)];
        assert!(
            !fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&lone),
                InterfaceKind::BluetoothAuto,
                FanTarget::AllExcept(source)
            ),
            "a flood whose fleet's only member is the source it arrived on reaches nobody"
        );

        let pair = [routable_descriptor(source), routable_descriptor(other)];
        assert!(
            fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&pair),
                InterfaceKind::BluetoothAuto,
                FanTarget::AllExcept(source)
            ),
            "with a second peer present the flood reaches it"
        );
        assert!(
            fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&lone),
                InterfaceKind::BluetoothAuto,
                FanTarget::All
            ),
            "an unconditional flood reaches the lone member"
        );
        assert!(
            fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&lone),
                InterfaceKind::BluetoothAuto,
                FanTarget::Only(source)
            ),
            "a directed target reaches its matching fleet member"
        );
        assert!(
            !fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&lone),
                InterfaceKind::BluetoothAuto,
                FanTarget::Only(other)
            ),
            "a directed target reaches nobody when that member is absent"
        );
        assert!(
            !fleet_fan_target_reaches_any_member(
                AttachedInterfaces::new(&[routable_descriptor(InterfaceId::new([0xFE; 8]))]),
                InterfaceKind::BluetoothAuto,
                FanTarget::All
            ),
            "a flood selects nobody when no member of the fleet's kind is attached"
        );
    }

    #[test]
    fn fleet_announce_targets_distinguish_directed_nonmember_and_repeating_sources() {
        let member = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x42, 0, 0, 0, 0, 0, 0]);
        let target = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x77, 0, 0, 0, 0, 0, 0]);
        let nonmember = InterfaceId::new([InterfaceKind::Loopback as u8, 0x19, 0, 0, 0, 0, 0, 0]);

        assert_eq!(
            fleet_announce_fan_target(
                AttachedInterfaces::new(&[routable_descriptor(member)]),
                InterfaceKind::BluetoothAuto,
                member,
                Some(target),
            ),
            FanTarget::Only(target),
        );
        assert_eq!(
            fleet_announce_fan_target(
                AttachedInterfaces::new(&[routable_descriptor(nonmember)]),
                InterfaceKind::BluetoothAuto,
                nonmember,
                None,
            ),
            FanTarget::All,
        );
        assert_eq!(
            fleet_announce_fan_target(
                AttachedInterfaces::new(&[routable_descriptor(member)]),
                InterfaceKind::BluetoothAuto,
                member,
                None,
            ),
            FanTarget::AllExcept(member),
        );
        assert_eq!(
            fleet_announce_fan_target(
                AttachedInterfaces::new(&[repeating_descriptor(member)]),
                InterfaceKind::BluetoothAuto,
                member,
                None,
            ),
            FanTarget::All,
        );
    }

    #[test]
    fn every_learned_on_and_egress_mode_pair_matches_rns_1_4_0() {
        let expected_by_learned_on = [
            (
                InterfaceMode::Full,
                [true, true, false, true, true, true, true],
            ),
            (
                InterfaceMode::PointToPoint,
                [true, true, false, true, true, true, true],
            ),
            (
                InterfaceMode::AccessPoint,
                [true, true, false, true, true, true, true],
            ),
            (
                InterfaceMode::Roaming,
                [true, true, false, false, false, true, true],
            ),
            (
                InterfaceMode::Boundary,
                [true, true, false, false, true, true, false],
            ),
            (
                InterfaceMode::Gateway,
                [true, true, false, true, true, true, true],
            ),
            (
                InterfaceMode::Internal,
                [true, true, false, true, true, true, true],
            ),
        ];

        for (learned_on, expected_egress) in expected_by_learned_on {
            for (egress, expected) in MODES.into_iter().zip(expected_egress) {
                assert_eq!(
                    mode_allows_announce_egress(egress, Some(learned_on), true),
                    expected,
                    "learned on {learned_on:?}, egress {egress:?}",
                );
            }
        }
    }

    #[test]
    fn announces_from_internal_only_closes_internal_sourced_egress() {
        for egress in MODES {
            assert!(!mode_allows_announce_egress(
                egress,
                Some(InterfaceMode::Internal),
                false,
            ));
            for learned_on in MODES {
                if learned_on == InterfaceMode::Internal {
                    continue;
                }
                assert_eq!(
                    mode_allows_announce_egress(egress, Some(learned_on), false),
                    mode_allows_announce_egress(egress, Some(learned_on), true),
                    "learned on {learned_on:?}, egress {egress:?}",
                );
            }
        }
    }
}
