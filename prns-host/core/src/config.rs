use alloc::string::String;
use alloc::vec::Vec;

use prns_runtime::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

use crate::{Capability, PrnsLimits};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostRole {
    Endpoint,
    Transport,
}

pub enum DestinationIdentityConfig {
    HostIdentity,
    Dedicated(IdentityConfig),
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
}

pub enum DestinationConfig {
    Plain(DestinationName),
    Single(SingleDestinationConfig),
}

pub struct HostConfig {
    pub identity: IdentityConfig,
    pub role: HostRole,
    pub destinations: Vec<DestinationConfig>,
    pub required_capabilities: Vec<Capability>,
    pub limits: PrnsLimits,
}
