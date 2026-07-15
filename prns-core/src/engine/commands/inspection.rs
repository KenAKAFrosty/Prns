use crate::engine::EngineState;
use crate::interfaces::AttachedInterfaces;
use crate::storage::StorageLayout;

cfg_if::cfg_if! {
    if #[cfg(feature = "alloc")] {
        use crate::interfaces::InterfaceId;
        use crate::routing::routes::RouteEntry;
        use crate::routing::types::NextHop;
        use crate::routing::warmth::WarmestOf;
        use crate::units::InstantMillis;
        use crate::wire::DestinationHash;
        use alloc::vec::Vec;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectionQuery {
    LinkCount,
    #[cfg(feature = "std")]
    AnnounceRates,
    #[cfg(feature = "alloc")]
    Routes,
    #[cfg(feature = "alloc")]
    Route(DestinationHash),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectionResult {
    LinkCount(u32),
    #[cfg(feature = "std")]
    AnnounceRates(Vec<AnnounceRateSnapshot>),
    #[cfg(feature = "alloc")]
    Routes(Vec<RouteSnapshot>),
    #[cfg(feature = "alloc")]
    Route(Option<RouteSnapshot>),
}

#[cfg(feature = "alloc")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSnapshot {
    pub destination: DestinationHash,
    pub hops: u8,
    pub via: NextHop,
    pub learned_at: InstantMillis,
    pub last_relayed_at: InstantMillis,
    pub expires_at: InstantMillis,
    pub interface: InterfaceId,
}

#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceRateSnapshot {
    pub destination: DestinationHash,
    pub last_allowed_announce_at: InstantMillis,
    pub blocked_until: InstantMillis,
    pub rate_violations: u16,
    pub observed_at: Vec<InstantMillis>,
}

#[cfg(feature = "alloc")]
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
        last_relayed_at: entry.last_relayed_at,
        expires_at,
        interface: entry.receiving_interface,
    }
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn run_inspection_query(
        &self,
        query: InspectionQuery,
        interfaces: AttachedInterfaces<'_>,
    ) -> InspectionResult {
        #[cfg(not(feature = "alloc"))]
        let _ = interfaces;
        match query {
            InspectionQuery::LinkCount => InspectionResult::LinkCount(self.links.len() as u32),
            #[cfg(feature = "std")]
            InspectionQuery::AnnounceRates => InspectionResult::AnnounceRates(
                self.destination_announce_limits
                    .entries()
                    .map(|(destination, entry)| AnnounceRateSnapshot {
                        destination,
                        last_allowed_announce_at: entry.last_allowed_announce_at,
                        blocked_until: entry.blocked_until,
                        rate_violations: entry.rate_violations,
                        observed_at: entry.observations().collect(),
                    })
                    .collect(),
            ),
            #[cfg(feature = "alloc")]
            InspectionQuery::Routes => {
                let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
                InspectionResult::Routes(
                    self.routing_table
                        .path_rows_with_expiry(interfaces, &warmth)
                        .map(|(destination, entry, expires_at)| {
                            route_snapshot(destination, entry, expires_at)
                        })
                        .collect(),
                )
            }
            #[cfg(feature = "alloc")]
            InspectionQuery::Route(destination) => {
                let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
                InspectionResult::Route(
                    self.routing_table
                        .path_row_with_expiry(&destination, interfaces, &warmth)
                        .map(|(entry, expires_at)| route_snapshot(destination, entry, expires_at)),
                )
            }
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::engine::test_support::TestStorageLayout;
    use crate::interfaces::AnnounceRateLimit;

    #[test]
    fn announce_rate_inspection_projects_complete_engine_rows() {
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

        assert_eq!(
            engine.run_inspection_query(
                InspectionQuery::AnnounceRates,
                AttachedInterfaces::new(&[]),
            ),
            InspectionResult::AnnounceRates(alloc::vec![AnnounceRateSnapshot {
                destination,
                last_allowed_announce_at: InstantMillis(20),
                blocked_until: InstantMillis(0),
                rate_violations: 1,
                observed_at: alloc::vec![InstantMillis(10), InstantMillis(20)],
            }])
        );
    }
}
