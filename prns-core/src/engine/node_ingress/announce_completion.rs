use super::journal_route_removal;
use crate::engine::settlement::settle;
use crate::engine::{
    AnnounceIngest, AnnounceVerifyOwed, EngineReaction, EngineState, Journaled, PathFound,
    Settlement, WakeSchedule, WakeSchedules,
};
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::announce::{Announce, AnnounceArrival};
use crate::routing::ingress::{AcceptedAnnounceEffect, IngestEffects};
use crate::storage::StorageLayout;

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn apply_announce_ingest(
        &mut self,
        ingest: AnnounceIngest,
        accepted_observation: Option<AcceptedAnnounceEffect<'_>>,
        source: InterfaceId,
        interfaces: AttachedInterfaces<'_>,
        wake: &mut WakeSchedules,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        #[cfg(feature = "runtime-metrics")]
        self.record_announce_ingress(source, ingest);
        match ingest {
            AnnounceIngest::Accepted(accepted) => {
                self.relay_announce_to_local_clients(
                    accepted.destination,
                    accepted.hops,
                    source,
                    interfaces,
                    sink,
                );
                if let Some(AcceptedAnnounceEffect {
                    observation,
                    rate_accounting,
                }) = accepted_observation
                {
                    sink(EngineReaction::Journaled(Journaled::AnnounceHeard {
                        observation,
                        rate_accounting,
                    }));
                }
                while let Some(settled) = self.pop_settled_path_request(&accepted.destination) {
                    settle(
                        sink,
                        settled.command_id,
                        Settlement::RequestPath(Ok(PathFound {
                            hops: crate::units::HopCount(accepted.hops),
                        })),
                    );
                }
                wake.scheduled_announces = self.scheduled_announces_wake();
                wake.path_request_timeouts = self.path_request_timeouts_wake();
                wake.expired_routes = self
                    .routing_table
                    .existing_route_for(&accepted.destination, interfaces)
                    .map_or(WakeSchedule::Unchanged, |route| {
                        WakeSchedule::AtMost(route.expires_at)
                    });
            }
            AnnounceIngest::Ignored | AnnounceIngest::Blackholed => {
                wake.scheduled_announces = self.scheduled_announces_wake();
            }
            AnnounceIngest::Held => {
                wake.held_announce_release = self.held_announce_release_wake();
            }
            AnnounceIngest::HeldDropped { destination, cause } => {
                sink(EngineReaction::Journaled(Journaled::AnnounceHeldDropped {
                    destination,
                    source_interface: source,
                    cause,
                }));
            }
        }
    }

    pub fn resume_announce(
        &mut self,
        owed: AnnounceVerifyOwed,
        interfaces: AttachedInterfaces<'_>,
        fill_entropy: &mut impl FnMut(&mut [u8]),
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let mut wake = WakeSchedules::UNCHANGED;
        let Ok((announce, identity_hash)) =
            Announce::from_wire_unverified_with_identity(&owed.header, &owed.payload)
        else {
            return wake;
        };
        let source = owed.source_interface;
        self.interface_announce_limits
            .record(source, owed.arrived_at);
        let arrival = AnnounceArrival {
            announce,
            hops: owed.received_hops,
            arrived_at: owed.arrived_at,
            receiving_interface: source,
            next_hop: owed.next_hop,
            is_path_response: owed.is_path_response,
        };
        let mut effects = IngestEffects::default();
        let ingest = self.ingest_announce(
            identity_hash,
            &arrival,
            fill_entropy,
            interfaces,
            &mut |removed| sink(EngineReaction::Journaled(journal_route_removal(removed))),
            &mut effects,
        );
        let accepted_observation = effects.accepted_announce.take();
        self.apply_announce_ingest(
            ingest,
            accepted_observation,
            source,
            interfaces,
            &mut wake,
            sink,
        );
        if let Some(expiry) = effects.destination_identity_expiry {
            wake.expired_destination_identities = WakeSchedule::AtMost(expiry);
        }
        wake
    }
}
