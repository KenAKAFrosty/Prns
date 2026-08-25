use crate::engine::node_egress::{
    allows_announce_rebroadcast, fan_frame, fleet_announce_fan_target,
    fleet_fan_target_reaches_any_member,
};
use crate::engine::settlement::{settle, timeout_settlement};
#[cfg(feature = "runtime-metrics")]
use crate::engine::AnnounceOrigin;
use crate::engine::{
    Directive, EngineReaction, EngineState, EstablishLinkFailure, FanTarget, InstantMillis,
    Journaled, LinkClosedReason, ReemitAnnounce, RequestPathFailure, Settlement, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{AttachedInterfaces, Egress};
use crate::interfaces::{InterfaceKind, InterfaceMode};
use crate::routing::announce::defaults::{MAX_OUR_EMISSIONS, REBROADCAST_RETRANSMIT_INTERVAL_MS};
use crate::routing::announce::schedule::ScheduledAnnounceQueue as _;
use crate::routing::links::maintenance::{write_keepalive, KEEPALIVE_REQUEST};
use crate::routing::links::table::OverdueLink;
use crate::routing::path_requests::write_path_request_wire_packet;
use crate::routing::warmth::WarmestOf;
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
            resource_deadlines: self.resource_deadlines_wake(),
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

    /// The reference's two cull arms (RNS 1.4.2 `Transport.jobs`): [`RouteRemovalCause::Expired`](crate::routing::RouteRemovalCause::Expired) for the aged, [`RouteRemovalCause::InterfaceGone`](crate::routing::RouteRemovalCause::InterfaceGone) for the orphaned.
    /// The orphan arm is softened by the [`crate::routing::warmth::DepartedInterfaces`] grace; the reverse-route and transported-link culls below stay eager like the reference's, since they carry in-flight work that a bounced lane kills regardless.
    pub fn cull_expired_routes(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        self.reconcile_pending_link_route_evidence();
        let tunnels_changed = self.tunnels.expire(now) != 0;
        let departures_changed = self.departed_interfaces.evict_expired(now) != 0;
        if tunnels_changed || departures_changed {
            self.routing_table.invalidate_route_expiries();
        }
        let warmth = WarmestOf(&self.tunnels, &self.departed_interfaces);
        let dirty = &mut self.dirty_interfaces;
        let scheduled_announces = &mut self.scheduled_announces;
        let culled_routes = self.routing_table.cull_expired_routes_indexed_with_warmth(
            now,
            interfaces,
            &warmth,
            &mut |removed| {
                let _ = scheduled_announces.cancel(&removed.destination);
                dirty.mark(removed.receiving_interface);
                sink(EngineReaction::Journaled(
                    crate::engine::node_ingress::journal_route_removal(removed),
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
            scheduled_announces: if culled_routes == 0 {
                crate::engine::WakeSchedule::Unchanged
            } else {
                self.scheduled_announces_wake()
            },
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
                let source_descriptor = interfaces.iter().find(|candidate| candidate.id == source);
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
                                && allows_announce_rebroadcast(
                                    descriptor,
                                    source,
                                    source_descriptor,
                                )
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
        self.reconcile_pending_link_route_evidence();
        self.expire_unestablished_links(now, sink);
        self.cull_overdue_transported_links(now, interfaces, fill_entropy, sink);
        self.close_stale_links(now, interfaces, fill_entropy, sink);
        self.send_due_keepalives(now, interfaces, sink);
        WakeSchedules {
            link_deadlines: self.link_deadlines_wake(),
            resource_deadlines: self.resource_deadlines_wake(),
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
                mut route_evidence,
                requested_at,
                ..
            } = overdue
            {
                self.routing_table
                    .mark_unresponsive_if_not_active_since(&mut route_evidence, requested_at);
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
        while let Some(mut overdue) = self.transported_links.pop_overdue(now) {
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
                            self.routing_table.mark_unresponsive_if_not_active_since(
                                &mut overdue.route_evidence,
                                overdue.last_active,
                            );
                        }
                        FanTarget::AllExcept(overdue.received_interface)
                    }
                    Some(_) => continue,
                };
            let mut id = [0u8; TRUNCATED_HASH_BYTE_LEN];
            fill_entropy(&mut id);
            let mut request = [0u8; BROADCAST_MTU];
            if let Ok(wire_bytes) =
                write_path_request_wire_packet(overdue.destination, transport_id, &id, &mut request)
            {
                fan_frame(
                    interfaces,
                    path_request_fan_target,
                    &request[..wire_bytes],
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
                            bytes: &buf[..dispatch.wire_bytes],
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
mod tests;
