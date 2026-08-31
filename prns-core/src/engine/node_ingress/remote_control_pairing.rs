use super::journal_route_removal;
use crate::engine::{EngineReaction, EngineState, Journaled, WakeSchedule, WakeSchedules};
use crate::interfaces::AttachedInterfaces;
use crate::remote_control::{
    RemoteControlPairingAvailabilityObservation, RemoteControlPairingAvailabilityVerifyOwed,
};
use crate::routing::ingress::{
    IngestEffects, IngestRemoteControlPairingAvailability, RemoteControlPairingAvailabilityArrival,
};
use crate::storage::StorageLayout;

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn apply_remote_control_pairing_availability<Work>(
        &mut self,
        observation: RemoteControlPairingAvailabilityObservation<'_>,
        interfaces: AttachedInterfaces<'_>,
        wake: &mut WakeSchedules,
        sink: &mut impl FnMut(EngineReaction<'_, Work>),
    ) {
        let destination = observation.endpoint().destination_hash();
        sink(EngineReaction::Journaled(
            Journaled::RemoteControlPairingAvailabilityObserved(observation),
        ));
        wake.expired_routes = self
            .routing_table
            .existing_route_for(&destination, interfaces)
            .map_or(WakeSchedule::Unchanged, |route| {
                WakeSchedule::AtMost(route.expires_at)
            });
    }

    pub fn resume_remote_control_pairing_availability(
        &mut self,
        owed: RemoteControlPairingAvailabilityVerifyOwed,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let mut wake = WakeSchedules::UNCHANGED;
        let Ok(availability) = owed.parse_verified() else {
            return wake;
        };
        let mut effects = IngestEffects::default();
        let outcome = self.ingest_verified_remote_control_pairing_availability(
            availability,
            RemoteControlPairingAvailabilityArrival {
                received_hops: owed.received_hops(),
                source_interface: owed.source_interface(),
                arrived_at: owed.observed_at(),
            },
            interfaces,
            &mut |removed| sink(EngineReaction::Journaled(journal_route_removal(removed))),
            &mut effects,
        );
        match outcome {
            IngestRemoteControlPairingAvailability::Observed => {}
            IngestRemoteControlPairingAvailability::Duplicate
            | IngestRemoteControlPairingAvailability::Blackholed => return wake,
        }
        if let Some(expiry) = effects.destination_identity_expiry {
            wake.expired_destination_identities = WakeSchedule::AtMost(expiry);
        }
        if let Some(observation) = effects.remote_control_pairing_availability {
            self.apply_remote_control_pairing_availability(
                observation,
                interfaces,
                &mut wake,
                sink,
            );
        }
        wake
    }
}
