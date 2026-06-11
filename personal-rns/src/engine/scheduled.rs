use crate::engine::{
    EngineReaction, EngineState, EstablishLinkFailure, InstantMillis, Journaled,
    RequestPathFailure, SendSingleFailure, Settlement, WakeSchedules,
};
use crate::interfaces::InterfaceConfig;
use crate::routing::links::table::OverdueLink;
use crate::routing::storage::EngineStorage;

impl<S: EngineStorage> EngineState<S> {
    /// Settle every send-single whose proof deadline has passed: each gives up and closes
    /// `SendSingle(Timeout)`. Returns the send-timeout lane's new soonest deadline.
    pub fn settle_timed_out_send_singles(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        while let Some(expired) = self.pop_timed_out_send_single(now) {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: expired.command_id,
                settlement: Settlement::SendSingle(Err(SendSingleFailure::Timeout)),
            }));
        }
        WakeSchedules {
            send_single_timeout: self.send_single_receipts_timeout_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    /// Settle every path request whose answer never arrived in time: each closes
    /// `RequestPath(Timeout)`. Returns the path-timeout lane's new soonest deadline.
    pub fn settle_timed_out_path_requests(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        while let Some(expired) = self.pop_timed_out_path_request(now) {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: expired.command_id,
                settlement: Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
            }));
        }
        WakeSchedules {
            path_request_timeout: self.path_request_timeout_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    pub fn settle_timed_out_link_establishments(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        while let Some(overdue) = self.pop_timed_out_link(now) {
            if let OverdueLink::Initiated { command_id, .. } = overdue {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id: command_id,
                    settlement: Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout)),
                }));
            }
        }
        WakeSchedules {
            link_establishment_timeout: self.link_establishment_timeout_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    /// Cull every route past its expiry — the reactor's timer edge for the
    /// expired-routes lane, the same removal the at-capacity insert runs inline.
    /// Each removal journals its cause — `RouteExpired` for the aged,
    /// `RouteInterfaceGone` for the orphaned — the reference's two cull arms
    /// (Transport.py:778-785). Returns the lane's new soonest expiry.
    pub fn cull_expired_routes(
        &mut self,
        now: InstantMillis,
        view: &[InterfaceConfig],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        self.routing_table
            .cull_expired_routes(now, view, &mut |removed| {
                sink(EngineReaction::Journaled(
                    crate::engine::inbound::journal_removal(removed),
                ));
            });
        WakeSchedules {
            expired_routes: self.route_expiry_wake(view),
            ..WakeSchedules::UNCHANGED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::Cap;
    use crate::engine::{
        CommandId, PathRequestId, PathRequestWriteOutcome, RequestPath, PATH_REQUEST_TIMEOUT_MS,
    };
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    #[test]
    fn the_cull_journals_an_orphan_as_route_interface_gone() {
        use crate::engine::test_support::{hx, routable_descriptor, RAW_ANNOUNCE, TEST_ENTROPY};
        use crate::interfaces::{InboundPacket, InterfaceId};
        use crate::wire::DestinationHash;

        let source = InterfaceId::new([0u8; 16]);
        let mut engine = EngineState::<Cap>::default();
        let mut raw = hx(RAW_ANNOUNCE);
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &[routable_descriptor(source)],
        );
        assert_eq!(engine.route_count(), 1);

        let without_source = [routable_descriptor(InterfaceId::new([0xEE; 16]))];
        let mut journal = std::vec::Vec::new();
        let delta =
            engine.cull_expired_routes(InstantMillis(2_000), &without_source, &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RouteInterfaceGone { destination }) =
                    reaction
                {
                    journal.push(destination);
                }
            });
        assert_eq!(
            journal,
            std::vec![DestinationHash::new(
                hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap()
            )],
            "the orphan's removal names its cause",
        );
        assert_eq!(engine.route_count(), 0);
        assert_eq!(
            delta.expired_routes,
            crate::engine::LaneWake::Idle,
            "nothing is left to wake for",
        );
    }

    #[test]
    fn settle_timed_out_path_requests_closes_each_expired_request_once_past_its_deadline() {
        let mut engine = EngineState::<Cap>::default();
        let issued_at = InstantMillis(1_000);
        let mut buf = [0u8; BROADCAST_MTU];
        let outcome = engine.write_commanded_path_request(
            CommandId(9),
            &RequestPath {
                destination: DestinationHash::new([0x44; 16]),
                id: PathRequestId::new([0x55; 16]),
            },
            issued_at,
            &mut buf,
        );
        assert!(matches!(outcome, PathRequestWriteOutcome::Written { .. }));

        let mut settled: std::vec::Vec<(CommandId, Settlement)> = std::vec::Vec::new();

        engine.settle_timed_out_path_requests(issued_at, &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settled.push((id, settlement));
            }
        });
        assert!(settled.is_empty(), "before the deadline, nothing settles");

        engine.settle_timed_out_path_requests(
            InstantMillis(issued_at.0 + PATH_REQUEST_TIMEOUT_MS + 1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        assert_eq!(
            settled,
            std::vec![(
                CommandId(9),
                Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
            )],
            "past the deadline the request settles Timeout, exactly once",
        );
    }
}
