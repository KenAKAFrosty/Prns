use super::announce::DirectAnnounceIngest;
use super::classification::DataPacket;
use super::dispatch::IngressCryptoMode;
use super::outcome::{IgnoreReason, IngestEffects, IngestPacketOutcome};
use crate::engine::{EngineState, InstantMillis};
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::remote_control::{
    RemoteControlPairingAvailability, RemoteControlPairingAvailabilityParseError,
    RemoteControlPairingAvailabilityVerifyOwed, RemoteControlPairingView,
};
use crate::routing::announce::{AnnounceArrival, AnnounceValidationError};
use crate::routing::{NextHop, RemovedRoute};
use crate::storage::StorageLayout;
use crate::wire::{ContextFlag, PropagationType, WireContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IngestRemoteControlPairingAvailability {
    Observed,
    Duplicate,
    Blackholed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RemoteControlPairingAvailabilityArrival {
    pub(crate) received_hops: u8,
    pub(crate) source_interface: InterfaceId,
    pub(crate) arrived_at: InstantMillis,
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn ingest_remote_control_pairing_availability<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrival: RemoteControlPairingAvailabilityArrival,
        crypto: IngressCryptoMode,
        _interfaces: AttachedInterfaces<'_>,
        _effects: &mut IngestEffects<'p>,
    ) -> IngestPacketOutcome<'p> {
        let RemoteControlPairingAvailabilityArrival {
            received_hops,
            source_interface,
            arrived_at,
        } = arrival;
        match self.remote_control_pairing_view() {
            RemoteControlPairingView::Unavailable => {
                return IngestPacketOutcome::Ignored(IgnoreReason::NotForUs)
            }
            RemoteControlPairingView::Closed | RemoteControlPairingView::Open(_) => {}
        }
        if data.header.hops != 0 {
            return IngestPacketOutcome::Ignored(IgnoreReason::HopLimitReached);
        }
        if data.header.context_flag != ContextFlag::Unset
            || data.header.propagation != PropagationType::Broadcast
            || data.header.transport_id.is_some()
        {
            return IngestPacketOutcome::Ignored(IgnoreReason::Malformed);
        }
        if data.header.context != WireContext::None {
            return IngestPacketOutcome::Ignored(IgnoreReason::UnhandledContext);
        }

        match crypto {
            IngressCryptoMode::Owed => {
                if let Err(error) =
                    RemoteControlPairingAvailability::parse_without_signature_verification(
                        data.payload,
                    )
                {
                    return IngestPacketOutcome::Ignored(pairing_availability_ignore_reason(error));
                }
                let owed = match RemoteControlPairingAvailabilityVerifyOwed::new(
                    data.payload,
                    arrived_at,
                    source_interface,
                    received_hops,
                ) {
                    Ok(owed) => owed,
                    Err(_) => return IngestPacketOutcome::Ignored(IgnoreReason::CapacityExhausted),
                };
                IngestPacketOutcome::OwesRemoteControlPairingAvailabilityVerify(owed)
            }
            #[cfg(test)]
            IngressCryptoMode::Inline => {
                let availability = match RemoteControlPairingAvailability::parse(data.payload) {
                    Ok(availability) => availability,
                    Err(error) => {
                        return IngestPacketOutcome::Ignored(pairing_availability_ignore_reason(
                            error,
                        ))
                    }
                };
                pairing_availability_outcome(
                    self.ingest_verified_remote_control_pairing_availability(
                        availability,
                        arrival,
                        _interfaces,
                        &mut |_| {},
                        _effects,
                    ),
                )
            }
        }
    }

    pub(crate) fn ingest_verified_remote_control_pairing_availability<'p>(
        &mut self,
        availability: RemoteControlPairingAvailability<'p>,
        arrival: RemoteControlPairingAvailabilityArrival,
        interfaces: AttachedInterfaces<'_>,
        on_removed: &mut impl FnMut(RemovedRoute),
        effects: &mut IngestEffects<'p>,
    ) -> IngestRemoteControlPairingAvailability {
        let RemoteControlPairingAvailabilityArrival {
            received_hops,
            source_interface,
            arrived_at,
        } = arrival;
        let identity_hash = availability.pairing_identity().identity_hash();
        let expires_after = availability.expires_after().into();
        let (observation, announce) =
            availability.into_observation(arrived_at, source_interface, received_hops);
        let arrival = AnnounceArrival {
            announce,
            hops: received_hops,
            arrived_at,
            receiving_interface: source_interface,
            next_hop: NextHop::Direct,
            is_path_response: false,
        };
        match self.ingest_direct_announce(
            identity_hash,
            &arrival,
            expires_after,
            interfaces,
            on_removed,
            effects,
        ) {
            DirectAnnounceIngest::Accepted => {
                effects.remote_control_pairing_availability = Some(observation);
                IngestRemoteControlPairingAvailability::Observed
            }
            DirectAnnounceIngest::Ignored => IngestRemoteControlPairingAvailability::Duplicate,
            DirectAnnounceIngest::Blackholed => IngestRemoteControlPairingAvailability::Blackholed,
        }
    }
}

#[cfg(test)]
fn pairing_availability_outcome<'p>(
    outcome: IngestRemoteControlPairingAvailability,
) -> IngestPacketOutcome<'p> {
    match outcome {
        IngestRemoteControlPairingAvailability::Observed => {
            IngestPacketOutcome::Ignored(IgnoreReason::Consumed)
        }
        IngestRemoteControlPairingAvailability::Duplicate => {
            IngestPacketOutcome::Ignored(IgnoreReason::Duplicate)
        }
        IngestRemoteControlPairingAvailability::Blackholed => {
            IngestPacketOutcome::Ignored(IgnoreReason::PermissionDenied)
        }
    }
}

fn pairing_availability_ignore_reason(
    error: RemoteControlPairingAvailabilityParseError,
) -> IgnoreReason {
    match error {
        RemoteControlPairingAvailabilityParseError::InnerAnnounce(
            AnnounceValidationError::InvalidSignature,
        ) => IgnoreReason::ProofInvalid,
        RemoteControlPairingAvailabilityParseError::PayloadTooLong { .. }
        | RemoteControlPairingAvailabilityParseError::EnvelopeHeader(_)
        | RemoteControlPairingAvailabilityParseError::InnerWire(_)
        | RemoteControlPairingAvailabilityParseError::InnerHeader(_)
        | RemoteControlPairingAvailabilityParseError::InnerAnnounce(
            AnnounceValidationError::NotAnnounce
            | AnnounceValidationError::NotSingleDestination
            | AnnounceValidationError::PayloadTooSmall
            | AnnounceValidationError::PayloadTooBig
            | AnnounceValidationError::DestinationMismatch,
        )
        | RemoteControlPairingAvailabilityParseError::UnexpectedDottedNameHash { .. }
        | RemoteControlPairingAvailabilityParseError::SignedHeader(_)
        | RemoteControlPairingAvailabilityParseError::HeaderMismatch { .. }
        | RemoteControlPairingAvailabilityParseError::SignedMetadataTruncated
        | RemoteControlPairingAvailabilityParseError::InvalidExpiresAfter(_)
        | RemoteControlPairingAvailabilityParseError::InvalidPublicAppData(_) => {
            IgnoreReason::Malformed
        }
    }
}
