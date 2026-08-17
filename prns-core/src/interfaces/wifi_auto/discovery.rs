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

use super::{TCP_RENDEZVOUS_PORT, UNICAST_DISCOVERY_PORT};

pub const DNS_SD_LOCAL_DOMAIN: &str = "local.";
pub const TCP_DNS_SD_BASE_SERVICE_TYPE: &str = "_reticulum._tcp";
pub const TCP_DNS_SD_SERVICE_TYPE: &str = "_reticulum._tcp.local.";
pub const UDP_DNS_SD_BASE_SERVICE_TYPE: &str = "_reticulum._udp";
pub const UDP_DNS_SD_SERVICE_TYPE: &str = "_reticulum._udp.local.";
pub const TXT_VERSION_KEY: &str = "v";
pub const TXT_VERSION_VALUE: &str = "1";
pub const DEFAULT_DISCOVERY_SERVICE_CAPACITY: NonZeroU8 = NonZeroU8::MAX;
pub const SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY: u8 = 8;
pub const DISCOVERY_SERVICE_NAME_MAX_BYTES: usize = 255;
pub const EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiscoveryTransport {
    Tcp,
    Udp,
}

impl DiscoveryTransport {
    pub const fn port(self) -> u16 {
        match self {
            Self::Tcp => TCP_RENDEZVOUS_PORT,
            Self::Udp => UNICAST_DISCOVERY_PORT,
        }
    }

    pub const fn dns_sd_base_service_type(self) -> &'static str {
        match self {
            Self::Tcp => TCP_DNS_SD_BASE_SERVICE_TYPE,
            Self::Udp => UDP_DNS_SD_BASE_SERVICE_TYPE,
        }
    }

    pub const fn dns_sd_service_type(self) -> &'static str {
        match self {
            Self::Tcp => TCP_DNS_SD_SERVICE_TYPE,
            Self::Udp => UDP_DNS_SD_SERVICE_TYPE,
        }
    }
}

impl fmt::Display for DiscoveryTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp => formatter.write_str("TCP"),
            Self::Udp => formatter.write_str("UDP"),
        }
    }
}

/// A DNS-SD instance label derived only from fresh random session material.
///
/// The private representation deliberately offers no hostname-, device-, or
/// node-identity constructor. A publisher obtains fresh random bytes for each
/// transport whenever a Central publication session starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EphemeralDiscoveryInstanceName(String);

impl EphemeralDiscoveryInstanceName {
    pub fn from_random_bytes(
        random_bytes: [u8; EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
    ) -> Self {
        const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut value = String::from("prns-");
        for byte in random_bytes {
            value.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
            value.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
        }
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EphemeralDiscoveryInstanceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

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
pub struct DiscoveryServiceName {
    value: String,
    transport: DiscoveryTransport,
}

impl DiscoveryServiceName {
    pub fn from_fullname(
        value: impl Into<String>,
        transport: DiscoveryTransport,
    ) -> Result<Self, DiscoveryServiceNameError> {
        let mut value = value.into();
        if value.is_empty() {
            return Err(DiscoveryServiceNameError::Empty);
        }
        if value.len() > DISCOVERY_SERVICE_NAME_MAX_BYTES {
            return Err(DiscoveryServiceNameError::TooLong {
                actual: value.len(),
            });
        }
        value.make_ascii_lowercase();
        let expected_service_type = transport.dns_sd_service_type();
        let instance_prefix_bytes = value.len().saturating_sub(expected_service_type.len());
        if !value.ends_with(expected_service_type)
            || instance_prefix_bytes < 2
            || value.as_bytes()[instance_prefix_bytes - 1] != b'.'
        {
            return Err(DiscoveryServiceNameError::WrongServiceType {
                expected: expected_service_type,
            });
        }
        Ok(Self { value, transport })
    }

    pub fn from_instance(
        instance: &str,
        transport: DiscoveryTransport,
    ) -> Result<Self, DiscoveryServiceNameError> {
        if instance.is_empty() {
            return Err(DiscoveryServiceNameError::Empty);
        }
        Self::from_fullname(
            format!("{instance}.{}", transport.dns_sd_service_type()),
            transport,
        )
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub const fn transport(&self) -> DiscoveryTransport {
        self.transport
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
    WrongServiceType { expected: &'static str },
}

impl fmt::Display for DiscoveryServiceNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("discovery service name is empty"),
            Self::TooLong { actual } => write!(
                formatter,
                "discovery service name has {actual} bytes, exceeding {DISCOVERY_SERVICE_NAME_MAX_BYTES}"
            ),
            Self::WrongServiceType { expected } => {
                write!(formatter, "discovery service name is not a {expected} service")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryServiceNameError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiscoveryEndpoint {
    address: SocketAddr,
    transport: DiscoveryTransport,
}

impl DiscoveryEndpoint {
    pub fn tcp(address: SocketAddr) -> Result<Self, DiscoveryEndpointError> {
        if address.port() != TCP_RENDEZVOUS_PORT {
            return Err(DiscoveryEndpointError::WrongPort {
                expected: TCP_RENDEZVOUS_PORT,
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
        Ok(Self {
            address,
            transport: DiscoveryTransport::Tcp,
        })
    }

    pub fn udp(address: SocketAddr) -> Result<Self, DiscoveryEndpointError> {
        if address.port() != UNICAST_DISCOVERY_PORT {
            return Err(DiscoveryEndpointError::WrongPort {
                expected: UNICAST_DISCOVERY_PORT,
                actual: address.port(),
            });
        }
        let SocketAddr::V6(address) = address else {
            return Err(DiscoveryEndpointError::UdpRequiresIpv6);
        };
        if !address.ip().is_unicast_link_local() {
            return Err(DiscoveryEndpointError::UdpRequiresIpv6LinkLocal);
        }
        if address.flowinfo() != 0 {
            return Err(DiscoveryEndpointError::Ipv6FlowInfo);
        }
        if address.scope_id() == 0 {
            return Err(DiscoveryEndpointError::MissingIpv6Scope);
        }
        Ok(Self {
            address: SocketAddr::V6(address),
            transport: DiscoveryTransport::Udp,
        })
    }

    pub const fn transport(self) -> DiscoveryTransport {
        self.transport
    }

    pub const fn socket_addr(self) -> SocketAddr {
        self.address
    }

    pub const fn ip(self) -> IpAddr {
        self.socket_addr().ip()
    }

    fn preference(self) -> u8 {
        match self.ip() {
            IpAddr::V4(address) if address.is_private() => 0,
            IpAddr::V6(_) => {
                if matches!(
                    local_address_scope(self.ip()),
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
        self.transport()
            .cmp(&other.transport())
            .then_with(|| self.preference().cmp(&other.preference()))
            .then_with(|| self.socket_addr().cmp(&other.socket_addr()))
    }
}

impl fmt::Display for DiscoveryEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.socket_addr().fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryEndpointError {
    WrongPort { expected: u16, actual: u16 },
    Loopback,
    NonLocal,
    UdpRequiresIpv6,
    UdpRequiresIpv6LinkLocal,
    MissingIpv6Scope,
    UnexpectedIpv6Scope,
    Ipv6FlowInfo,
}

impl fmt::Display for DiscoveryEndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongPort { expected, actual } => {
                write!(
                    formatter,
                    "discovery endpoint port {actual} is not {expected}"
                )
            }
            Self::Loopback => formatter.write_str("discovery endpoint is loopback"),
            Self::NonLocal => formatter.write_str("discovery endpoint is not a local address"),
            Self::UdpRequiresIpv6 => {
                formatter.write_str("UDP discovery endpoint is not an IPv6 address")
            }
            Self::UdpRequiresIpv6LinkLocal => {
                formatter.write_str("UDP discovery endpoint is not IPv6 link-local")
            }
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
    ReplacedLowerPriority,
    RejectedLowerPriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateInsertionError {
    pub service_transport: DiscoveryTransport,
    pub endpoint_transport: DiscoveryTransport,
}

impl fmt::Display for CandidateInsertionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot insert a {} endpoint into a {} service advertisement",
            self.endpoint_transport, self.service_transport
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CandidateInsertionError {}

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

    pub fn insert(
        &mut self,
        endpoint: DiscoveryEndpoint,
    ) -> Result<CandidateInsertion, CandidateInsertionError> {
        if self.service.transport() != endpoint.transport() {
            return Err(CandidateInsertionError {
                service_transport: self.service.transport(),
                endpoint_transport: endpoint.transport(),
            });
        }
        Ok(match self.endpoints.binary_search(&endpoint) {
            Ok(_) => CandidateInsertion::AlreadyPresent,
            Err(index) => {
                let capacity = usize::from(SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY);
                if self.endpoints.len() == capacity {
                    if index == capacity {
                        return Ok(CandidateInsertion::RejectedLowerPriority);
                    }
                    self.endpoints.insert(index, endpoint);
                    self.endpoints.pop();
                    return Ok(CandidateInsertion::ReplacedLowerPriority);
                }
                self.endpoints.insert(index, endpoint);
                CandidateInsertion::Inserted
            }
        })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertisementRemoval {
    Removed,
    NotPresent,
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

    pub fn remove(&mut self, service: &DiscoveryServiceName) -> AdvertisementRemoval {
        match self.advertisements.remove(service) {
            Some(_removed_advertisement) => AdvertisementRemoval::Removed,
            None => AdvertisementRemoval::NotPresent,
        }
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

impl TryFrom<(DiscoveryTransport, SocketAddr)> for DiscoveryEndpoint {
    type Error = DiscoveryEndpointError;

    fn try_from(
        (transport, address): (DiscoveryTransport, SocketAddr),
    ) -> Result<Self, Self::Error> {
        match transport {
            DiscoveryTransport::Tcp => Self::tcp(address),
            DiscoveryTransport::Udp => Self::udp(address),
        }
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

    fn tcp_endpoint(value: &str) -> DiscoveryEndpoint {
        DiscoveryEndpoint::tcp(value.parse().expect("test endpoint parses"))
            .expect("test TCP endpoint is valid")
    }

    fn udp_endpoint(address_suffix: u16, scope_id: u32) -> DiscoveryEndpoint {
        DiscoveryEndpoint::udp(SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, address_suffix),
            UNICAST_DISCOVERY_PORT,
            0,
            scope_id,
        )))
        .expect("test UDP endpoint is valid")
    }

    fn advertisement(
        instance: &str,
        transport: DiscoveryTransport,
        endpoint: DiscoveryEndpoint,
    ) -> ServiceAdvertisement {
        let service = DiscoveryServiceName::from_instance(instance, transport)
            .expect("test service name is valid");
        let mut advertisement = ServiceAdvertisement::new(service);
        assert_eq!(
            advertisement.insert(endpoint),
            Ok(CandidateInsertion::Inserted)
        );
        advertisement
    }

    #[test]
    fn tcp_endpoint_validation_keeps_the_fixed_local_contract() {
        assert_eq!(
            DiscoveryEndpoint::tcp("192.168.1.9:7".parse().unwrap()),
            Err(DiscoveryEndpointError::WrongPort {
                expected: TCP_RENDEZVOUS_PORT,
                actual: 7,
            })
        );
        assert_eq!(
            DiscoveryEndpoint::tcp("127.0.0.1:42699".parse().unwrap()),
            Err(DiscoveryEndpointError::Loopback)
        );
        assert_eq!(
            DiscoveryEndpoint::tcp("8.8.8.8:42699".parse().unwrap()),
            Err(DiscoveryEndpointError::NonLocal)
        );
        assert_eq!(
            DiscoveryEndpoint::tcp("[fe80::1]:42699".parse().unwrap()),
            Err(DiscoveryEndpointError::MissingIpv6Scope)
        );
        let scoped = SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
            TCP_RENDEZVOUS_PORT,
            0,
            4,
        ));
        assert!(DiscoveryEndpoint::tcp(scoped).is_ok());
    }

    #[test]
    fn udp_endpoint_validation_requires_the_reverse_discovery_contract() {
        assert_eq!(
            DiscoveryEndpoint::udp("192.168.1.9:29717".parse().unwrap()),
            Err(DiscoveryEndpointError::UdpRequiresIpv6)
        );
        assert_eq!(
            DiscoveryEndpoint::udp("[fd00::1]:29717".parse().unwrap()),
            Err(DiscoveryEndpointError::UdpRequiresIpv6LinkLocal)
        );
        assert_eq!(
            DiscoveryEndpoint::udp("[fe80::1]:29717".parse().unwrap()),
            Err(DiscoveryEndpointError::MissingIpv6Scope)
        );
        assert_eq!(
            DiscoveryEndpoint::udp(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
                TCP_RENDEZVOUS_PORT,
                0,
                4,
            ))),
            Err(DiscoveryEndpointError::WrongPort {
                expected: UNICAST_DISCOVERY_PORT,
                actual: TCP_RENDEZVOUS_PORT,
            })
        );
        assert_eq!(udp_endpoint(1, 4).transport(), DiscoveryTransport::Udp);
    }

    #[test]
    fn candidate_selection_is_bounded_deduplicated_and_order_independent() {
        let service = DiscoveryServiceName::from_instance("peer", DiscoveryTransport::Tcp).unwrap();
        let mut advertisement = ServiceAdvertisement::new(service);
        let candidates: Vec<_> = (1..=10)
            .map(|suffix| tcp_endpoint(&format!("192.168.1.{suffix}:42699")))
            .collect();
        for candidate in candidates.iter().rev().copied() {
            advertisement
                .insert(candidate)
                .expect("candidate transport matches the advertisement");
        }
        let expected = candidates[..usize::from(SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY)].to_vec();
        assert_eq!(advertisement.endpoints(), expected);
        assert_eq!(
            advertisement.insert(candidates[0]),
            Ok(CandidateInsertion::AlreadyPresent)
        );

        let mut rotated = ServiceAdvertisement::new(advertisement.service().clone());
        let mut replacement_count = 0;
        for candidate in candidates[3..]
            .iter()
            .chain(candidates[..3].iter())
            .copied()
        {
            let insertion = rotated
                .insert(candidate)
                .expect("candidate transport matches the advertisement");
            if insertion == CandidateInsertion::ReplacedLowerPriority {
                replacement_count += 1;
            }
        }
        assert_eq!(replacement_count, 2);
        assert_eq!(rotated.endpoints(), expected);
        assert_eq!(
            rotated.insert(tcp_endpoint("192.168.1.99:42699")),
            Ok(CandidateInsertion::RejectedLowerPriority)
        );
    }

    #[test]
    fn a_service_only_accepts_endpoints_for_its_transport() {
        let service = DiscoveryServiceName::from_instance("peer", DiscoveryTransport::Tcp).unwrap();
        let mut advertisement = ServiceAdvertisement::new(service);
        assert_eq!(
            advertisement.insert(udp_endpoint(1, 4)),
            Err(CandidateInsertionError {
                service_transport: DiscoveryTransport::Tcp,
                endpoint_transport: DiscoveryTransport::Udp,
            })
        );
        assert!(advertisement.is_empty());
    }

    #[test]
    fn one_capacity_bounds_tcp_and_udp_records_together() {
        let capacity = DEFAULT_DISCOVERY_SERVICE_CAPACITY;
        let mut snapshot = DiscoverySnapshot::new(capacity);
        let first_tcp_endpoint = tcp_endpoint("10.0.0.2:42699");
        let first_udp_endpoint = udp_endpoint(2, 4);
        for index in 0..128u8 {
            assert_eq!(
                snapshot.insert(advertisement(
                    &format!("tcp-peer-{index}"),
                    DiscoveryTransport::Tcp,
                    first_tcp_endpoint,
                )),
                AdvertisementInsertion::Inserted
            );
        }
        for index in 0..127u8 {
            assert_eq!(
                snapshot.insert(advertisement(
                    &format!("udp-peer-{index}"),
                    DiscoveryTransport::Udp,
                    first_udp_endpoint,
                )),
                AdvertisementInsertion::Inserted
            );
        }
        assert_eq!(
            snapshot.insert(advertisement(
                "overflow",
                DiscoveryTransport::Udp,
                first_udp_endpoint,
            )),
            AdvertisementInsertion::AtCapacity
        );
        assert_eq!(
            snapshot.insert(advertisement(
                "tcp-peer-0",
                DiscoveryTransport::Tcp,
                tcp_endpoint("10.0.0.3:42699"),
            )),
            AdvertisementInsertion::Replaced
        );
        assert_eq!(snapshot.len(), usize::from(capacity.get()));
        assert_eq!(snapshot.capacity(), capacity);

        let removed =
            DiscoveryServiceName::from_instance("udp-peer-0", DiscoveryTransport::Udp).unwrap();
        assert_eq!(snapshot.remove(&removed), AdvertisementRemoval::Removed);
        assert_eq!(
            snapshot.insert(advertisement(
                "new-udp-peer",
                DiscoveryTransport::Udp,
                first_udp_endpoint,
            )),
            AdvertisementInsertion::Inserted
        );
    }

    #[test]
    fn default_discovery_capacity_is_the_nonzero_u8_maximum() {
        let capacity = DEFAULT_DISCOVERY_SERVICE_CAPACITY;
        let snapshot = DiscoverySnapshot::new(capacity);
        assert_eq!(snapshot.capacity(), NonZeroU8::MAX);
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
    fn service_instances_have_transport_specific_dns_sd_identities() {
        assert_eq!(
            DiscoveryServiceName::from_instance("peer", DiscoveryTransport::Tcp)
                .unwrap()
                .as_str(),
            "peer._reticulum._tcp.local."
        );
        assert_eq!(
            DiscoveryServiceName::from_instance("peer", DiscoveryTransport::Udp)
                .unwrap()
                .as_str(),
            "peer._reticulum._udp.local."
        );
        assert_ne!(
            DiscoveryServiceName::from_instance("peer", DiscoveryTransport::Tcp).unwrap(),
            DiscoveryServiceName::from_instance("peer", DiscoveryTransport::Udp).unwrap()
        );
        assert_eq!(
            DiscoveryServiceName::from_instance("", DiscoveryTransport::Tcp),
            Err(DiscoveryServiceNameError::Empty)
        );
        assert_eq!(
            DiscoveryServiceName::from_fullname(
                "peer._reticulum._udp.local.",
                DiscoveryTransport::Tcp,
            ),
            Err(DiscoveryServiceNameError::WrongServiceType {
                expected: TCP_DNS_SD_SERVICE_TYPE,
            })
        );
    }

    #[test]
    fn publication_names_are_derived_only_from_session_randomness() {
        let tcp_name = EphemeralDiscoveryInstanceName::from_random_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        ]);
        let udp_name = EphemeralDiscoveryInstanceName::from_random_bytes([
            0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
        ]);
        assert_eq!(tcp_name.as_str(), "prns-0123456789abcdef");
        assert_eq!(udp_name.as_str(), "prns-fedcba9876543210");
        assert_ne!(tcp_name, udp_name);
    }
}
