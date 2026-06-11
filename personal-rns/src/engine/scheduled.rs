use crate::engine::inbound::{is_egress_eligible, Egress};
use crate::engine::reaction::LinkClosedReason;
use crate::engine::{
    Directive, EngineReaction, EngineState, EstablishLinkFailure, InstantMillis, Journaled,
    RequestPathFailure, SendLinkFailure, SendSingleFailure, Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::InterfaceConfig;
use crate::routing::delivery::receipts::ReceiptKind;
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_REQUEST};
use crate::routing::links::table::OverdueLink;
use crate::routing::storage::EngineStorage;
use crate::wire::BROADCAST_MTU;

impl<S: EngineStorage> EngineState<S> {
    /// Settle every tracked send whose proof deadline has passed: each gives up
    /// and closes its own kind's `Timeout`. Returns the receipt-timeout lane's
    /// new soonest deadline.
    pub fn settle_timed_out_receipts(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        while let Some(expired) = self.pop_timed_out_receipt(now) {
            let settlement = match expired.kind {
                ReceiptKind::SendSingle => Settlement::SendSingle(Err(SendSingleFailure::Timeout)),
                ReceiptKind::SendLink => Settlement::SendLink(Err(SendLinkFailure::Timeout)),
            };
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: expired.command_id,
                settlement,
            }));
        }
        WakeSchedules {
            receipt_timeouts: self.receipt_timeouts_wake(),
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

    pub fn fire_due_link_deadlines<F>(
        &mut self,
        now: InstantMillis,
        view: &[InterfaceConfig],
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        while let Some(overdue) = self.pop_timed_out_link(now) {
            if let OverdueLink::Initiated { command_id, .. } = overdue {
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id: command_id,
                    settlement: Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout)),
                }));
            }
        }
        while let Some(link_id) = self.links.pop_stale(now) {
            let mut iv = [0u8; ENCRYPTION_IV_LEN];
            fill_entropy(&mut iv);
            let mut buf = [0u8; BROADCAST_MTU];
            if let Ok(dispatch) = self.write_owed_link_close(&link_id, &iv, &mut buf) {
                if let Some(target) = dispatch.fire_on {
                    if is_egress_eligible(view, target, Egress::Transmit) {
                        sink(EngineReaction::Directive(Directive::Send {
                            target,
                            bytes: &buf[..dispatch.wire_len],
                        }));
                    }
                }
                sink(EngineReaction::Journaled(Journaled::LinkClosed {
                    link_id,
                    reason: LinkClosedReason::Timeout,
                }));
            }
        }
        while let Some(due) = self.links.pop_due_keepalive(now) {
            if is_egress_eligible(view, due.attached_interface, Egress::Transmit) {
                let mut buf = [0u8; BROADCAST_MTU];
                if let Ok(written) = write_keepalive(&due.link_id, KEEPALIVE_REQUEST, &mut buf) {
                    sink(EngineReaction::Directive(Directive::Send {
                        target: due.attached_interface,
                        bytes: &buf[..written],
                    }));
                }
            }
        }
        WakeSchedules {
            link_deadlines: self.link_deadlines_wake(),
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
