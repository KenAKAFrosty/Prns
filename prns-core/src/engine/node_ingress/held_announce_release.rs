use super::journal_route_removal;
use crate::engine::settlement::settle;
use crate::engine::{
    AnnounceIngest, EngineReaction, EngineState, InstantMillis, Journaled, PathFound, Settlement,
    WakeSchedule, WakeSchedules,
};
use crate::interfaces::{AttachedInterfaces, InterfaceCommonPolicy, InterfaceId};
use crate::routing::announce::{Announce, AnnounceArrival};
use crate::routing::ingress::{AcceptedAnnounceEffect, IngestEffects};
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;

impl<S: StorageLayout> EngineState<S> {
    /// RNS 1.4.2 `Interface.process_held_announces`.
    pub fn fire_due_held_announces<F>(
        &mut self,
        now: InstantMillis,
        interfaces: AttachedInterfaces<'_>,
        fill_random: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let mut wake = WakeSchedules::UNCHANGED;
        let mut destination_identity_expiry = None;
        let mut released_any = false;
        while let Some(interface) = self.next_due_held_interface(now) {
            let policy = interfaces.descriptor_for(interface).map_or(
                InterfaceCommonPolicy::RNS_DEFAULT.ingress_control,
                |descriptor| descriptor.common.ingress_control,
            );
            self.interface_announce_limits
                .schedule_next_held_release_with_policy(interface, now, policy);
            if !self
                .interface_announce_limits
                .rate_is_under_limit_with_policy(interface, now, policy)
            {
                continue;
            }
            let mut app_data = [0u8; BROADCAST_MTU];
            let Some(released) = self
                .held_announces
                .release_lowest_hop_for(interface, &mut app_data)
            else {
                continue;
            };
            let held = released.held_announce;
            let app_data_bytes = released.app_data_bytes;
            let announce = Announce {
                destination: held.destination,
                public_keys: held.announce.public_keys,
                dotted_name_hash: held.announce.dotted_name_hash,
                announce_id: held.announce.announce_id,
                ratchet: held.announce.ratchet,
                signature: held.announce.signature,
                app_data: &app_data[..app_data_bytes],
            };
            let arrival = AnnounceArrival {
                announce,
                hops: held.hops,
                arrived_at: now,
                receiving_interface: held.receiving_interface,
                next_hop: held.next_hop,
                is_path_response: held.is_path_response,
            };
            let identity_hash = arrival.announce.public_keys.identity_hash();
            let mut effects = IngestEffects::default();
            let ingest = self.ingest_announce(
                identity_hash,
                &arrival,
                &mut *fill_random,
                interfaces,
                &mut |removed| sink(EngineReaction::Journaled(journal_route_removal(removed))),
                &mut effects,
            );
            if let Some(AcceptedAnnounceEffect {
                observation,
                rate_accounting,
            }) = effects.accepted_announce.take()
            {
                sink(EngineReaction::Journaled(Journaled::AnnounceHeard {
                    observation,
                    rate_accounting,
                }));
            }
            if let Some(expiry) = effects.destination_identity_expiry {
                destination_identity_expiry = Some(
                    destination_identity_expiry
                        .map_or(expiry, |current: InstantMillis| current.min(expiry)),
                );
            }
            if let AnnounceIngest::Accepted(accepted) = ingest {
                released_any = true;
                while let Some(settled) = self.pop_settled_path_request(&accepted.destination) {
                    settle(
                        sink,
                        settled.command_id,
                        Settlement::RequestPath(Ok(PathFound {
                            hops: crate::units::HopCount(accepted.hops),
                        })),
                    );
                }
            }
        }
        wake.held_announce_release = self.held_announce_release_wake();
        if let Some(expiry) = destination_identity_expiry {
            wake.expired_destination_identities = WakeSchedule::AtMost(expiry);
        }
        if released_any {
            wake.scheduled_announces = self.scheduled_announces_wake();
            wake.path_request_timeouts = self.path_request_timeouts_wake();
            wake.expired_routes = self.route_expiry_wake(interfaces);
        }
        wake
    }

    fn next_due_held_interface(&self, now: InstantMillis) -> Option<InterfaceId> {
        self.held_announces.interfaces().find(|&interface| {
            self.interface_announce_limits
                .next_held_release_at(interface)
                .is_some_and(|release| release.0 <= now.0)
        })
    }
}
