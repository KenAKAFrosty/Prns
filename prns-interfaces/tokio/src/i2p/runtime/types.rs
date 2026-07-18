use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use prns_core::interfaces::EffectiveInterfacePolicy;

use super::super::destination::I2pDestinationKeyPath;
use super::super::sam::{I2pAddress, I2pPublicDestination, SamValueError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pInterfaceName(String);

impl I2pInterfaceName {
    pub fn new(value: impl Into<String>) -> Result<Self, I2pInterfaceNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(I2pInterfaceNameError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pInterfaceNameError {
    Empty,
}

impl fmt::Display for I2pInterfaceNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("I2P interface name is empty")
    }
}

impl std::error::Error for I2pInterfaceNameError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum I2pPeerAddress {
    Named(I2pAddress),
    Destination(I2pPublicDestination),
}

impl I2pPeerAddress {
    pub fn new(value: impl Into<String>) -> Result<Self, I2pPeerAddressError> {
        let value = value.into();
        if value.ends_with(".i2p") {
            return I2pAddress::new(value)
                .map(Self::Named)
                .map_err(I2pPeerAddressError::Name);
        }
        I2pPublicDestination::new(value)
            .map(Self::Destination)
            .map_err(I2pPeerAddressError::Destination)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Named(value) => value.as_str(),
            Self::Destination(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum I2pPeerAddressError {
    Name(SamValueError),
    Destination(SamValueError),
}

impl fmt::Display for I2pPeerAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(error) => write!(formatter, "invalid I2P peer name: {error}"),
            Self::Destination(error) => write!(formatter, "invalid I2P peer destination: {error}"),
        }
    }
}

impl std::error::Error for I2pPeerAddressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Name(error) | Self::Destination(error) => Some(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pPeers(Vec<I2pPeerAddress>);

impl I2pPeers {
    pub fn new(peers: impl IntoIterator<Item = I2pPeerAddress>) -> Result<Self, DuplicateI2pPeer> {
        let mut unique = BTreeSet::new();
        let mut ordered = Vec::new();
        for peer in peers {
            if !unique.insert(peer.clone()) {
                return Err(DuplicateI2pPeer { peer });
            }
            ordered.push(peer);
        }
        Ok(Self(ordered))
    }

    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn iter(&self) -> impl Iterator<Item = &I2pPeerAddress> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateI2pPeer {
    pub peer: I2pPeerAddress,
}

impl fmt::Display for DuplicateI2pPeer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "I2P peer {} is configured more than once",
            self.peer.as_str()
        )
    }
}

impl std::error::Error for DuplicateI2pPeer {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum I2pReachability {
    OutboundOnly,
    Connectable { key_path: I2pDestinationKeyPath },
}

impl I2pReachability {
    pub fn is_connectable(&self) -> bool {
        matches!(self, Self::Connectable { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct I2pRetryPolicy {
    tunnel_setup_interval: Duration,
    peer_reconnect_interval: Duration,
    endpoint_retry_interval: Duration,
}

impl I2pRetryPolicy {
    pub const STOCK: Self = Self {
        tunnel_setup_interval: Duration::from_secs(8),
        peer_reconnect_interval: Duration::from_secs(15),
        endpoint_retry_interval: Duration::from_secs(15),
    };

    pub fn new(
        tunnel_setup_interval: Duration,
        peer_reconnect_interval: Duration,
        endpoint_retry_interval: Duration,
    ) -> Result<Self, I2pRetryPolicyError> {
        if tunnel_setup_interval.is_zero() {
            return Err(I2pRetryPolicyError::TunnelSetupZero);
        }
        if peer_reconnect_interval.is_zero() {
            return Err(I2pRetryPolicyError::PeerReconnectZero);
        }
        if endpoint_retry_interval.is_zero() {
            return Err(I2pRetryPolicyError::EndpointRetryZero);
        }
        Ok(Self {
            tunnel_setup_interval,
            peer_reconnect_interval,
            endpoint_retry_interval,
        })
    }

    pub(crate) fn tunnel_setup_interval(self) -> Duration {
        self.tunnel_setup_interval
    }

    pub(crate) fn peer_reconnect_interval(self) -> Duration {
        self.peer_reconnect_interval
    }

    pub(crate) fn endpoint_retry_interval(self) -> Duration {
        self.endpoint_retry_interval
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2pRetryPolicyError {
    TunnelSetupZero,
    PeerReconnectZero,
    EndpointRetryZero,
}

impl fmt::Display for I2pRetryPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TunnelSetupZero => formatter.write_str("I2P tunnel setup interval is zero"),
            Self::PeerReconnectZero => formatter.write_str("I2P peer reconnect interval is zero"),
            Self::EndpointRetryZero => formatter.write_str("I2P endpoint retry interval is zero"),
        }
    }
}

impl std::error::Error for I2pRetryPolicyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pRuntimeConfig {
    pub name: I2pInterfaceName,
    pub peers: I2pPeers,
    pub reachability: I2pReachability,
    pub policy: EffectiveInterfacePolicy,
    pub retry: I2pRetryPolicy,
}
