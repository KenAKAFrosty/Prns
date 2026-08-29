use crate::engine::EngineState;
use crate::identity::IdentityHash;
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::links::LinkId;
use crate::routing::routes::{NextHop, RouteEntry, RouteRetention};
use crate::routing::warmth::WarmestOf;
use crate::storage::StorageLayout;
use crate::units::{InstantMillis, RttMillis};
use crate::wire::DestinationHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceRateState {
    pub destination: DestinationHash,
    pub last_allowed_announce_at: InstantMillis,
    pub blocked_until: InstantMillis,
    pub rate_violations: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSnapshot {
    pub destination: DestinationHash,
    pub hops: u8,
    pub via: NextHop,
    pub learned_at: InstantMillis,
    pub last_route_activity_at: InstantMillis,
    pub expires_at: InstantMillis,
    pub interface: InterfaceId,
    pub retention: RouteRetention,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLinkSnapshot {
    pub link_id: LinkId,
    pub rtt: RttMillis,
    pub remote_identity: Option<IdentityHash>,
}

fn route_snapshot(
    destination: DestinationHash,
    entry: RouteEntry,
    expires_at: InstantMillis,
) -> RouteSnapshot {
    RouteSnapshot {
        destination,
        hops: entry.hops,
        via: entry.next_hop,
        learned_at: entry.learned_at,
        last_route_activity_at: entry.last_route_activity_at,
        expires_at,
        interface: entry.receiving_interface,
        retention: entry.retention,
    }
}

impl<S: StorageLayout> EngineState<S> {
    #[must_use]
    pub fn link_count(&self) -> u32 {
        u32::try_from(self.links.active_link_count()).unwrap_or(u32::MAX)
    }

    pub fn visit_announce_rate_states(&self, mut visit: impl FnMut(AnnounceRateState)) {
        for (destination, entry) in self.destination_announce_limits.entries() {
            visit(AnnounceRateState {
                destination,
                last_allowed_announce_at: entry.last_allowed_announce_at,
                blocked_until: entry.blocked_until,
                rate_violations: entry.rate_violations,
            });
        }
    }

    pub fn visit_active_link_snapshots(&self, mut visit: impl FnMut(ActiveLinkSnapshot)) {
        self.links
            .visit_active_links(|link_id, rtt, remote_identity| {
                visit(ActiveLinkSnapshot {
                    link_id,
                    rtt,
                    remote_identity,
                });
            });
    }

    pub fn visit_route_snapshots(
        &self,
        interfaces: AttachedInterfaces<'_>,
        mut visit: impl FnMut(RouteSnapshot),
    ) {
        let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
        for (destination, entry, expires_at) in self
            .routing_table
            .path_rows_with_expiry(interfaces, &warmth)
        {
            visit(route_snapshot(destination, entry, expires_at));
        }
    }

    #[must_use]
    pub fn route_snapshot(
        &self,
        destination: DestinationHash,
        interfaces: AttachedInterfaces<'_>,
    ) -> Option<RouteSnapshot> {
        let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
        self.routing_table
            .path_row_with_expiry(&destination, interfaces, &warmth)
            .map(|(entry, expires_at)| route_snapshot(destination, entry, expires_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::TestStorageLayout;
    use crate::interfaces::AnnounceRateLimit;

    #[test]
    fn announce_rate_introspection_projects_engine_state() {
        let mut engine = EngineState::<TestStorageLayout>::default();
        let destination = DestinationHash::new([0x42; 16]);
        let limit = AnnounceRateLimit {
            target_ms: 100,
            grace: 3,
            penalty_ms: 1_000,
        };
        engine
            .destination_announce_limits
            .observe(destination, InstantMillis(10), limit);
        engine
            .destination_announce_limits
            .observe(destination, InstantMillis(20), limit);
        let mut inspected = None;

        engine.visit_announce_rate_states(|state| inspected = Some(state));

        assert_eq!(
            inspected,
            Some(AnnounceRateState {
                destination,
                last_allowed_announce_at: InstantMillis(20),
                blocked_until: InstantMillis(0),
                rate_violations: 1,
            })
        );
    }
}
