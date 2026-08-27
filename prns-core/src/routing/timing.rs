//! Passive, bitrate-aware timing policy shared by routing, links and shared-instance RPC.

use crate::interfaces::{
    AttachedInterfaces, BitrateBps, InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceMode,
};
use crate::wire::BROADCAST_MTU;

/// RNS' fixed processing allowance for each routed hop.
pub const DEFAULT_PER_HOP_TIMEOUT_SECONDS: u32 = 6;
pub const DEFAULT_PER_HOP_TIMEOUT_MS: u64 = DEFAULT_PER_HOP_TIMEOUT_SECONDS as u64 * 1_000;
/// The fallback when no selected-interface bitrate is available.
pub const DEFAULT_FIRST_HOP_TIMEOUT_MS: u64 = DEFAULT_PER_HOP_TIMEOUT_MS;
/// The ordinary path-request lifetime, retained as the adaptive floor.
pub const PATH_REQUEST_TIMEOUT_FLOOR_MS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRequestAudience {
    Network,
    BoundaryAndGateway,
    LocalClients,
}

#[derive(Debug, Clone, Copy)]
pub struct FirstHopTiming<'a> {
    /// Interfaces attached to the engine at the point the route is prepared.
    pub interfaces: AttachedInterfaces<'a>,
    /// Optional physical-route estimate supplied by a shared-instance daemon.
    pub shared_instance_floor_ms: Option<u64>,
}

#[must_use]
pub fn path_request_egress_eligible(
    descriptor: &InterfaceDescriptor,
    ingress_id: Option<InterfaceId>,
    audience: PathRequestAudience,
) -> bool {
    if !descriptor.capabilities.allows_transmit() || ingress_id == Some(descriptor.id) {
        return false;
    }
    match audience {
        PathRequestAudience::Network => !matches!(
            descriptor.id.kind(),
            Some(InterfaceKind::LocalClient | InterfaceKind::LocalServer)
        ),
        PathRequestAudience::BoundaryAndGateway => {
            !matches!(
                descriptor.id.kind(),
                Some(InterfaceKind::LocalClient | InterfaceKind::LocalServer)
            ) && matches!(
                descriptor.mode,
                InterfaceMode::Boundary | InterfaceMode::Gateway
            )
        }
        PathRequestAudience::LocalClients => {
            descriptor.id.kind() == Some(InterfaceKind::LocalClient)
        }
    }
}

/// Ceiling-rounded airtime in milliseconds. Intermediate `u128` arithmetic keeps even
/// pathological byte counts saturating instead of wrapping.
#[must_use]
pub fn frame_airtime_ms(bytes: usize, bitrate: BitrateBps) -> u64 {
    let numerator = (bytes as u128).saturating_mul(8).saturating_mul(1_000);
    let denominator = u128::from(bitrate.get());
    let milliseconds = numerator.saturating_add(denominator.saturating_sub(1)) / denominator;
    milliseconds.min(u128::from(u64::MAX)) as u64
}

#[must_use]
pub fn broadcast_airtime_ms(bitrate: BitrateBps) -> u64 {
    frame_airtime_ms(BROADCAST_MTU, bitrate)
}

#[must_use]
pub fn first_hop_timeout_ms(bitrate: Option<BitrateBps>) -> u64 {
    DEFAULT_FIRST_HOP_TIMEOUT_MS.saturating_add(bitrate.map_or(0, broadcast_airtime_ms))
}

#[must_use]
pub fn single_packet_timeout_ms(hops: u8, bitrate: Option<BitrateBps>) -> u64 {
    first_hop_timeout_ms(bitrate)
        .saturating_add(DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(hops)))
}

#[must_use]
pub fn link_establishment_timeout_ms(hops: u8, bitrate: Option<BitrateBps>) -> u64 {
    first_hop_timeout_ms(bitrate)
        .saturating_add(DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(hops.max(1))))
}

/// The RNS 1.5 `medium_path_timeout` value before callers apply their ordinary 15s floor.
/// `None` deliberately maps to zero for shared-RPC compatibility.
#[must_use]
pub fn medium_path_timeout_ms(slowest: Option<BitrateBps>) -> u64 {
    slowest.map_or(0, |bitrate| {
        DEFAULT_FIRST_HOP_TIMEOUT_MS.saturating_add(broadcast_airtime_ms(bitrate).saturating_mul(2))
    })
}

#[must_use]
pub fn path_discovery_timeout_ms(slowest: Option<BitrateBps>) -> u64 {
    PATH_REQUEST_TIMEOUT_FLOOR_MS.max(medium_path_timeout_ms(slowest))
}

/// Finds the slowest interface in the exact caller-defined discovery audience.
#[must_use]
pub fn slowest_eligible_bitrate(
    interfaces: AttachedInterfaces<'_>,
    mut eligible: impl FnMut(&InterfaceDescriptor) -> bool,
) -> Option<BitrateBps> {
    interfaces
        .iter()
        .filter(|descriptor| descriptor.capabilities.allows_transmit() && eligible(descriptor))
        .map(|descriptor| descriptor.bitrate)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_vectors_are_ceiling_rounded() {
        let cases = [
            (5, 800_000, 806_000, 1_606_000),
            (250, 16_000, 22_000, 38_000),
            (500, 8_000, 14_000, 22_000),
            (1_000, 4_000, 10_000, 14_000),
            (10_000, 400, 6_400, 6_800),
        ];
        for (bps, airtime, first_hop, medium) in cases {
            let bitrate = BitrateBps::guess(bps);
            assert_eq!(broadcast_airtime_ms(bitrate), airtime);
            assert_eq!(first_hop_timeout_ms(Some(bitrate)), first_hop);
            assert_eq!(medium_path_timeout_ms(Some(bitrate)), medium);
        }
        assert_eq!(broadcast_airtime_ms(BitrateBps::guess(500_000_000)), 1);
    }

    #[test]
    fn discovery_keeps_its_floor_and_missing_bitrate_rpc_semantics() {
        assert_eq!(medium_path_timeout_ms(None), 0);
        assert_eq!(path_discovery_timeout_ms(None), 15_000);
        assert_eq!(
            path_discovery_timeout_ms(Some(BitrateBps::guess(10_000))),
            15_000,
        );
        assert_eq!(
            path_discovery_timeout_ms(Some(BitrateBps::guess(500))),
            22_000,
        );
    }

    #[test]
    fn arithmetic_saturates() {
        assert_eq!(frame_airtime_ms(usize::MAX, BitrateBps::guess(5)), u64::MAX);
        assert_eq!(first_hop_timeout_ms(Some(BitrateBps::guess(5))), 806_000,);
    }

    #[test]
    fn path_request_audiences_exclude_ingress_and_local_only_interfaces() {
        use crate::engine::test_support::routable_descriptor;

        let ingress = InterfaceId::new([0xA1; 8]);
        let network = InterfaceId::new([0xB2; 8]);
        let local = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"app");
        assert!(!path_request_egress_eligible(
            &routable_descriptor(ingress),
            Some(ingress),
            PathRequestAudience::Network,
        ));
        assert!(path_request_egress_eligible(
            &routable_descriptor(network),
            Some(ingress),
            PathRequestAudience::Network,
        ));
        assert!(!path_request_egress_eligible(
            &routable_descriptor(local),
            Some(ingress),
            PathRequestAudience::Network,
        ));
        assert!(path_request_egress_eligible(
            &routable_descriptor(local),
            Some(ingress),
            PathRequestAudience::LocalClients,
        ));
    }
}
