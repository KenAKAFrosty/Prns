//! Allocation-free DNS-SD constants shared by every AutoWifi host.

use core::fmt;

use super::{TCP_RENDEZVOUS_PORT, UNICAST_DISCOVERY_PORT};

pub const DNS_SD_LOCAL_DOMAIN: &str = "local.";
pub const TCP_DNS_SD_BASE_SERVICE_TYPE: &str = "_reticulum._tcp";
pub const TCP_DNS_SD_SERVICE_TYPE: &str = "_reticulum._tcp.local.";
pub const UDP_DNS_SD_BASE_SERVICE_TYPE: &str = "_reticulum._udp";
pub const UDP_DNS_SD_SERVICE_TYPE: &str = "_reticulum._udp.local.";
pub const TXT_VERSION_KEY: &str = "v";
pub const TXT_VERSION_VALUE: &str = "1";
pub const EPHEMERAL_DISCOVERY_INSTANCE_PREFIX: &str = "prns-";
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
