use alloc::string::String;

use crate::wire::TransportId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdvertisedInterfaceType {
    Backbone,
    TcpServer,
    TcpClient,
    I2p,
    RNode,
    Weave,
    Kiss,
}

impl AdvertisedInterfaceType {
    pub const fn rns_name(self) -> &'static str {
        match self {
            Self::Backbone => "BackboneInterface",
            Self::TcpServer => "TCPServerInterface",
            Self::TcpClient => "TCPClientInterface",
            Self::I2p => "I2PInterface",
            Self::RNode => "RNodeInterface",
            Self::Weave => "WeaveInterface",
            Self::Kiss => "KISSInterface",
        }
    }

    pub fn from_rns_name(name: &str) -> Option<Self> {
        match name {
            "BackboneInterface" => Some(Self::Backbone),
            "TCPServerInterface" => Some(Self::TcpServer),
            "TCPClientInterface" => Some(Self::TcpClient),
            "I2PInterface" => Some(Self::I2p),
            "RNodeInterface" => Some(Self::RNode),
            "WeaveInterface" => Some(Self::Weave),
            "KISSInterface" => Some(Self::Kiss),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AdvertisedTransport {
    Enabled(TransportId),
    Disabled(TransportId),
}

impl AdvertisedTransport {
    pub const fn from_wire(enabled: bool, transport_id: TransportId) -> Self {
        if enabled {
            Self::Enabled(transport_id)
        } else {
            Self::Disabled(transport_id)
        }
    }

    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    pub const fn transport_id(&self) -> &TransportId {
        match self {
            Self::Enabled(transport_id) | Self::Disabled(transport_id) => transport_id,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct GeographicLocation {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub height: Option<f64>,
}

impl GeographicLocation {
    pub const UNKNOWN: Self = Self {
        latitude: None,
        longitude: None,
        height: None,
    };
}

#[derive(Debug, PartialEq, Eq)]
pub struct PublishedIfac {
    pub network_name: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum AdvertisementDetails {
    None,
    Reachable {
        host: String,
        port: u16,
    },
    I2p {
        address: String,
    },
    RNode {
        frequency_hz: u64,
        bandwidth_hz: u32,
        spreading_factor: u8,
        coding_rate: u8,
    },
    Weave {
        frequency_hz: u64,
        bandwidth_hz: u32,
        channel: u32,
        modulation: String,
    },
    Kiss {
        frequency_hz: u64,
        bandwidth_hz: u32,
        modulation: String,
    },
}

impl AdvertisementDetails {
    pub fn matches(&self, interface_type: AdvertisedInterfaceType) -> bool {
        matches!(
            (interface_type, self),
            (AdvertisedInterfaceType::Backbone, Self::Reachable { .. })
                | (AdvertisedInterfaceType::TcpServer, Self::Reachable { .. })
                | (AdvertisedInterfaceType::TcpClient, Self::None)
                | (AdvertisedInterfaceType::I2p, Self::I2p { .. })
                | (AdvertisedInterfaceType::RNode, Self::RNode { .. })
                | (AdvertisedInterfaceType::Weave, Self::Weave { .. })
                | (AdvertisedInterfaceType::Kiss, Self::Kiss { .. })
        )
    }
}

#[derive(Debug, PartialEq)]
pub struct DiscoveryAdvertisement {
    pub interface_type: AdvertisedInterfaceType,
    pub transport: AdvertisedTransport,
    pub name: Option<String>,
    pub location: GeographicLocation,
    pub details: AdvertisementDetails,
    pub published_ifac: Option<PublishedIfac>,
}

pub(crate) fn invalid_reachable_on(advertisement: &DiscoveryAdvertisement) -> Option<&str> {
    match &advertisement.details {
        AdvertisementDetails::Reachable { host, .. } if !address_is_valid(host) => Some(host),
        AdvertisementDetails::I2p { address } if !address_is_valid(address) => Some(address),
        AdvertisementDetails::Reachable { .. } | AdvertisementDetails::I2p { .. } => None,
        AdvertisementDetails::None
        | AdvertisementDetails::RNode { .. }
        | AdvertisementDetails::Weave { .. }
        | AdvertisementDetails::Kiss { .. } => None,
    }
}

fn address_is_valid(address: &str) -> bool {
    address.parse::<core::net::IpAddr>().is_ok() || hostname_is_valid(address)
}

fn hostname_is_valid(hostname: &str) -> bool {
    let hostname = hostname.strip_suffix('.').unwrap_or(hostname);
    if hostname.is_empty() || hostname.len() > 253 || !hostname.is_ascii() {
        return false;
    }
    let mut labels = hostname.split('.');
    let Some(last) = labels.next_back() else {
        return false;
    };
    if last.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    hostname.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reachable(host: &str) -> DiscoveryAdvertisement {
        DiscoveryAdvertisement {
            interface_type: AdvertisedInterfaceType::Backbone,
            transport: AdvertisedTransport::Disabled(TransportId::new([0x11; 16])),
            name: None,
            location: GeographicLocation::UNKNOWN,
            details: AdvertisementDetails::Reachable {
                host: String::from(host),
                port: 4242,
            },
            published_ifac: None,
        }
    }

    #[test]
    fn reference_address_rules_accept_addresses_and_dns_names() {
        for host in [
            "192.0.2.1",
            "2001:db8::1",
            "router.example",
            "router.example.",
            "peer.b32.i2p",
        ] {
            assert_eq!(invalid_reachable_on(&reachable(host)), None);
        }
    }

    #[test]
    fn reference_address_rules_reject_non_hosts_and_numeric_dns_tails() {
        for host in [
            "",
            "not a host",
            "-router.example",
            "router-.example",
            "host.123",
        ] {
            assert_eq!(invalid_reachable_on(&reachable(host)), Some(host));
        }
    }
}
