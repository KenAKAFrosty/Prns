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
