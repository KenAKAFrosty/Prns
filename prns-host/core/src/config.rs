use alloc::string::String;
use alloc::vec::Vec;

use prns_runtime::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

use crate::{Capability, HostRole, PrnsLimits, RequestPolicy};

pub struct IdentitySecret(Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>);

impl IdentitySecret {
    #[must_use]
    pub fn new(bytes: [u8; IDENTITY_SECRET_KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    #[must_use]
    pub fn expose(&self) -> &[u8; IDENTITY_SECRET_KEY_LEN] {
        &self.0
    }
}

pub enum IdentityConfig {
    Existing(IdentitySecret),
    GenerateEphemeral,
    LoadOrCreate { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistenceConfig {
    Ephemeral,
    Directory { path: String },
}

pub enum DestinationIdentityConfig {
    HostIdentity,
    Dedicated(IdentityConfig),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationProofStrategy {
    ProveAll,
    ProveNone,
    ProveIf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationLinkRequestPolicy {
    AcceptAll,
    AcceptNone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationRatchetPolicy {
    NoRatchets,
    Ratcheted,
    RatchetsRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestinationName {
    app_name: String,
    aspects: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationNameError {
    EmptyAppName,
    EmptyAspects,
    EmptyAspect,
}

impl DestinationName {
    pub fn try_new(
        app_name: impl Into<String>,
        aspects: impl IntoIterator<Item = String>,
    ) -> Result<Self, DestinationNameError> {
        let app_name = app_name.into();
        if app_name.is_empty() {
            return Err(DestinationNameError::EmptyAppName);
        }
        let aspects: Vec<String> = aspects.into_iter().collect();
        if aspects.is_empty() {
            return Err(DestinationNameError::EmptyAspects);
        }
        if aspects.iter().any(String::is_empty) {
            return Err(DestinationNameError::EmptyAspect);
        }
        Ok(Self { app_name, aspects })
    }

    #[must_use]
    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    #[must_use]
    pub fn aspects(&self) -> &[String] {
        &self.aspects
    }
}

pub struct SingleDestinationConfig {
    pub name: DestinationName,
    pub identity: DestinationIdentityConfig,
    pub announce_app_data: Vec<u8>,
    pub maximum_request_bytes: Option<u64>,
    pub proof: DestinationProofStrategy,
    pub link_requests: DestinationLinkRequestPolicy,
    pub ratchet: DestinationRatchetPolicy,
    pub resource_strategy: crate::ResourceStrategy,
    pub request_handlers: Vec<RequestHandlerConfig>,
}

impl SingleDestinationConfig {
    /// Construct the canonical hosted destination with its shared policy defaults.
    /// Bindings set their supported optional fields on this value instead of
    /// maintaining independent policy defaults.
    #[must_use]
    pub fn new(name: DestinationName, identity: DestinationIdentityConfig) -> Self {
        Self {
            name,
            identity,
            announce_app_data: Vec::new(),
            maximum_request_bytes: None,
            proof: DestinationProofStrategy::ProveAll,
            link_requests: DestinationLinkRequestPolicy::AcceptAll,
            ratchet: DestinationRatchetPolicy::NoRatchets,
            resource_strategy: crate::ResourceStrategy::Refuse,
            request_handlers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestHandlerConfig {
    pub path: String,
    pub policy: RequestPolicy,
}

pub enum DestinationConfig {
    Plain(DestinationName),
    Single(SingleDestinationConfig),
}

pub struct HostConfig {
    pub identity: IdentityConfig,
    pub persistence: PersistenceConfig,
    pub role: HostRole,
    pub destinations: Vec<DestinationConfig>,
    pub required_capabilities: Vec<Capability>,
    pub limits: PrnsLimits,
}
