use crate::engine::command::fan_self_originated;
use crate::engine::egress::write_path_request_wire_packet;
use crate::engine::inbound::{is_egress_eligible, Egress};
use crate::engine::reaction::LinkClosedReason;
use crate::engine::{
    Directive, EngineReaction, EngineState, EstablishLinkFailure, FanTarget, InstantMillis,
    Journaled, RequestPathFailure, SendLinkFailure, SendRequestFailure, SendSingleFailure,
    Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::InterfaceConfig;
use crate::routing::delivery::receipts::ReceiptKind;
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_REQUEST};
use crate::routing::links::table::OverdueLink;
use crate::routing::RouteResponsiveness;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::wire::{BROADCAST_MTU, TRUNCATED_HASH_BYTE_LEN};

impl<S: StorageLayout> EngineState<S> {
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
                ReceiptKind::SendRequest => {
                    Settlement::SendRequest(Err(SendRequestFailure::Timeout))
                }
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
        // A discovery forwarded on a stranger's behalf shares the 15s window; drop
        // any whose answering announce never arrived so it neither lingers nor
        // suppresses a fresh discovery for the same destination.
        self.discovery_path_requests.cull_expired(now);
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
            if let OverdueLink::Initiated {
                command_id,
                destination,
                ..
            } = overdue
            {
                self.routing_table
                    .mark_responsiveness(&destination, RouteResponsiveness::Unresponsive);
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id: command_id,
                    settlement: Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout)),
                }));
            }
        }
        let transport_id = self.transport_id;
        while let Some(overdue) = self.transported_links.pop_overdue(now) {
            if overdue.validated {
                self.mark_interface_dirty(overdue.next_hop_interface);
                self.mark_interface_dirty(overdue.received_interface);
                continue;
            }
            let has_route = self.routing_table.has_route(&overdue.destination);
            let destination_is_neighbor =
                self.routing_table.hop_count_to(&overdue.destination) == Some(1);
            let initiator_is_neighbor = overdue.taken_hops == 1;

            let fanout = if has_route && (destination_is_neighbor || initiator_is_neighbor) {
                if self
                    .recent_path_requests
                    .is_throttled(&overdue.destination, now)
                {
                    None
                } else {
                    self.routing_table.mark_responsiveness(
                        &overdue.destination,
                        RouteResponsiveness::Unresponsive,
                    );
                    Some(FanTarget::AllExcept(overdue.received_interface))
                }
            } else if !has_route {
                Some(FanTarget::All)
            } else {
                None
            };
            if let Some(fanout) = fanout {
                let mut tag = [0u8; TRUNCATED_HASH_BYTE_LEN];
                fill_entropy(&mut tag);
                let mut request = [0u8; BROADCAST_MTU];
                if let Ok(wire_len) = write_path_request_wire_packet(
                    overdue.destination,
                    transport_id,
                    &tag,
                    &mut request,
                ) {
                    fan_self_originated(view, fanout, &request[..wire_len], sink);
                    self.recent_path_requests
                        .mark_seen_at(overdue.destination, now);
                }
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
        let dirty = &mut self.dirty_interfaces;
        self.routing_table
            .cull_expired_routes(now, view, &mut |removed| {
                dirty.mark(removed.receiving_interface);
                sink(EngineReaction::Journaled(
                    crate::engine::inbound::journal_removal(removed),
                ));
            });
        self.reverse_routes
            .cull_interface_orphans(|id| view.iter().any(|config| config.id == id));
        let dirty = &mut self.dirty_interfaces;
        self.transported_links.cull_interface_orphans(
            |id| view.iter().any(|config| config.id == id),
            &mut |iface| dirty.mark(iface),
        );
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

        let source = InterfaceId::new([0u8; 8]);
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

        let without_source = [routable_descriptor(InterfaceId::new([0xEE; 8]))];
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

    #[cfg(feature = "tokio-host")]
    #[test]
    fn a_dropped_route_marks_its_interface_so_the_destination_count_recomputes() {
        use crate::engine::test_support::{hx, routable_descriptor, RAW_ANNOUNCE, TEST_ENTROPY};
        use crate::interfaces::{InboundPacket, InterfaceId};

        let source = InterfaceId::new([0u8; 8]);
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

        let mut on_insert = std::vec::Vec::new();
        engine.drain_dirty_interfaces(|interface| on_insert.push(interface));
        assert_eq!(
            on_insert,
            std::vec![source],
            "learning a route marks the interface it arrived on",
        );
        assert_eq!(engine.interface_counts(source).destinations, 1);

        let without_source = [routable_descriptor(InterfaceId::new([0xEE; 8]))];
        engine.cull_expired_routes(InstantMillis(2_000), &without_source, &mut |_| {});

        let mut on_cull = std::vec::Vec::new();
        engine.drain_dirty_interfaces(|interface| on_cull.push(interface));
        assert_eq!(
            on_cull,
            std::vec![source],
            "dropping the route re-marks the interface, so the stale count never lingers silently",
        );
        assert_eq!(engine.interface_counts(source).destinations, 0);
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

    #[test]
    fn an_unproved_transported_link_to_a_neighbor_marks_the_route_unresponsive() {
        use crate::engine::test_support::{hx, routable_descriptor, RAW_ANNOUNCE, TEST_ENTROPY};
        use crate::interfaces::{InboundPacket, InterfaceId};
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let source = InterfaceId::new([0xA1; 8]);
        let view = [routable_descriptor(source)];
        let mut engine = EngineState::<Cap>::default();
        let mut raw = hx(RAW_ANNOUNCE);
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &view,
        );
        let destination =
            DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap());
        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, &view)
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unknown,
            "a freshly learned route is unconfirmed",
        );

        engine
            .transported_links
            .track(TransportedLink {
                link_id: LinkId::new([0x5C; 16]),
                destination,
                next_hop: None,
                next_hop_interface: source,
                received_interface: source,
                taken_hops: 1,
                remaining_hops: 1,
                validated: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            &view,
            &mut |bytes: &mut [u8]| bytes.fill(0),
            &mut |_| {},
        );

        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, &view)
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unresponsive,
            "the neighbor link never proved, so its route is marked unresponsive",
        );
    }

    #[test]
    fn an_unproved_neighbor_link_fires_a_path_request_away_from_the_received_lane() {
        use crate::engine::egress::PATH_REQUEST_DESTINATION;
        use crate::engine::test_support::{hx, routable_descriptor, RAW_ANNOUNCE, TEST_ENTROPY};
        use crate::interfaces::{InboundPacket, InterfaceId};
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;
        use crate::wire::{DestinationType, WirePacketHeader};

        let received = InterfaceId::new([0xA1; 8]);
        let away = InterfaceId::new([0xB2; 8]);
        let view = [routable_descriptor(received), routable_descriptor(away)];

        let mut engine = EngineState::<Cap>::default();
        let mut raw = hx(RAW_ANNOUNCE);
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: received,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &view,
        );
        let destination =
            DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap());

        engine
            .transported_links
            .track(TransportedLink {
                link_id: LinkId::new([0x5C; 16]),
                destination,
                next_hop: None,
                next_hop_interface: away,
                received_interface: received,
                taken_hops: 1,
                remaining_hops: 1,
                validated: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let mut sent = std::vec::Vec::new();
        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            &view,
            &mut |bytes: &mut [u8]| bytes.fill(0x5A),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { target, bytes }) = reaction {
                    sent.push((target, bytes.to_vec()));
                }
            },
        );

        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, &view)
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unresponsive,
        );
        assert_eq!(
            sent.len(),
            1,
            "the request fires on the one lane that wasn't the dead link's",
        );
        assert_eq!(
            sent[0].0, away,
            "never back out the interface the failed link arrived on",
        );
        let (header, payload) = WirePacketHeader::parse(&sent[0].1).unwrap();
        assert_eq!(header.destination, PATH_REQUEST_DESTINATION);
        assert_eq!(header.destination_type, DestinationType::Plain);
        assert_eq!(
            &payload[..16],
            destination.as_bytes(),
            "and it asks for the destination whose link just died",
        );
    }

    #[test]
    fn an_unproved_link_recovers_when_the_initiator_is_the_neighbor_too() {
        use crate::engine::test_support::{hx, routable_descriptor, RAW_ANNOUNCE, TEST_ENTROPY};
        use crate::interfaces::{InboundPacket, InterfaceId};
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let received = InterfaceId::new([0xA1; 8]);
        let away = InterfaceId::new([0xB2; 8]);
        let view = [routable_descriptor(received), routable_descriptor(away)];

        let mut engine = EngineState::<Cap>::default();
        let mut raw = hx(RAW_ANNOUNCE);
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: received,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &view,
        );
        let destination =
            DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap());

        engine
            .transported_links
            .track(TransportedLink {
                link_id: LinkId::new([0x5C; 16]),
                destination,
                next_hop: None,
                next_hop_interface: away,
                received_interface: received,
                taken_hops: 1,
                remaining_hops: 4,
                validated: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let mut sent = std::vec::Vec::new();
        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            &view,
            &mut |bytes: &mut [u8]| bytes.fill(0x5A),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                    sent.push(target);
                }
            },
        );

        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, &view)
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unresponsive,
            "a far destination still recovers when its link initiator is our neighbor",
        );
        assert_eq!(sent, std::vec![away]);
    }

    #[test]
    fn a_recently_requested_destination_holds_off_the_overdue_links_path_request() {
        use crate::engine::test_support::{hx, routable_descriptor, RAW_ANNOUNCE, TEST_ENTROPY};
        use crate::interfaces::{InboundPacket, InterfaceId};
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let received = InterfaceId::new([0xA1; 8]);
        let away = InterfaceId::new([0xB2; 8]);
        let view = [routable_descriptor(received), routable_descriptor(away)];

        let mut engine = EngineState::<Cap>::default();
        let mut raw = hx(RAW_ANNOUNCE);
        let _ = engine.ingest_packet(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: received,
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &view,
        );
        let destination =
            DestinationHash::new(hx("16f8a6d3f7d7c5b6f106d293804d7314").try_into().unwrap());

        engine
            .transported_links
            .track(TransportedLink {
                link_id: LinkId::new([0x5C; 16]),
                destination,
                next_hop: None,
                next_hop_interface: away,
                received_interface: received,
                taken_hops: 1,
                remaining_hops: 1,
                validated: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let asked_well_within_the_throttle_window = InstantMillis(2_000);
        engine
            .recent_path_requests
            .mark_seen_at(destination, asked_well_within_the_throttle_window);

        let mut sent = std::vec::Vec::new();
        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            &view,
            &mut |bytes: &mut [u8]| bytes.fill(0x5A),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                    sent.push(target);
                }
            },
        );

        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, &view)
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unknown,
            "the throttle holds off the unresponsive mark too, not only the resend",
        );
        assert!(
            sent.is_empty(),
            "a path request inside the minimum interval suppresses the re-request",
        );
    }
}
