//! Bounded, runtime-independent DNS-SD discovery contracts for AutoWifi.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;
use core::net::{IpAddr, SocketAddr};
use core::num::NonZeroU8;

use crate::interfaces::local_network::{local_address_scope, LocalAddressScope};

use super::TCP_RENDEZVOUS_PORT;

pub const DNS_SD_BASE_SERVICE_TYPE: &str = "_reticulum._tcp";
pub const DNS_SD_LOCAL_DOMAIN: &str = "local.";
pub const DNS_SD_SERVICE_TYPE: &str = "_reticulum._tcp.local.";
pub const TXT_VERSION_KEY: &str = "v";
pub const TXT_VERSION_VALUE: &str = "1";
pub const SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY: u8 = 8;
pub const DISCOVERY_SERVICE_NAME_MAX_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryVersion {
    ImplicitV1,
    ExplicitV1,
}

impl DiscoveryVersion {
    pub fn parse(value: Option<&[u8]>) -> Result<Self, DiscoveryVersionError> {
        let Some(value) = value else {
            return Ok(Self::ImplicitV1);
        };
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
            return Err(DiscoveryVersionError::Malformed);
        }
        let mut parsed = 0u32;
        for digit in value {
            parsed = parsed
                .checked_mul(10)
                .and_then(|number| number.checked_add(u32::from(*digit - b'0')))
                .ok_or(DiscoveryVersionError::Malformed)?;
        }
        match parsed {
            1 if value == TXT_VERSION_VALUE.as_bytes() => Ok(Self::ExplicitV1),
            1 => Err(DiscoveryVersionError::Malformed),
            unsupported => Err(DiscoveryVersionError::Unsupported(unsupported)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryVersionError {
    Malformed,
    Unsupported(u32),
}

impl fmt::Display for DiscoveryVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => formatter.write_str("discovery TXT version is malformed"),
            Self::Unsupported(version) => {
                write!(formatter, "discovery TXT version {version} is unsupported")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryVersionError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiscoveryServiceName(String);

impl DiscoveryServiceName {
    pub fn new(value: impl Into<String>) -> Result<Self, DiscoveryServiceNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DiscoveryServiceNameError::Empty);
        }
        if value.len() > DISCOVERY_SERVICE_NAME_MAX_BYTES {
            return Err(DiscoveryServiceNameError::TooLong {
                actual: value.len(),
            });
        }
        Ok(Self(value))
    }

    pub fn from_instance(instance: &str) -> Result<Self, DiscoveryServiceNameError> {
        if instance.is_empty() {
            return Err(DiscoveryServiceNameError::Empty);
        }
        Self::new(format!("{instance}.{DNS_SD_SERVICE_TYPE}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DiscoveryServiceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryServiceNameError {
    Empty,
    TooLong { actual: usize },
}

impl fmt::Display for DiscoveryServiceNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("discovery service name is empty"),
            Self::TooLong { actual } => write!(
                formatter,
                "discovery service name has {actual} bytes, exceeding {DISCOVERY_SERVICE_NAME_MAX_BYTES}"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryServiceNameError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiscoveryEndpoint(SocketAddr);

impl DiscoveryEndpoint {
    pub fn new(address: SocketAddr) -> Result<Self, DiscoveryEndpointError> {
        if address.port() != TCP_RENDEZVOUS_PORT {
            return Err(DiscoveryEndpointError::WrongPort {
                actual: address.port(),
            });
        }
        let scope = local_address_scope(address.ip()).ok_or(DiscoveryEndpointError::NonLocal)?;
        if scope == LocalAddressScope::Loopback {
            return Err(DiscoveryEndpointError::Loopback);
        }
        if let SocketAddr::V6(address) = address {
            if address.flowinfo() != 0 {
                return Err(DiscoveryEndpointError::Ipv6FlowInfo);
            }
            if address.ip().is_unicast_link_local() && address.scope_id() == 0 {
                return Err(DiscoveryEndpointError::MissingIpv6Scope);
            }
            if !address.ip().is_unicast_link_local() && address.scope_id() != 0 {
                return Err(DiscoveryEndpointError::UnexpectedIpv6Scope);
            }
        }
        Ok(Self(address))
    }

    pub const fn socket_addr(self) -> SocketAddr {
        self.0
    }

    pub const fn ip(self) -> IpAddr {
        self.0.ip()
    }

    fn preference(self) -> u8 {
        match self.0.ip() {
            IpAddr::V4(address) if address.is_private() => 0,
            IpAddr::V6(_) => {
                if matches!(
                    local_address_scope(self.0.ip()),
                    Some(LocalAddressScope::Private)
                ) {
                    1
                } else {
                    3
                }
            }
            IpAddr::V4(_) => 2,
        }
    }
}

impl PartialOrd for DiscoveryEndpoint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DiscoveryEndpoint {
    fn cmp(&self, other: &Self) -> Ordering {
        self.preference()
            .cmp(&other.preference())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl fmt::Display for DiscoveryEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryEndpointError {
    WrongPort { actual: u16 },
    Loopback,
    NonLocal,
    MissingIpv6Scope,
    UnexpectedIpv6Scope,
    Ipv6FlowInfo,
}

impl fmt::Display for DiscoveryEndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongPort { actual } => write!(
                formatter,
                "discovery endpoint port {actual} is not {TCP_RENDEZVOUS_PORT}"
            ),
            Self::Loopback => formatter.write_str("discovery endpoint is loopback"),
            Self::NonLocal => formatter.write_str("discovery endpoint is not a local address"),
            Self::MissingIpv6Scope => {
                formatter.write_str("IPv6 link-local discovery endpoint has no scope")
            }
            Self::UnexpectedIpv6Scope => {
                formatter.write_str("non-link-local discovery endpoint has an IPv6 scope")
            }
            Self::Ipv6FlowInfo => {
                formatter.write_str("discovery endpoint has nonzero IPv6 flow information")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryEndpointError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateInsertion {
    Inserted,
    AlreadyPresent,
    AtCapacity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAdvertisement {
    service: DiscoveryServiceName,
    endpoints: Vec<DiscoveryEndpoint>,
}

impl ServiceAdvertisement {
    pub const fn new(service: DiscoveryServiceName) -> Self {
        Self {
            service,
            endpoints: Vec::new(),
        }
    }

    pub fn service(&self) -> &DiscoveryServiceName {
        &self.service
    }

    pub fn endpoints(&self) -> &[DiscoveryEndpoint] {
        &self.endpoints
    }

    pub fn insert(&mut self, endpoint: DiscoveryEndpoint) -> CandidateInsertion {
        match self.endpoints.binary_search(&endpoint) {
            Ok(_) => CandidateInsertion::AlreadyPresent,
            Err(_)
                if self.endpoints.len()
                    == usize::from(SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY) =>
            {
                CandidateInsertion::AtCapacity
            }
            Err(index) => {
                self.endpoints.insert(index, endpoint);
                CandidateInsertion::Inserted
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertisementInsertion {
    Inserted,
    Replaced,
    AtCapacity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySnapshot {
    capacity: NonZeroU8,
    advertisements: BTreeMap<DiscoveryServiceName, ServiceAdvertisement>,
}

impl DiscoverySnapshot {
    pub const fn new(capacity: NonZeroU8) -> Self {
        Self {
            capacity,
            advertisements: BTreeMap::new(),
        }
    }

    pub const fn capacity(&self) -> NonZeroU8 {
        self.capacity
    }

    pub fn insert(&mut self, advertisement: ServiceAdvertisement) -> AdvertisementInsertion {
        if self.advertisements.contains_key(advertisement.service()) {
            self.advertisements
                .insert(advertisement.service().clone(), advertisement);
            return AdvertisementInsertion::Replaced;
        }
        if self.advertisements.len() >= usize::from(self.capacity.get()) {
            return AdvertisementInsertion::AtCapacity;
        }
        self.advertisements
            .insert(advertisement.service().clone(), advertisement);
        AdvertisementInsertion::Inserted
    }

    pub fn remove(&mut self, service: &DiscoveryServiceName) -> bool {
        self.advertisements.remove(service).is_some()
    }

    pub fn get(&self, service: &DiscoveryServiceName) -> Option<&ServiceAdvertisement> {
        self.advertisements.get(service)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ServiceAdvertisement> {
        self.advertisements.values()
    }

    pub fn len(&self) -> usize {
        self.advertisements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.advertisements.is_empty()
    }
}

impl From<DiscoveryEndpoint> for SocketAddr {
    fn from(value: DiscoveryEndpoint) -> Self {
        value.socket_addr()
    }
}

impl TryFrom<SocketAddr> for DiscoveryEndpoint {
    type Error = DiscoveryEndpointError;

    fn try_from(value: SocketAddr) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<&DiscoveryServiceName> for String {
    fn from(value: &DiscoveryServiceName) -> Self {
        value.as_str().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::net::{Ipv6Addr, SocketAddrV6};

    fn endpoint(value: &str) -> DiscoveryEndpoint {
        DiscoveryEndpoint::new(value.parse().expect("test endpoint parses"))
            .expect("test endpoint is valid")
    }

    fn advertisement(name: &str, endpoint: DiscoveryEndpoint) -> ServiceAdvertisement {
        let mut advertisement = ServiceAdvertisement::new(
            DiscoveryServiceName::new(name).expect("test service name is valid"),
        );
        assert_eq!(advertisement.insert(endpoint), CandidateInsertion::Inserted);
        advertisement
    }

    #[test]
    fn endpoint_validation_keeps_the_fixed_local_contract() {
        assert_eq!(
            DiscoveryEndpoint::new("192.168.1.9:7".parse().unwrap()),
            Err(DiscoveryEndpointError::WrongPort { actual: 7 })
        );
        assert_eq!(
            DiscoveryEndpoint::new("127.0.0.1:42699".parse().unwrap()),
            Err(DiscoveryEndpointError::Loopback)
        );
        assert_eq!(
            DiscoveryEndpoint::new("8.8.8.8:42699".parse().unwrap()),
            Err(DiscoveryEndpointError::NonLocal)
        );
        assert_eq!(
            DiscoveryEndpoint::new("[fe80::1]:42699".parse().unwrap()),
            Err(DiscoveryEndpointError::MissingIpv6Scope)
        );
        let scoped = SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
            TCP_RENDEZVOUS_PORT,
            0,
            4,
        ));
        assert!(DiscoveryEndpoint::new(scoped).is_ok());
    }

    #[test]
    fn candidates_are_deduplicated_bounded_and_preference_ordered() {
        let service = DiscoveryServiceName::new("peer._reticulum._tcp.local.").unwrap();
        let mut advertisement = ServiceAdvertisement::new(service);
        let v6_private = endpoint("[fd00::2]:42699");
        let v4_private = endpoint("192.168.1.2:42699");
        let v4_link_local = endpoint("169.254.1.2:42699");
        assert_eq!(
            advertisement.insert(v6_private),
            CandidateInsertion::Inserted
        );
        assert_eq!(
            advertisement.insert(v4_private),
            CandidateInsertion::Inserted
        );
        assert_eq!(
            advertisement.insert(v4_link_local),
            CandidateInsertion::Inserted
        );
        assert_eq!(
            advertisement.insert(v4_private),
            CandidateInsertion::AlreadyPresent
        );
        assert_eq!(
            advertisement.endpoints(),
            &[v4_private, v6_private, v4_link_local]
        );

        for tail in 3..=7 {
            assert_eq!(
                advertisement.insert(endpoint(&format!("192.168.1.{tail}:42699"))),
                CandidateInsertion::Inserted
            );
        }
        assert_eq!(
            advertisement.endpoints().len(),
            usize::from(SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY)
        );
        assert_eq!(
            advertisement.insert(endpoint("192.168.1.99:42699")),
            CandidateInsertion::AtCapacity
        );
    }

    #[test]
    fn snapshot_updates_known_services_when_full() {
        let capacity = NonZeroU8::new(3).unwrap();
        let mut snapshot = DiscoverySnapshot::new(capacity);
        let first_endpoint = endpoint("10.0.0.2:42699");
        for index in 0..capacity.get() {
            assert_eq!(
                snapshot.insert(advertisement(&format!("peer-{index}"), first_endpoint)),
                AdvertisementInsertion::Inserted
            );
        }
        assert_eq!(
            snapshot.insert(advertisement("overflow", first_endpoint)),
            AdvertisementInsertion::AtCapacity
        );
        assert_eq!(
            snapshot.insert(advertisement("peer-0", endpoint("10.0.0.3:42699"))),
            AdvertisementInsertion::Replaced
        );
        assert_eq!(snapshot.len(), usize::from(capacity.get()));
        assert_eq!(snapshot.capacity(), capacity);
    }

    #[test]
    fn platform_capacity_is_explicitly_bounded_by_its_type() {
        let capacity = NonZeroU8::new(u8::MAX).unwrap();
        let snapshot = DiscoverySnapshot::new(capacity);
        assert_eq!(snapshot.capacity(), capacity);
        assert_eq!(NonZeroU8::new(0), None);
    }

    #[test]
    fn versions_distinguish_implicit_compatible_and_explicit_incompatible() {
        assert_eq!(
            DiscoveryVersion::parse(None),
            Ok(DiscoveryVersion::ImplicitV1)
        );
        assert_eq!(
            DiscoveryVersion::parse(Some(b"1")),
            Ok(DiscoveryVersion::ExplicitV1)
        );
        assert_eq!(
            DiscoveryVersion::parse(Some(b"2")),
            Err(DiscoveryVersionError::Unsupported(2))
        );
        assert_eq!(
            DiscoveryVersion::parse(Some(b"v1")),
            Err(DiscoveryVersionError::Malformed)
        );
        assert_eq!(
            DiscoveryVersion::parse(Some(b"01")),
            Err(DiscoveryVersionError::Malformed)
        );
    }

    #[test]
    fn service_instances_have_one_canonical_dns_sd_identity() {
        assert_eq!(
            DiscoveryServiceName::from_instance("peer")
                .unwrap()
                .as_str(),
            "peer._reticulum._tcp.local."
        );
        assert_eq!(
            DiscoveryServiceName::from_instance(""),
            Err(DiscoveryServiceNameError::Empty)
        );
    }
}
