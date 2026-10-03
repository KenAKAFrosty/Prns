use prns_core::interfaces::InterfaceId;
use prns_core::remote_control::{
    RemoteControlPairingAvailabilityObservation, RemoteControlPairingEndpoint,
};
use prns_core::units::InstantMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingCandidate {
    endpoint: RemoteControlPairingEndpoint,
    source_interface: InterfaceId,
    observed_at: InstantMillis,
    expires_at: InstantMillis,
}

impl PairingCandidate {
    pub const fn endpoint(self) -> RemoteControlPairingEndpoint {
        self.endpoint
    }

    pub const fn source_interface(self) -> InterfaceId {
        self.source_interface
    }

    pub const fn observed_at(self) -> InstantMillis {
        self.observed_at
    }

    pub const fn expires_at(self) -> InstantMillis {
        self.expires_at
    }
}

impl From<&RemoteControlPairingAvailabilityObservation<'_>> for PairingCandidate {
    fn from(observation: &RemoteControlPairingAvailabilityObservation<'_>) -> Self {
        Self {
            endpoint: observation.endpoint(),
            source_interface: observation.source_interface(),
            observed_at: observation.observed_at(),
            expires_at: observation.expires_at(),
        }
    }
}
