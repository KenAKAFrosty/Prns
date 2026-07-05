use super::core::{AwareEndpoint, NdpRole, RendezvousToken};

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
pub enum NdpEndReason {
    PeerGone,
    LocalClosed,
    LinkLost,
    Revoked(&'static str),
}

pub enum WifiAwareEvent {
    PeerDiscovered {
        peer: RendezvousToken,
    },
    NdpRequested {
        peer: RendezvousToken,
    },
    DataPathUp {
        peer: RendezvousToken,
        role: NdpRole,
        endpoint: AwareEndpoint,
    },
    DataPathDown {
        peer: RendezvousToken,
        role: NdpRole,
        reason: NdpEndReason,
    },
    NdpFailed {
        peer: RendezvousToken,
        role: NdpRole,
    },
    AvailabilityChanged(Availability),
}

#[allow(async_fn_in_trait)]
pub trait WifiAwareBackend {
    type Error: core::fmt::Debug;

    fn blocked(&self) -> Option<&'static str> {
        None
    }

    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), Self::Error>;
    async fn request_data_path(&mut self, peer: RendezvousToken, role: NdpRole);
    async fn abandon_data_path(&mut self, peer: RendezvousToken);
    async fn next_event(&mut self) -> WifiAwareEvent;
}
