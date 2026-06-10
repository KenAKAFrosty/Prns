use crate::engine::{
    EngineReaction, EngineState, InstantMillis, Journaled, RequestPathFailure, SendSingleFailure,
    Settlement, WakeSchedules,
};
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
            send_single_timeout: self.send_timeout_lane(),
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
            path_request_timeout: self.path_timeout_lane(),
            ..WakeSchedules::UNCHANGED
        }
    }

    /// Cull every route past its expiry — the reactor's timer edge for the
    /// expired-routes lane, the same removal the at-capacity insert runs inline.
    /// Each removal is journaled `RouteExpired`, the reference's own cull log
    /// (Transport.py:781). Returns the lane's new soonest expiry.
    pub fn cull_expired_routes(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        self.routing_table
            .cull_expired_routes(now, &mut |destination| {
                sink(EngineReaction::Journaled(Journaled::RouteExpired {
                    destination,
                }));
            });
        WakeSchedules {
            expired_routes: self.route_expiry_lane(),
            ..WakeSchedules::UNCHANGED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{
        hx, transporting_view, Cap, RATCHETED_ANNOUNCE_RNS_WIRE, RAW_ANNOUNCE, TEST_ENTROPY,
    };
    use crate::engine::{
        CommandId, PathRequestId, PathRequestWriteOutcome, RequestPath, PATH_REQUEST_TIMEOUT_MS,
    };
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::announce::defaults::DEFAULT_ROUTE_EXPIRY_MILLIS;
    use crate::routing::storage::FixedInline;
    use crate::wire::{DestinationHash, MTU};

    #[test]
    fn settle_timed_out_path_requests_closes_each_expired_request_once_past_its_deadline() {
        let mut engine = EngineState::<Cap>::default();
        let issued_at = InstantMillis(1_000);
        let mut buf = [0u8; MTU];
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

    #[test]
    fn a_cull_frees_the_arena_and_a_held_announce_finally_recovers() {
        type TinyArena = FixedInline<4, 8, 16, 4, 32, 4, 4, 4, 32, 4, 4, 4, 4, 8>;
        let mut engine = EngineState::<TinyArena>::default();
        let source_interface = InterfaceId::new([0u8; 16]);

        let mut first = hx(RAW_ANNOUNCE);
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface,
                bytes: &mut first,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(
            engine.route_count(),
            1,
            "the first announce fills the arena"
        );

        let mut second = hx(RATCHETED_ANNOUNCE_RNS_WIRE);
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(2_000),
                source_interface,
                bytes: &mut second,
            },
            TEST_ENTROPY,
            &transporting_view(),
        );
        assert_eq!(engine.route_count(), 1);
        assert_eq!(
            engine.held_announce_count(),
            1,
            "the second announce parks on arena pressure",
        );

        let mut expired = std::vec::Vec::new();
        let _ = engine.cull_expired_routes(
            InstantMillis(1_000 + DEFAULT_ROUTE_EXPIRY_MILLIS),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RouteExpired { destination }) = reaction
                {
                    expired.push(destination);
                }
            },
        );
        assert_eq!(engine.route_count(), 0, "the expired occupant is culled");
        assert_eq!(
            expired,
            std::vec![DestinationHash::new(
                hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap()
            )],
            "the cull journals the removed route",
        );

        let mut recovered = std::vec::Vec::new();
        let _ =
            engine.recover_held_announces(TEST_ENTROPY, &transporting_view(), &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::AnnounceHeard { destination, .. }) =
                    reaction
                {
                    recovered.push(destination);
                }
            });
        assert_eq!(
            recovered,
            std::vec![DestinationHash::new(
                hx("c3cfae69b36bb6e3bbfd96a3b5867a59").try_into().unwrap()
            )],
            "the cull freed the arena, so the held announce lands and journals its hearing",
        );
        assert_eq!(engine.route_count(), 1);
        assert_eq!(engine.held_announce_count(), 0);
    }
}
