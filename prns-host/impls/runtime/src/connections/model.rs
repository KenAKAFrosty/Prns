use prns_host::{Bitrate, InterfaceConfig, InterfaceRoutingPolicy};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, num::NonZeroUsize, sync::Arc};

/// Caller-created stable identity. A save never invents IDs after admission, so
/// an uncertain save can be compared with the exact proposed profiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProfileId([u8; 16]);
impl ProfileId {
    pub fn new(bytes: [u8; 16]) -> Result<Self, Error> {
        if bytes == [0; 16] {
            return Err(Error::Invalid(InputError::InvalidId));
        }
        Ok(Self(bytes))
    }
    pub fn generate() -> Result<Self, Error> {
        Self::new(random_id()?)
    }
    pub fn bytes(self) -> [u8; 16] {
        self.0
    }
}
pub(super) fn random_id() -> Result<[u8; 16], Error> {
    let mut bytes = [0; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| Error::EntropyUnavailable)?;
    if bytes == [0; 16] {
        return Err(Error::EntropyUnavailable);
    }
    Ok(bytes)
}

/// The host configuration is authoritative. This module owns only persistent
/// identity, ordering, label and enabled intent around that configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: ProfileId,
    pub label: String,
    pub enabled: bool,
    pub config: InterfaceConfig,
    pub routing: Option<InterfaceRoutingPolicy>,
}

/// A different file incarnation cannot satisfy a stale revision guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guard {
    pub incarnation: [u8; 16],
    pub revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub guard: Guard,
    pub limits: Limits,
    pub profiles: Vec<Profile>,
}

/// Persisted logical capacity; physical cache and queue budgets are separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub profiles: u32,
    pub encoded_bytes: u32,
}
impl Limits {
    pub fn validate(self) -> Result<Self, Error> {
        if !(1..=256).contains(&self.profiles)
            || !(1024..=1024 * 1024).contains(&self.encoded_bytes)
        {
            return Err(Error::Invalid(InputError::InvalidLimits));
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub limits: Limits,
    pub queue_capacity: NonZeroUsize,
    pub cache_bytes: usize,
}
impl Options {
    pub(super) fn validate(self) -> Result<Self, Error> {
        self.limits.validate()?;
        if self.queue_capacity.get() > 1024
            || !(1024 * 1024..=1024 * 1024 * 1024).contains(&self.cache_bytes)
        {
            return Err(Error::Invalid(InputError::InvalidLimits));
        }
        Ok(self)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputError {
    InvalidLimits,
    InvalidId,
    InvalidLabel,
    InvalidTarget,
    InvalidConfig,
    UnsupportedConfig,
    DuplicateId,
    DuplicateEndpoint,
    MultipleLanProfiles,
    TooManyProfiles,
    EncodedCapacity,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    Missing,
    Corrupt,
    UnsupportedVersion(u32),
    CapacityMismatch,
    RevisionExhausted,
}
/// Storage errors retain their native cause. They never imply noncommit; read
/// the same owner again before deciding whether an uncertain save took effect.
#[derive(Clone, Debug)]
pub enum Error {
    Invalid(InputError),
    Schema(SchemaError),
    Conflict(Snapshot),
    Storage(Arc<redb::Error>),
    File(Arc<prns_storage::files::Error>),
    Spawn(Arc<std::io::Error>),
    EntropyUnavailable,
    Busy,
    Closed,
    Reentrant,
    WorkerFailed,
    AlreadyBound,
    BindingChanged,
    NotEnabled,
    Host(crate::owner::SessionError),
    Attachment(crate::attachment_scope::AttachmentError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "connection profiles: {self:?}")
    }
}
impl std::error::Error for Error {}
pub(super) fn storage(error: impl Into<redb::Error>) -> Error {
    Error::Storage(Arc::new(error.into()))
}

/// Normalize without resolving DNS or attempting a connection.
pub fn tcp_client_config(
    host: &str,
    port: u16,
    bitrate: Bitrate,
) -> Result<InterfaceConfig, Error> {
    if port == 0 {
        return Err(Error::Invalid(InputError::InvalidTarget));
    }
    let host = crate::attachment_scope::canonical_network_host(host)
        .ok_or(Error::Invalid(InputError::InvalidTarget))?;
    let ipv6 = host.contains(':');
    if matches!(bitrate, Bitrate::BitsPerSecond(0)) {
        return Err(Error::Invalid(InputError::InvalidConfig));
    }
    let target = if ipv6 {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    Ok(InterfaceConfig::TcpClient { target, bitrate })
}

/// Normalize a complete saved/editor endpoint through the same owning checks.
pub fn tcp_client_config_from_target(
    target: &str,
    bitrate: Bitrate,
) -> Result<InterfaceConfig, Error> {
    let (host, port) = crate::endpoint_parts(target.trim(), false)
        .map_err(|_| Error::Invalid(InputError::InvalidTarget))?;
    let host = host.ok_or(Error::Invalid(InputError::InvalidTarget))?;
    tcp_client_config(&host, port, bitrate)
}

pub(super) fn normalize(mut profiles: Vec<Profile>, limits: Limits) -> Result<Vec<Profile>, Error> {
    if profiles.len() > limits.profiles as usize {
        return Err(Error::Invalid(InputError::TooManyProfiles));
    }
    // Bound caller-owned nested allocations before normalization clones or JSON
    // encoding. Escaping/structural overhead is checked by the exact encoder.
    let mut input_bytes = 0_usize;
    let mut charge = |bytes: usize| -> Result<(), Error> {
        input_bytes = input_bytes
            .checked_add(bytes)
            .ok_or(Error::Invalid(InputError::EncodedCapacity))?;
        if input_bytes > limits.encoded_bytes as usize {
            return Err(Error::Invalid(InputError::EncodedCapacity));
        }
        Ok(())
    };
    for profile in &profiles {
        charge(profile.label.len())?;
        match &profile.config {
            InterfaceConfig::TcpClient { target, .. } => charge(target.len())?,
            InterfaceConfig::AutoLan {
                group_id,
                devices,
                ignored_devices,
                ..
            } => {
                charge(group_id.as_ref().map_or(0, String::len))?;
                charge(
                    devices
                        .len()
                        .checked_add(ignored_devices.len())
                        .and_then(|n| n.checked_mul(4))
                        .ok_or(Error::Invalid(InputError::EncodedCapacity))?,
                )?;
                for name in devices.iter().chain(ignored_devices) {
                    charge(name.len())?;
                }
            }
            _ => return Err(Error::Invalid(InputError::UnsupportedConfig)),
        }
    }
    let mut ids = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut lan = false;
    for profile in &mut profiles {
        ProfileId::new(profile.id.bytes())?;
        if !ids.insert(profile.id) {
            return Err(Error::Invalid(InputError::DuplicateId));
        }
        profile.label = profile.label.trim().to_owned();
        if profile.label.is_empty()
            || profile.label.len() > 256
            || profile.label.chars().any(char::is_control)
        {
            return Err(Error::Invalid(InputError::InvalidLabel));
        }
        profile
            .config
            .validate()
            .map_err(|_| Error::Invalid(InputError::InvalidConfig))?;
        match &profile.config {
            InterfaceConfig::TcpClient { target, bitrate } => {
                profile.config = tcp_client_config_from_target(target, *bitrate)?;
                let InterfaceConfig::TcpClient { target, .. } = &profile.config else {
                    unreachable!()
                };
                if !targets.insert(target.clone()) {
                    return Err(Error::Invalid(InputError::DuplicateEndpoint));
                }
            }
            InterfaceConfig::AutoLan { .. } => {
                // One installation-wide LAN discovery attachment. Selecting a
                // different LAN configuration replaces it after retirement.
                if lan {
                    return Err(Error::Invalid(InputError::MultipleLanProfiles));
                }
                lan = true;
            }
            _ => return Err(Error::Invalid(InputError::UnsupportedConfig)),
        }
    }
    Ok(profiles)
}
