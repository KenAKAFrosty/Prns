use crate::engine::egress::{
    firable_on, fleet_announce_fan_target, fleet_fan_target_reaches_any_member,
};
use crate::engine::execute::{fan_frame, settle, timeout_settlement};
#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{
    write_path_request_wire_packet, Directive, EngineReaction, EngineState, EstablishLinkFailure,
    FanTarget, InstantMillis, Journaled, LinkClosedReason, ReemitAnnounce, RequestPathFailure,
    Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{AttachedInterfaces, Egress};
use crate::interfaces::{InterfaceKind, InterfaceMode};
use crate::routing::announce::defaults::{MAX_OUR_EMISSIONS, REBROADCAST_RETRANSMIT_INTERVAL_MS};
use crate::routing::announce::schedule::ScheduledAnnounceQueue as _;
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_REQUEST};
use crate::routing::links::table::OverdueLink;
use crate::routing::warmth::WarmestOf;
use crate::routing::RouteResponsiveness;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::wire::{BROADCAST_MTU, TRUNCATED_HASH_BYTE_LEN};

impl<S: StorageLayout> EngineState<S> {
    pub fn settle_timed_out_receipts(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        while let Some(expired) = self.receipts.pop_expired(now) {
            settle(sink, expired.command_id, timeout_settlement(expired.kind));
        }
        WakeSchedules {
            receipt_timeouts: self.receipt_timeouts_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    pub fn settle_timed_out_path_requests(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        while let Some(expired) = self.pop_timed_out_path_request(now) {
            settle(
                sink,
                expired.command_id,
                Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
            );
        }

        self.recursive_path_requests.cull_expired(now);
        WakeSchedules {
            path_request_timeouts: self.path_request_timeouts_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    /// The reference's two cull arms (RNS 1.3.5 `Transport.jobs`): [`RouteRemovalCause::Expired`](crate::routing::RouteRemovalCause::Expired) for the aged, [`RouteRemovalCause::InterfaceGone`](crate::routing::RouteRemovalCause::InterfaceGone) for the orphaned.
    /// The orphan arm is softened by the [`crate::routing::warmth::DepartedInterfaces`] grace; the reverse-route and transported-link culls below stay eager like the reference's, since they carry in-flight work that a bounced lane kills regardless.
    pub fn cull_expired_routes(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let tunnels_changed = self.tunnels.expire(now) != 0;
        let departures_changed = self.departed_interfaces.evict_expired(now) != 0;
        if tunnels_changed || departures_changed {
            self.routing_table.invalidate_route_expiries();
        }
        let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
        let dirty = &mut self.dirty_interfaces;
        self.routing_table.cull_expired_routes_indexed_with_warmth(
            now,
            interfaces,
            &warmth,
            &mut |removed| {
                dirty.mark(removed.receiving_interface);
                sink(EngineReaction::Journaled(
                    crate::engine::inbound::journal_route_removal(removed),
                ));
            },
        );

        self.reverse_routes
            .cull_interface_orphans(|id| interfaces.iter().any(|descriptor| descriptor.id == id));

        let dirty = &mut self.dirty_interfaces;
        self.transported_links.cull_interface_orphans(
            |id| interfaces.iter().any(|descriptor| descriptor.id == id),
            &mut |iface| dirty.mark(iface),
        );
        WakeSchedules {
            expired_routes: self.route_expiry_wake(interfaces),
            expired_destination_identities: self.destination_identity_expiry_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    pub fn fire_due_scheduled_announces(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        if let Some(via) = self.transport_id() {
            let scheduled = &self.scheduled_announces;
            let routing = &self.routing_table;
            for entry in scheduled.iter().filter(|s| s.due_at.0 <= now.0) {
                let Some(stored) = routing.stored_announce_for(&entry.destination) else {
                    continue;
                };
                let source = entry.source_interface;
                let directed_to = entry.directed_to;
                let crosses_local_boundary = source.kind() == Some(InterfaceKind::LocalClient)
                    && directed_to
                        .is_none_or(|target| target.kind() != Some(InterfaceKind::LocalClient));
                let emit_hops = self
                    .protocol
                    .local_hop_count_override
                    .apply(stored.hops, crosses_local_boundary);
                #[cfg(feature = "runtime-metrics")]
                let origin = if source.kind() == Some(InterfaceKind::LocalClient) {
                    AnnounceOrigin::SharedClient
                } else {
                    AnnounceOrigin::Relay
                };
                let mut buf = [0u8; BROADCAST_MTU];
                let directive = ReemitAnnounce {
                    announce: stored.announce.clone(),
                    emit_hops,
                    via,
                    target: source,
                    is_path_response: directed_to.is_some(),
                };
                let Ok(written) = directive.to_wire(&mut buf) else {
                    continue;
                };
                let bytes = &buf[..written];
                let next_hop_mode = interfaces.iter().find(|c| c.id == source).map(|c| c.mode);
                let mut fleets_emitted: u128 = 0;
                for descriptor in interfaces {
                    let eligible = match directed_to {
                        Some(target) => {
                            descriptor.id == target && descriptor.capabilities.allows_transmit()
                        }
                        None if source.kind() == Some(InterfaceKind::LocalClient) => {
                            descriptor.id.kind() != Some(InterfaceKind::LocalClient)
                                && descriptor.mode != InterfaceMode::AccessPoint
                                && descriptor.capabilities.allows_transmit()
                        }
                        None => {
                            descriptor.id.kind() != Some(InterfaceKind::LocalClient)
                                && firable_on(descriptor, source, next_hop_mode)
                        }
                    };
                    if !eligible {
                        continue;
                    }
                    match descriptor
                        .id
                        .kind()
                        .and_then(InterfaceKind::supervisor_kind)
                    {
                        Some(supervisor) => {
                            let bit = 1u128 << (supervisor as u8);
                            if fleets_emitted & bit == 0 {
                                fleets_emitted |= bit;
                                let fan = fleet_announce_fan_target(
                                    interfaces,
                                    supervisor,
                                    source,
                                    directed_to,
                                );
                                if fleet_fan_target_reaches_any_member(interfaces, supervisor, fan)
                                {
                                    sink(EngineReaction::Directive(
                                        Directive::SendAnnounceToFleet {
                                            supervisor,
                                            fan,
                                            bytes,
                                            hops: emit_hops,
                                            #[cfg(feature = "runtime-metrics")]
                                            origin,
                                        },
                                    ));
                                }
                            }
                        }
                        None => sink(EngineReaction::Directive(Directive::SendAnnounce {
                            target: descriptor.id,
                            bytes,
                            hops: emit_hops,
                            #[cfg(feature = "runtime-metrics")]
                            origin,
                        })),
                    }
                }
            }
        }
        self.scheduled_announces.advance_due_retransmits(
            now,
            REBROADCAST_RETRANSMIT_INTERVAL_MS,
            MAX_OUR_EMISSIONS,
        );
        WakeSchedules {
            scheduled_announces: self.scheduled_announces_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    pub fn fire_due_link_deadlines<F>(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        self.expire_unestablished_links(now, sink);
        self.cull_overdue_transported_links(now, interfaces, fill_entropy, sink);
        self.close_stale_links(now, interfaces, fill_entropy, sink);
        self.send_due_keepalives(now, interfaces, sink);
        WakeSchedules {
            link_deadlines: self.link_deadlines_wake(),
            ..WakeSchedules::UNCHANGED
        }
    }

    fn expire_unestablished_links(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        while let Some(overdue) = self.pop_timed_out_link(now) {
            if let OverdueLink::Initiated {
                command_id,
                destination,
                ..
            } = overdue
            {
                self.routing_table
                    .mark_responsiveness(&destination, RouteResponsiveness::Unresponsive);
                settle(
                    sink,
                    command_id,
                    Settlement::EstablishLink(Err(EstablishLinkFailure::Timeout)),
                );
            }
        }
    }

    fn cull_overdue_transported_links<F>(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let transport_id = self
            .network_transport_enabled()
            .then(|| self.transport_id())
            .flatten();
        while let Some(overdue) = self.transported_links.pop_overdue(now) {
            if overdue.validated_by_proof {
                self.mark_interface_dirty(overdue.next_hop_interface);
                self.mark_interface_dirty(overdue.received_interface);
                continue;
            }

            let initiated_by_local_client = overdue.taken_hops == 0;
            let initiator_is_neighbor = overdue.taken_hops == 1;
            let path_requests_are_throttled = self
                .recent_path_requests
                .is_throttled(&overdue.destination, now);

            let path_request_fan_target =
                match self.routing_table.hop_count_to(&overdue.destination) {
                    None => FanTarget::All,
                    Some(_) if path_requests_are_throttled => continue,
                    Some(_) if initiated_by_local_client => FanTarget::All,
                    Some(hops) if hops == 1 || initiator_is_neighbor => {
                        let arrival_mode = interfaces
                            .descriptor_for(overdue.received_interface)
                            .map(|descriptor| descriptor.mode);
                        if !matches!(arrival_mode, Some(InterfaceMode::Boundary)) {
                            self.routing_table.mark_responsiveness(
                                &overdue.destination,
                                RouteResponsiveness::Unresponsive,
                            );
                        }
                        FanTarget::AllExcept(overdue.received_interface)
                    }
                    Some(_) => continue,
                };
            let mut id = [0u8; TRUNCATED_HASH_BYTE_LEN];
            fill_entropy(&mut id);
            let mut request = [0u8; BROADCAST_MTU];
            if let Ok(wire_len) =
                write_path_request_wire_packet(overdue.destination, transport_id, &id, &mut request)
            {
                fan_frame(
                    interfaces,
                    path_request_fan_target,
                    &request[..wire_len],
                    sink,
                );
                self.recent_path_requests
                    .mark_seen_at(overdue.destination, now);
            }
        }
    }

    fn close_stale_links<F>(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        while let Some(link_id) = self.links.pop_stale(now) {
            let mut iv = [0u8; ENCRYPTION_IV_LEN];
            fill_entropy(&mut iv);
            let mut buf = [0u8; BROADCAST_MTU];
            if let Ok(dispatch) = self.write_owed_link_close(&link_id, &iv, &mut buf) {
                if let Some(target) = dispatch.fire_on {
                    if interfaces.is_egress_eligible(target, Egress::Transmit) {
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
    }

    fn send_due_keepalives(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        while let Some(due) = self.links.pop_due_keepalive(now) {
            if interfaces.is_egress_eligible(due.attached_interface, Egress::Transmit) {
                let mut buf = [0u8; BROADCAST_MTU];
                if let Ok(written) = write_keepalive(&due.link_id, KEEPALIVE_REQUEST, &mut buf) {
                    sink(EngineReaction::Directive(Directive::Send {
                        target: due.attached_interface,
                        bytes: &buf[..written],
                    }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{
        CommandId, IngestIo, PathRequestId, PathRequestWriteOutcome, RequestPath,
        RouteRemovalCause, WakeSchedule, PATH_REQUEST_TIMEOUT_MS,
    };
    use crate::interfaces::InterfaceDescriptor;
    use crate::interfaces::{InboundPacket, InterfaceId, InterfaceMode};
    use crate::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
    use crate::wire::{
        DestinationHash, DestinationType, PacketType, PropagationType, WirePacketHeader,
    };

    #[test]
    fn a_fresh_drive_is_deterministic_and_emits_nothing() {
        let mut left: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
        let mut right: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();

        let left_bytes = tick_capture(
            &mut left,
            InstantMillis(1_000),
            AttachedInterfaces::new(&[]),
        );
        let right_bytes = tick_capture(
            &mut right,
            InstantMillis(1_000),
            AttachedInterfaces::new(&[]),
        );

        assert_eq!(observable_state(&left), observable_state(&right));
        assert!(left_bytes.is_empty());
        assert_eq!(left_bytes, right_bytes);
    }

    #[test]
    fn accepted_announces_schedule_a_rebroadcast_and_tick_emits_them() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();
        let interfaces = [routable_descriptor(InterfaceId::new([0xFE; 8]))];

        let arrival = InstantMillis(1_000);
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(out, rns_1_3_5_announce_accepted(1));
        assert_eq!(state.scheduled_announce_count(), 1);

        let emitted = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            AttachedInterfaces::new(&interfaces),
        );
        assert_eq!(
            state.scheduled_announce_count(),
            1,
            "the first emission re-arms the entry for its second rebroadcast",
        );

        assert_eq!(emitted.len(), 1);
        let wire = &emitted[0];
        let (header, payload) = WirePacketHeader::parse(wire).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.propagation, PropagationType::Transport);
        assert_eq!(header.transport_id, Some(TEST_TRANSPORT_ID));
        let original = WirePacketHeader::parse(&raw).unwrap().0;
        assert_eq!(header.hops, original.hops + 1);
        assert_eq!(header.address, original.address);
        let original_payload = WirePacketHeader::parse(&raw).unwrap().1;
        assert_eq!(payload, original_payload);
    }

    #[test]
    fn a_rebroadcast_reproduces_the_rns_1_3_5_retransmitted_wire() {
        let mut heard = bytes_from_hex(RNS_1_3_5_RATCHETED_ANNOUNCE);
        let mut state = transporting_node();
        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut heard,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let emitted = tick_capture(
            &mut state,
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            AttachedInterfaces::new(&transporting_interfaces()),
        );
        assert_eq!(
            emitted,
            std::vec![bytes_from_hex(RNS_1_3_5_RETRANSMITTED_ANNOUNCE)],
            "our retransmission must be byte-identical to the reference's own",
        );
    }

    #[test]
    fn a_directed_scheduled_announce_fires_only_to_its_target_interface() {
        use crate::engine::{AnnounceIngest, IngestPacketOutcome};

        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();
        let IngestPacketOutcome::Announce(AnnounceIngest::Accepted(accepted)) = state
            .ingest_packet_with(
                InboundPacket {
                    arrived_at: InstantMillis(1_000),
                    source_interface: InterfaceId::new([0u8; 8]),
                    bytes: &mut raw,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            )
        else {
            panic!("the announce is accepted");
        };

        let target = InterfaceId::new([0xAA; 8]);
        state.scheduled_announces.schedule_directed(
            accepted.destination,
            InstantMillis(2_000),
            target,
            accepted.hops,
        );

        let interfaces = [
            routable_descriptor(target),
            routable_descriptor(InterfaceId::new([0xBB; 8])),
        ];
        let mut targets = std::vec::Vec::new();
        state.fire_due_scheduled_announces(
            InstantMillis(2_000),
            AttachedInterfaces::new(&interfaces),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction
                {
                    targets.push(target);
                }
            },
        );
        assert_eq!(
            targets,
            std::vec![target],
            "a directed answer reaches only its target, where a flood would reach both interfaces",
        );
    }

    fn rebroadcast_fan_for(
        state: &mut EngineState<TestStorageLayout>,
        interfaces: AttachedInterfaces<'_>,
    ) -> std::vec::Vec<InterfaceId> {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let mut targets = std::vec::Vec::new();
        let _ = state.fire_due_scheduled_announces(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            interfaces,
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction
                {
                    targets.push(target);
                }
            },
        );
        targets
    }

    #[test]
    fn a_same_interface_repeat_source_joins_its_own_rebroadcast_fan() {
        let source = InterfaceId::new([0u8; 8]);
        let other = InterfaceId::new([0xFE; 8]);
        let interfaces = [repeating_descriptor(source), routable_descriptor(other)];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
            std::vec![source, other],
        );
    }

    #[test]
    fn a_cross_interface_only_source_is_left_out_of_its_own_rebroadcast_fan() {
        let source = InterfaceId::new([0u8; 8]);
        let other = InterfaceId::new([0xFE; 8]);
        let interfaces = [routable_descriptor(source), routable_descriptor(other)];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
            std::vec![other]
        );
    }

    #[test]
    fn a_bluetooth_peer_announce_rebroadcasts_to_usb_device_transport() {
        let source = InterfaceId::new([InterfaceKind::BluetoothPeer as u8, 0x42, 0, 0, 0, 0, 0, 0]);
        let usb = InterfaceId::new([
            InterfaceKind::UsbAutoDevice as u8,
            b'i',
            b'o',
            b's',
            b'-',
            b'u',
            b's',
            b'b',
        ]);
        let interfaces = [
            routable_descriptor(source),
            crate::interfaces::usb_auto::core::device_descriptor(usb),
        ];

        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();
        let arrival = InstantMillis(1_000);
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        assert_eq!(out, rns_1_3_5_announce_accepted(1));
        assert_eq!(state.scheduled_announce_count(), 1);

        let mut targets = std::vec::Vec::new();
        let _ = state.fire_due_scheduled_announces(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            AttachedInterfaces::new(&interfaces),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction
                {
                    targets.push(target);
                }
            },
        );

        assert_eq!(
            targets,
            std::vec![usb],
            "a transport-enabled iPad must forward a BLE-learned announce over USB",
        );
    }

    #[test]
    fn our_own_repeat_echoed_back_is_deduplicated() {
        use crate::engine::{AnnounceIngest, IngestPacketOutcome};

        let source = InterfaceId::new([0u8; 8]);
        let interfaces = [repeating_descriptor(source)];
        let mut state = transporting_node();
        let fan = rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces));
        assert_eq!(fan, std::vec![source]);

        let mut echo = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        echo[1] += 1;
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(5_000),
                source_interface: source,
                bytes: &mut echo,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        assert_eq!(
            out,
            IngestPacketOutcome::Announce(AnnounceIngest::Ignored),
            "the repeat coming home is the same announce: dedup eats it, no loop",
        );
        assert_eq!(state.route_count(), 1);
        assert_eq!(
            state.scheduled_announce_count(),
            1,
            "the echo is absorbed as a same-distance peer rebroadcast — counted, not looped into a fresh schedule",
        );
    }

    #[test]
    fn an_onward_announce_echo_cancels_the_pending_retransmit() {
        let source = InterfaceId::new([0u8; 8]);
        let interfaces = [repeating_descriptor(source)];
        let mut state = transporting_node();
        let fan = rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces));
        assert_eq!(fan, std::vec![source]);
        assert_eq!(
            state.scheduled_announce_count(),
            1,
            "after one emission the entry is re-armed for its second rebroadcast",
        );

        let mut echo = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        echo[1] += 2;
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(5_000),
                source_interface: source,
                bytes: &mut echo,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        assert_eq!(
            state.scheduled_announce_count(),
            0,
            "hearing our own rebroadcast one hop onward retires the pending retransmit",
        );
    }

    #[test]
    fn an_interface_that_cannot_transport_never_joins_a_rebroadcast_fan() {
        use crate::interfaces::{EgressCapability, TransportCapability};

        let source = InterfaceId::new([0u8; 8]);
        let mut leaf = routable_descriptor(InterfaceId::new([0xFE; 8]));
        leaf.capabilities.egress = EgressCapability::Enabled(TransportCapability::NoTransport);
        let interfaces = [routable_descriptor(source), leaf];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
            std::vec![]
        );
    }

    fn moded(mode: InterfaceMode, descriptor: InterfaceDescriptor) -> InterfaceDescriptor {
        InterfaceDescriptor { mode, ..descriptor }
    }

    #[test]
    fn an_access_point_egress_interface_is_withheld_from_the_rebroadcast_fan() {
        let source = InterfaceId::new([0u8; 8]);
        let ap = InterfaceId::new([0xFE; 8]);
        let interfaces = [
            repeating_descriptor(source),
            moded(InterfaceMode::AccessPoint, routable_descriptor(ap)),
        ];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
            std::vec![source],
            "an access-point interface never carries an announce rebroadcast",
        );
    }

    #[test]
    fn a_local_clients_announce_is_also_withheld_from_access_point_egress() {
        let app = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
        let ap = InterfaceId::new([0xFE; 8]);
        let interfaces = [
            routable_descriptor(app),
            moded(InterfaceMode::AccessPoint, routable_descriptor(ap)),
        ];
        let mut state = transporting_node();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: app,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let mut targets = std::vec::Vec::new();
        let _ = state.fire_due_scheduled_announces(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            AttachedInterfaces::new(&interfaces),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { target, .. }) = reaction
                {
                    targets.push(target);
                }
            },
        );
        assert!(targets.is_empty());
    }

    #[test]
    fn a_scheduled_local_client_announce_uses_the_hop_count_override_at_external_egress() {
        let local_client = InterfaceId::from_channel_tag(InterfaceKind::LocalClient, b"sideband");
        let external = InterfaceId::new([0xFE; 8]);
        let interfaces = [
            routable_descriptor(local_client),
            routable_descriptor(external),
        ];
        let mut state = transporting_node();
        state.protocol.local_hop_count_override =
            crate::engine::LocalHopCountOverride::override_with(5).unwrap();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let arrival = InstantMillis(1_000);
        let out = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: local_client,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        assert_eq!(out, rns_1_3_5_announce_accepted(0));

        let mut emitted = std::vec::Vec::new();
        let _ = state.fire_due_scheduled_announces(
            InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1),
            AttachedInterfaces::new(&interfaces),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce {
                    target, bytes, ..
                }) = reaction
                {
                    emitted.push((target, WirePacketHeader::parse(bytes).unwrap().0.hops));
                }
            },
        );

        assert_eq!(emitted, std::vec![(external, 5)]);
    }

    #[test]
    fn a_roaming_egress_interface_is_withheld_toward_a_roaming_learned_route() {
        let source = InterfaceId::new([0u8; 8]);
        let roaming_out = InterfaceId::new([0xFE; 8]);
        let other = InterfaceId::new([0xAB; 8]);
        let interfaces = [
            moded(InterfaceMode::Roaming, repeating_descriptor(source)),
            moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
            routable_descriptor(other),
        ];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
            std::vec![other],
            "a roaming interface withholds a roaming-learned route; a full interface carries it",
        );
    }

    #[test]
    fn a_roaming_egress_interface_carries_a_full_learned_route() {
        let source = InterfaceId::new([0u8; 8]);
        let roaming_out = InterfaceId::new([0xFE; 8]);
        let interfaces = [
            repeating_descriptor(source),
            moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
        ];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
            std::vec![source, roaming_out],
        );
    }

    #[test]
    fn a_boundary_egress_carries_a_boundary_learned_route_where_a_roaming_egress_will_not() {
        let source = InterfaceId::new([0u8; 8]);
        let boundary_out = InterfaceId::new([0xFE; 8]);
        let roaming_out = InterfaceId::new([0xAB; 8]);
        let interfaces = [
            moded(InterfaceMode::Boundary, repeating_descriptor(source)),
            moded(InterfaceMode::Boundary, routable_descriptor(boundary_out)),
            moded(InterfaceMode::Roaming, routable_descriptor(roaming_out)),
        ];

        let mut state = transporting_node();
        assert_eq!(
            rebroadcast_fan_for(&mut state, AttachedInterfaces::new(&interfaces)),
            std::vec![source, boundary_out],
            "boundary carries a boundary-learned route; roaming withholds the same route",
        );
    }

    #[test]
    fn scheduled_announces_are_not_emitted_before_their_due_time() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();
        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let interfaces = [routable_descriptor(InterfaceId::new([0xFE; 8]))];
        let emitted = tick_capture(
            &mut state,
            InstantMillis(arrival.0 - 1),
            AttachedInterfaces::new(&interfaces),
        );
        assert!(emitted.is_empty());
        assert_eq!(state.scheduled_announce_count(), 1);
    }

    #[test]
    fn same_inputs_produce_byte_identical_emissions_on_two_engines() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let now = InstantMillis(5_000);
        let arrival = InstantMillis(1_000);

        let mut left = transporting_node();
        let mut right = transporting_node();

        let interfaces = [routable_descriptor(InterfaceId::new([0xFE; 8]))];
        for state in [&mut left, &mut right] {
            let _ = state.ingest_packet_with(
                InboundPacket {
                    arrived_at: arrival,
                    source_interface: InterfaceId::new([0u8; 8]),
                    bytes: &mut raw,
                },
                &mut |_| {},
                AttachedInterfaces::new(&transporting_interfaces()),
                &mut |_| {},
                None,
            );
        }
        let left_bytes = tick_capture(&mut left, now, AttachedInterfaces::new(&interfaces));
        let right_bytes = tick_capture(&mut right, now, AttachedInterfaces::new(&interfaces));

        assert_eq!(observable_state(&left), observable_state(&right));
        assert_eq!(left_bytes, right_bytes);
        assert_eq!(left_bytes.len(), 1);
    }

    #[test]
    fn fire_due_scheduled_announces_emits_then_re_arms_until_the_cap() {
        fn fire(
            state: &mut EngineState<TestStorageLayout>,
            now: InstantMillis,
            interfaces: AttachedInterfaces<'_>,
        ) -> (
            std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
            WakeSchedule,
        ) {
            let mut sent = std::vec::Vec::new();
            let delta = state.fire_due_scheduled_announces(now, interfaces, &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce {
                    target, bytes, ..
                }) = reaction
                {
                    sent.push((target, bytes.to_vec()));
                }
            });
            (sent, delta.scheduled_announces)
        }

        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();
        let target = InterfaceId::new([0xFE; 8]);
        let interfaces = [routable_descriptor(target)];

        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let first_due = InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
        let (sent, schedule) = fire(&mut state, first_due, AttachedInterfaces::new(&interfaces));
        assert_eq!(sent.len(), 1, "one directive for the lone interface");
        assert_eq!(
            sent[0].0, target,
            "the rebroadcast names the firable interface"
        );
        assert_eq!(
            state.scheduled_announce_count(),
            1,
            "the first emission re-arms the entry rather than clearing it",
        );
        assert_eq!(
            schedule,
            WakeSchedule::At(InstantMillis(
                first_due.0 + REBROADCAST_RETRANSMIT_INTERVAL_MS
            )),
            "the schedule is re-armed one retransmit interval out",
        );
        let (header, _) = WirePacketHeader::parse(&sent[0].1).unwrap();
        assert_eq!(header.packet_type, PacketType::Announce);
        let original = WirePacketHeader::parse(&bytes_from_hex(RNS_1_3_5_ANNOUNCE))
            .unwrap()
            .0;
        assert_eq!(
            header.hops,
            original.hops + 1,
            "the rebroadcast bumps the hop count"
        );
        let first_bytes = sent[0].1.clone();

        let second_due = InstantMillis(first_due.0 + REBROADCAST_RETRANSMIT_INTERVAL_MS);
        let (sent, schedule) = fire(&mut state, second_due, AttachedInterfaces::new(&interfaces));
        assert_eq!(sent.len(), 1, "the second and final emission");
        assert_eq!(
            sent[0].1, first_bytes,
            "the retransmit re-emits the same pinned announce, byte for byte",
        );
        assert_eq!(
            state.scheduled_announce_count(),
            0,
            "reaching the rebroadcast cap drops the entry",
        );
        assert_eq!(
            schedule,
            WakeSchedule::Idle,
            "no rebroadcasts remain after the cap"
        );
    }

    #[test]
    fn an_ignored_echo_that_cancels_a_rebroadcast_reports_the_emptied_lane() {
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let mut state = transporting_node();
        let target = InterfaceId::new([0xFE; 8]);
        let interfaces = [routable_descriptor(target)];

        let arrival = InstantMillis(1_000);
        let _ = state.ingest_packet_with(
            InboundPacket {
                arrived_at: arrival,
                source_interface: InterfaceId::new([0u8; 8]),
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
        );
        assert_eq!(state.scheduled_announce_count(), 1);

        let first_due = InstantMillis(arrival.0 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
        let mut rebroadcast = std::vec::Vec::new();
        let _ = state.fire_due_scheduled_announces(
            first_due,
            AttachedInterfaces::new(&interfaces),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::SendAnnounce { bytes, .. }) = reaction {
                    rebroadcast = bytes.to_vec();
                }
            },
        );
        assert_eq!(state.scheduled_announce_count(), 1);
        assert!(
            !rebroadcast.is_empty(),
            "the fire emitted a rebroadcast to echo back"
        );

        let echo = |state: &mut EngineState<TestStorageLayout>, now: u64| -> WakeSchedule {
            let mut bytes = rebroadcast.clone();
            state
                .ingest_packet_into(
                    InboundPacket {
                        arrived_at: InstantMillis(now),
                        source_interface: InterfaceId::new([0u8; 8]),
                        bytes: &mut bytes,
                    },
                    IngestIo {
                        interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                        now: InstantMillis(now),
                        fill_entropy: &mut |bytes: &mut [u8]| bytes.fill(0),
                        should_prove: &mut |_| false,
                        should_accept_resource:
                            &mut |_: &crate::routing::links::resources::ResourceOffer| false,
                        sink: &mut |_| {},
                    },
                )
                .scheduled_announces
        };

        let echo_at = first_due.0 + 1;
        let _ = echo(&mut state, echo_at);
        assert_eq!(
            state.scheduled_announce_count(),
            1,
            "the first echo only counts the peer rebroadcast",
        );

        let second = echo(&mut state, echo_at + 1);
        assert_eq!(
            state.scheduled_announce_count(),
            0,
            "the second echo reaches the peer cap and cancels the pending rebroadcast",
        );
        assert_eq!(
            second,
            WakeSchedule::Idle,
            "an ignored echo that empties the queue reports Idle, not a stale Unchanged",
        );
        assert_eq!(
            second,
            state.scheduled_announces_wake(),
            "the ingest delta agrees with a full wake recompute (no reactor drift)",
        );
    }

    #[test]
    fn settle_timed_out_path_requests_closes_each_expired_request_once_past_its_deadline() {
        let mut engine = EngineState::<TestStorageLayout>::default();
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
    fn the_cull_journals_an_orphan_as_route_interface_gone() {
        let source = InterfaceId::new([0u8; 8]);
        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&[routable_descriptor(source)]),
            &mut |_| {},
            None,
        );
        assert_eq!(engine.route_count(), 1);

        let without_source = [routable_descriptor(InterfaceId::new([0xEE; 8]))];
        let mut journal = std::vec::Vec::new();
        let delta = engine.cull_expired_routes(
            InstantMillis(2_000),
            AttachedInterfaces::new(&without_source),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RouteRemoved {
                    destination,
                    cause: RouteRemovalCause::InterfaceGone,
                }) = reaction
                {
                    journal.push(destination);
                }
            },
        );
        assert_eq!(
            journal,
            std::vec![DestinationHash::new(
                bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                    .try_into()
                    .unwrap()
            )],
            "the orphan's removal names its cause",
        );
        assert_eq!(engine.route_count(), 0);
        assert_eq!(
            delta.expired_routes,
            crate::engine::WakeSchedule::Idle,
            "nothing is left to wake for",
        );
    }

    #[test]
    fn a_dropped_route_marks_its_interface_so_the_destination_count_recomputes() {
        let source = InterfaceId::new([0u8; 8]);
        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&[routable_descriptor(source)]),
            &mut |_| {},
            None,
        );

        let mut on_insert = std::vec::Vec::new();
        engine
            .take_dirty_interfaces()
            .drain(|interface| on_insert.push(interface));
        assert_eq!(
            on_insert,
            std::vec![source],
            "learning a route marks the interface it arrived on",
        );
        assert_eq!(engine.interface_counts(source).destinations, 1);

        let without_source = [routable_descriptor(InterfaceId::new([0xEE; 8]))];
        engine.cull_expired_routes(
            InstantMillis(2_000),
            AttachedInterfaces::new(&without_source),
            &mut |_| {},
        );

        let mut on_cull = std::vec::Vec::new();
        engine
            .take_dirty_interfaces()
            .drain(|interface| on_cull.push(interface));
        assert_eq!(
            on_cull,
            std::vec![source],
            "dropping the route re-marks the interface, so the stale count never lingers silently",
        );
        assert_eq!(engine.interface_counts(source).destinations, 0);
    }

    #[test]
    fn an_unproved_transported_link_to_a_neighbor_marks_the_route_unresponsive() {
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let source = InterfaceId::new([0xA1; 8]);
        let interfaces = [routable_descriptor(source)];
        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        let destination = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );
        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
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
                validated_by_proof: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            AttachedInterfaces::new(&interfaces),
            &mut |bytes: &mut [u8]| bytes.fill(0),
            &mut |_| {},
        );

        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unresponsive,
            "the neighbor link never proved, so its route is marked unresponsive",
        );
    }

    #[test]
    fn an_unproved_neighbor_link_fires_a_path_request_away_from_the_received_lane() {
        use crate::engine::PATH_REQUEST_DESTINATION;
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let received = InterfaceId::new([0xA1; 8]);
        let away = InterfaceId::new([0xB2; 8]);
        let interfaces = [routable_descriptor(received), routable_descriptor(away)];

        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: received,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        let destination = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );

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
                validated_by_proof: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let mut sent = std::vec::Vec::new();
        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            AttachedInterfaces::new(&interfaces),
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
                .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
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
        assert_eq!(
            DestinationHash::from_address(header.address),
            PATH_REQUEST_DESTINATION
        );
        assert_eq!(header.destination_type, DestinationType::Plain);
        assert_eq!(
            &payload[..16],
            destination.as_bytes(),
            "and it asks for the destination whose link just died",
        );
    }

    #[test]
    fn an_unproved_link_recovers_when_the_initiator_is_the_neighbor_too() {
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let received = InterfaceId::new([0xA1; 8]);
        let away = InterfaceId::new([0xB2; 8]);
        let interfaces = [routable_descriptor(received), routable_descriptor(away)];

        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: received,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        let destination = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );

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
                validated_by_proof: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let mut sent = std::vec::Vec::new();
        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            AttachedInterfaces::new(&interfaces),
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
                .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unresponsive,
            "a far destination still recovers when its link initiator is our neighbor",
        );
        assert_eq!(sent, std::vec![away]);
    }

    #[test]
    fn an_unproved_link_from_a_local_client_rediscovers_everywhere_without_a_mark() {
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let received = InterfaceId::new([0xA1; 8]);
        let away = InterfaceId::new([0xB2; 8]);
        let interfaces = [routable_descriptor(received), routable_descriptor(away)];

        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: received,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        let destination = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );

        engine
            .transported_links
            .track(TransportedLink {
                link_id: LinkId::new([0x5C; 16]),
                destination,
                next_hop: None,
                next_hop_interface: away,
                received_interface: received,
                taken_hops: 0,
                remaining_hops: 1,
                validated_by_proof: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let mut sent = std::vec::Vec::new();
        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            AttachedInterfaces::new(&interfaces),
            &mut |bytes: &mut [u8]| bytes.fill(0x5A),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                    sent.push(target);
                }
            },
        );

        assert_eq!(
            sent,
            std::vec![received, away],
            "a local client's dead link re-requests on every interface, its own included",
        );
        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unknown,
            "the route itself is not the suspect when the local client's request died",
        );
    }

    #[test]
    fn a_boundary_arrival_interface_rediscovers_without_the_unresponsive_mark() {
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let received = InterfaceId::new([0xA1; 8]);
        let away = InterfaceId::new([0xB2; 8]);
        let learn_view = [routable_descriptor(received), routable_descriptor(away)];
        let fire_view = [
            moded(InterfaceMode::Boundary, routable_descriptor(received)),
            routable_descriptor(away),
        ];

        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: received,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&learn_view),
            &mut |_| {},
            None,
        );
        let destination = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );

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
                validated_by_proof: false,
                last_active: InstantMillis(1_000),
                proof_timeout: InstantMillis(7_000),
            })
            .unwrap();

        let mut sent = std::vec::Vec::new();
        let _ = engine.fire_due_link_deadlines(
            InstantMillis(7_000),
            AttachedInterfaces::new(&fire_view),
            &mut |bytes: &mut [u8]| bytes.fill(0x5A),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { target, .. }) = reaction {
                    sent.push(target);
                }
            },
        );

        assert_eq!(
            sent,
            std::vec![away],
            "the re-request still fires away from the dead link's lane",
        );
        assert_eq!(
            engine
                .routing_table
                .existing_route_for(&destination, AttachedInterfaces::new(&fire_view))
                .unwrap()
                .responsiveness,
            RouteResponsiveness::Unknown,
            "a boundary interface's routes are not marked unresponsive over one silent link",
        );
    }

    #[test]
    fn a_may_return_departure_holds_the_bounced_peers_routes_through_the_grace() {
        use crate::engine::{Departure, DEPARTED_INTERFACE_GRACE_MS};

        let source = InterfaceId::new([0xA1; 8]);
        let other = InterfaceId::new([0xB2; 8]);
        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&[routable_descriptor(source), routable_descriptor(other)]),
            &mut |_| {},
            None,
        );
        assert_eq!(engine.route_count(), 1);

        engine.interface_departed(source, Departure::MayReturn, InstantMillis(2_000));
        let without_source = [routable_descriptor(other)];
        engine.cull_expired_routes(
            InstantMillis(2_001),
            AttachedInterfaces::new(&without_source),
            &mut |_| {},
        );
        assert_eq!(
            engine.route_count(),
            1,
            "within the grace the bounced peer's route holds",
        );
        assert_eq!(
            engine.route_expiry_wake(AttachedInterfaces::new(&without_source)),
            WakeSchedule::At(InstantMillis(2_000 + DEPARTED_INTERFACE_GRACE_MS)),
            "the wake names the grace deadline",
        );

        let mut journal = std::vec::Vec::new();
        engine.cull_expired_routes(
            InstantMillis(2_000 + DEPARTED_INTERFACE_GRACE_MS),
            AttachedInterfaces::new(&without_source),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::RouteRemoved {
                    destination,
                    cause: RouteRemovalCause::InterfaceGone,
                }) = reaction
                {
                    journal.push(destination);
                }
            },
        );
        assert_eq!(
            engine.route_count(),
            0,
            "past the grace the orphan finally culls",
        );
        assert_eq!(journal.len(), 1, "and its removal still names its cause");
    }

    #[cfg(feature = "std")]
    #[test]
    fn a_growable_hosts_route_index_rebuilds_for_departure_warmth() {
        use crate::engine::{Departure, DEPARTED_INTERFACE_GRACE_MS};
        use crate::storage::GrowableHeap;

        let source = InterfaceId::new([0xA1; 8]);
        let other = InterfaceId::new([0xB2; 8]);
        let attached = [routable_descriptor(source), routable_descriptor(other)];
        let mut engine = EngineState::<GrowableHeap>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&attached),
            &mut |_| {},
            None,
        );
        assert!(matches!(
            engine.route_expiry_wake(AttachedInterfaces::new(&attached)),
            WakeSchedule::At(_)
        ));

        engine.interface_departed(source, Departure::MayReturn, InstantMillis(2_000));
        let without_source = [routable_descriptor(other)];
        assert_eq!(
            engine.route_expiry_wake(AttachedInterfaces::new(&without_source)),
            WakeSchedule::At(InstantMillis(2_000 + DEPARTED_INTERFACE_GRACE_MS))
        );
    }

    #[test]
    fn a_forgotten_departure_culls_the_routes_at_once() {
        use crate::engine::Departure;

        let source = InterfaceId::new([0xA1; 8]);
        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&[routable_descriptor(source)]),
            &mut |_| {},
            None,
        );
        assert_eq!(engine.route_count(), 1);

        engine.interface_departed(source, Departure::Forgotten, InstantMillis(2_000));
        let without_source = [routable_descriptor(InterfaceId::new([0xEE; 8]))];
        engine.cull_expired_routes(
            InstantMillis(2_001),
            AttachedInterfaces::new(&without_source),
            &mut |_| {},
        );
        assert_eq!(
            engine.route_count(),
            0,
            "a deliberate forget keeps the reference's eager cull",
        );
    }

    #[test]
    fn a_returned_interface_resumes_normal_route_aging() {
        use crate::engine::{Departure, DEPARTED_INTERFACE_GRACE_MS};

        let source = InterfaceId::new([0xA1; 8]);
        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: source,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&[routable_descriptor(source)]),
            &mut |_| {},
            None,
        );

        engine.interface_departed(source, Departure::MayReturn, InstantMillis(2_000));
        let interfaces = [routable_descriptor(source)];
        engine.cull_expired_routes(
            InstantMillis(2_000 + DEPARTED_INTERFACE_GRACE_MS + 1),
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
        );
        assert_eq!(
            engine.route_count(),
            1,
            "back among the attached interfaces, the stale grace entry is ignored and mode expiry governs",
        );
    }

    #[test]
    fn a_recently_requested_destination_holds_off_the_overdue_links_path_request() {
        use crate::routing::links::transported::TransportedLink;
        use crate::routing::links::LinkId;

        let received = InterfaceId::new([0xA1; 8]);
        let away = InterfaceId::new([0xB2; 8]);
        let interfaces = [routable_descriptor(received), routable_descriptor(away)];

        let mut engine = EngineState::<TestStorageLayout>::default();
        let mut raw = bytes_from_hex(RNS_1_3_5_ANNOUNCE);
        let _ = engine.ingest_packet_with(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: received,
                bytes: &mut raw,
            },
            &mut |_| {},
            AttachedInterfaces::new(&interfaces),
            &mut |_| {},
            None,
        );
        let destination = DestinationHash::new(
            bytes_from_hex("16f8a6d3f7d7c5b6f106d293804d7314")
                .try_into()
                .unwrap(),
        );

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
                validated_by_proof: false,
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
            AttachedInterfaces::new(&interfaces),
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
                .existing_route_for(&destination, AttachedInterfaces::new(&interfaces))
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
