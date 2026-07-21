use super::protocol::{DataPlanePlan, GoIntent, GroupRole, Initiative, PeerEvidence};
use crate::interfaces::MacAddress;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    Unavailable(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMode {
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupEndReason {
    PeerRemoved,
    LocalRemoved,
    LinkLost,
    Revoked(&'static str),
}

pub enum WifiDirectEvent<G> {
    Sighting {
        peer: MacAddress,
        evidence: PeerEvidence,
        initiative: Initiative,
    },
    PeerGone {
        peer: MacAddress,
    },
    Invitation {
        peer: MacAddress,
    },
    GroupOffer {
        peer: MacAddress,
    },
    GroupFormed {
        group: G,
    },
    GroupLost {
        reason: GroupEndReason,
    },
    FormationFailed {
        peer: MacAddress,
    },
    FormationProgress,
    AvailabilityChanged(Availability),
}

#[allow(async_fn_in_trait)]
pub trait WifiDirectBackend {
    type Error: core::fmt::Debug;
    type Group: WifiDirectGroup;

    fn blocked(&self) -> Option<&'static str> {
        None
    }

    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), Self::Error>;
    async fn form_group(&mut self, peer: MacAddress, intent: GoIntent);
    async fn accept_invitation(&mut self, peer: MacAddress, intent: GoIntent);
    async fn join_group(&mut self, _peer: MacAddress) {}
    async fn remove_group(&mut self);
    async fn next_event(&mut self) -> WifiDirectEvent<Self::Group>;
}

pub trait WifiDirectGroup {
    fn role(&self) -> GroupRole;
    fn data_plane(&self) -> DataPlanePlan;
}
