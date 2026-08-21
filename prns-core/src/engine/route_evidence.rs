use crate::engine::EngineState;
use crate::interfaces::InterfaceId;
use crate::routing::routes::{RouteEvidenceId, RouteEvidenceScan};
use crate::routing::NextHop;
use crate::storage::StorageLayout;
use crate::wire::DestinationHash;

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn route_evidence_id_for_update(
        &mut self,
        destination: &DestinationHash,
        receiving_interface: InterfaceId,
        next_hop: NextHop,
    ) -> RouteEvidenceId {
        if let (Some(row), Some(handle)) = (
            self.routing_table.path_row(destination),
            self.routing_table.route_evidence_handle_for(destination),
        ) {
            if row.receiving_interface == receiving_interface && row.next_hop == next_hop {
                return handle.id;
            }
        }
        self.mint_route_evidence_id()
    }

    fn mint_route_evidence_id(&mut self) -> RouteEvidenceId {
        let routing_table = &self.routing_table;
        let links = &self.links;
        let transported_links = &self.transported_links;
        self.route_evidence_id_issuer.issue(|candidate| {
            RouteEvidenceScan::over(
                candidate,
                routing_table
                    .route_evidence_ids()
                    .iter()
                    .copied()
                    .chain(links.route_evidence_ids())
                    .chain(transported_links.route_evidence_ids()),
            )
        })
    }

    /// Promotes at most one observation per initiated Link, regardless of how many authenticated
    /// frames arrived since the previous engine boundary. Returns the count of changed items.
    pub(crate) fn reconcile_pending_link_route_evidence(&mut self) -> usize {
        let routing_table = &mut self.routing_table;
        let mut changed = 0;
        self.links
            .reconcile_pending_route_evidence(|handle, observed_at| {
                if routing_table.apply_route_evidence(handle, observed_at) {
                    changed += 1;
                }
            });
        changed
    }
}
