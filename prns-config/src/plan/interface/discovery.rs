use prns_core::interface_discovery::{StampCost, DEFAULT_STAMP_COST};
use prns_core::interfaces::tcp::core::TcpWireFraming;
use prns_core::units::DurationMillis;

use super::medium::PlannedMedium;
use crate::reference::keys::interface as interface_key;
use crate::reference::{ReferenceInterface, ReferenceParams};

#[derive(Debug, Clone, PartialEq)]
pub enum InterfaceDiscoveryPlan {
    Disabled,
    Announce(DiscoveryAnnouncementPlan),
    Unpublishable(DiscoveryPublicationProblem),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryAnnouncementPlan {
    pub interval: DurationMillis,
    pub stamp_cost: StampCost,
    pub name: Option<String>,
    pub encryption: DiscoveryEncryption,
    pub ifac: DiscoveryIfacPublication,
    pub location: DiscoveryLocationPlan,
    pub advertisement: DiscoveryAdvertisementPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryAdvertisementPlan {
    Backbone {
        reachable_on: String,
        port: u16,
    },
    TcpServer {
        reachable_on: String,
        port: u16,
    },
    RNode {
        frequency_hz: u64,
        bandwidth_hz: u32,
        spreading_factor: u8,
        coding_rate: u8,
    },
    Kiss {
        frequency_hz: u64,
        bandwidth_hz: u32,
        modulation: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryPublicationProblem {
    UnsupportedInterfaceType,
    MissingRequiredSetting { key: &'static str },
    IncompatibleSetting { key: &'static str },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryEncryption {
    Plaintext,
    NetworkIdentity,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryIfacPublication {
    Omit,
    Include,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryLocationPlan {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub height: Option<f64>,
}

pub(in crate::plan) fn plan_interface_discovery(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
) -> InterfaceDiscoveryPlan {
    if interface.discovery.discoverable != Some(true) {
        return InterfaceDiscoveryPlan::Disabled;
    }
    let advertisement = match plan_discovery_advertisement(interface, medium) {
        Ok(advertisement) => advertisement,
        Err(problem) => return InterfaceDiscoveryPlan::Unpublishable(problem),
    };
    let minutes = interface
        .discovery
        .announce_interval_minutes
        .unwrap_or(6 * 60)
        .max(5) as u64;
    InterfaceDiscoveryPlan::Announce(DiscoveryAnnouncementPlan {
        interval: DurationMillis(minutes.saturating_mul(60 * 1_000)),
        stamp_cost: interface.discovery.stamp_cost.unwrap_or(DEFAULT_STAMP_COST),
        name: interface.discovery.name.clone(),
        encryption: if interface.discovery.encrypt == Some(true) {
            DiscoveryEncryption::NetworkIdentity
        } else {
            DiscoveryEncryption::Plaintext
        },
        ifac: if interface.discovery.publish_ifac == Some(true) {
            DiscoveryIfacPublication::Include
        } else {
            DiscoveryIfacPublication::Omit
        },
        location: DiscoveryLocationPlan {
            latitude: interface.discovery.latitude,
            longitude: interface.discovery.longitude,
            height: interface.discovery.height,
        },
        advertisement,
    })
}

fn plan_discovery_advertisement(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
) -> Result<DiscoveryAdvertisementPlan, DiscoveryPublicationProblem> {
    let reachable_on = || {
        interface.discovery.reachable_on.clone().ok_or(
            DiscoveryPublicationProblem::MissingRequiredSetting {
                key: interface_key::REACHABLE_ON,
            },
        )
    };
    let kiss = || {
        Ok(DiscoveryAdvertisementPlan::Kiss {
            frequency_hz: interface.discovery.frequency_hz.ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::DISCOVERY_FREQUENCY,
                },
            )?,
            bandwidth_hz: interface.discovery.bandwidth_hz.ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::DISCOVERY_BANDWIDTH,
                },
            )?,
            modulation: interface.discovery.modulation.clone().ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::DISCOVERY_MODULATION,
                },
            )?,
        })
    };
    match (medium, &interface.params) {
        (
            PlannedMedium::Backbone { .. },
            ReferenceParams::Backbone {
                listen_port, port, ..
            },
        ) => Ok(DiscoveryAdvertisementPlan::Backbone {
            reachable_on: reachable_on()?,
            port: port.or(*listen_port).ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::LISTEN_PORT,
                },
            )?,
        }),
        (
            PlannedMedium::TcpServer { .. },
            ReferenceParams::TcpServer {
                listen_port, port, ..
            },
        ) => Ok(DiscoveryAdvertisementPlan::TcpServer {
            reachable_on: reachable_on()?,
            port: port.or(*listen_port).ok_or(
                DiscoveryPublicationProblem::MissingRequiredSetting {
                    key: interface_key::LISTEN_PORT,
                },
            )?,
        }),
        (
            PlannedMedium::Rnode {
                frequency_hz,
                bandwidth_hz,
                spreading_factor,
                coding_rate,
                ..
            },
            ReferenceParams::Rnode { .. },
        ) => Ok(rnode_discovery_advertisement(
            *frequency_hz,
            *bandwidth_hz,
            *spreading_factor,
            *coding_rate,
        )),
        (PlannedMedium::RnodeMulti { member }, ReferenceParams::RnodeMulti { .. }) => {
            let radio = member.radio();
            Ok(rnode_discovery_advertisement(
                u64::from(radio.frequency().hz()),
                radio.bandwidth_hz(),
                radio.spreading_factor(),
                radio.coding_rate(),
            ))
        }
        (PlannedMedium::Kiss { .. }, ReferenceParams::Kiss { .. }) => kiss(),
        (
            PlannedMedium::TcpClient {
                framing: TcpWireFraming::Kiss,
                ..
            },
            ReferenceParams::TcpClient { .. },
        ) => kiss(),
        (PlannedMedium::TcpClient { .. }, ReferenceParams::TcpClient { .. }) => {
            Err(DiscoveryPublicationProblem::IncompatibleSetting {
                key: interface_key::KISS_FRAMING,
            })
        }
        _ => Err(DiscoveryPublicationProblem::UnsupportedInterfaceType),
    }
}

fn rnode_discovery_advertisement(
    frequency_hz: u64,
    bandwidth_hz: u32,
    spreading_factor: u8,
    coding_rate: u8,
) -> DiscoveryAdvertisementPlan {
    DiscoveryAdvertisementPlan::RNode {
        frequency_hz,
        bandwidth_hz,
        spreading_factor,
        coding_rate,
    }
}
