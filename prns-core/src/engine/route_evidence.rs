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
        self.route_evidence_id_issuer.issue(|candidate| {
            RouteEvidenceScan::over(
                candidate,
                routing_table.route_evidence_ids().iter().copied(),
            )
        })
    }
}
