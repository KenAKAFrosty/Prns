use core::num::NonZeroU64;
use std::collections::BTreeMap;

use crate::identity::IdentityHash;
use crate::interface_discovery::model::invalid_reachable_on;
use crate::interface_discovery::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveredInterface,
    DiscoveredInterfaceId, DiscoveryAdvertisement, DiscoveryCatalogSeed, DiscoveryEnvelopeSecurity,
    DiscoveryObservationCount, DiscoveryProvenance, DiscoveryRecord, GeographicLocation,
    PublishedIfac, StampValue,
};
use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use crate::units::{HopCount, InstantMillis};
use crate::wire::TransportId;
use serde::{Deserialize, Serialize};

use super::file::{decode_hex, encode_hex, ArchiveRecordError};
use super::manual_configuration::manual_configuration;

#[derive(Deserialize)]
pub(super) struct ArchiveDocument {
    pub format_version: u32,
    pub interfaces: BTreeMap<String, ArchivedRecord>,
}

impl ArchiveDocument {
    pub fn empty() -> Self {
        Self {
            format_version: super::FORMAT_VERSION,
            interfaces: BTreeMap::new(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct ArchiveDocumentRef<'a> {
    pub format_version: u32,
    pub configuration_note: &'static str,
    pub interfaces: &'a BTreeMap<String, ArchivedRecord>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ArchivedRecord {
    name: String,
    advertisement: ArchivedAdvertisement,
    stamp_value: u16,
    first_heard_unix_ms: u64,
    observation_count: u64,
    provenance: ArchivedProvenance,
    #[serde(default)]
    configuration_entry: Option<String>,
}

impl ArchivedRecord {
    pub fn from_record(record: &DiscoveryRecord) -> Self {
        let interface = record.interface();
        Self {
            name: interface.name.clone(),
            advertisement: ArchivedAdvertisement::from_advertisement(&interface.advertisement),
            stamp_value: interface.stamp_value.get(),
            first_heard_unix_ms: record.first_heard().0,
            observation_count: record.observation_count().get(),
            provenance: ArchivedProvenance::from_provenance(interface.provenance),
            configuration_entry: manual_configuration(interface),
        }
    }

    pub fn to_seed(&self, id: &str) -> Result<DiscoveryCatalogSeed, ArchiveRecordError> {
        if self.name.is_empty() {
            return Err(ArchiveRecordError::EmptyName);
        }
        let id = DiscoveredInterfaceId::from_bytes(decode_hex(id).map_err(|source| {
            ArchiveRecordError::InvalidHex {
                field: "discovery id",
                source,
            }
        })?);
        let advertisement = self.advertisement.to_advertisement()?;
        let provenance = self.provenance.to_provenance()?;
        let observation_count = NonZeroU64::new(self.observation_count)
            .map(DiscoveryObservationCount::from_non_zero)
            .ok_or(ArchiveRecordError::ZeroObservationCount)?;
        Ok(DiscoveryCatalogSeed {
            interface: DiscoveredInterface {
                id,
                name: self.name.clone(),
                advertisement,
                stamp_value: StampValue::new(self.stamp_value)
                    .map_err(ArchiveRecordError::StampValue)?,
                provenance,
            },
            first_heard: InstantMillis(self.first_heard_unix_ms),
            observation_count,
        })
    }

    pub fn refresh_manual_configuration(&mut self, interface: &DiscoveredInterface) {
        self.configuration_entry = manual_configuration(interface);
    }

    pub fn merge_history_from(&mut self, previous: &Self) -> bool {
        if self.provenance.received_at_unix_ms < previous.provenance.received_at_unix_ms {
            return false;
        }
        if self.first_heard_unix_ms > previous.first_heard_unix_ms {
            self.first_heard_unix_ms = previous.first_heard_unix_ms;
            self.observation_count = previous
                .observation_count
                .saturating_add(self.observation_count);
        } else {
            self.first_heard_unix_ms = self.first_heard_unix_ms.min(previous.first_heard_unix_ms);
            self.observation_count = self.observation_count.max(previous.observation_count);
        }
        true
    }
}

#[derive(Serialize, Deserialize)]
struct ArchivedAdvertisement {
    interface_type: String,
    transport_enabled: bool,
    transport_id: String,
    advertised_name: Option<String>,
    location: ArchivedLocation,
    details: ArchivedDetails,
    published_ifac: Option<ArchivedIfac>,
}

impl ArchivedAdvertisement {
    fn from_advertisement(advertisement: &DiscoveryAdvertisement) -> Self {
        let (transport_enabled, transport_id) = match &advertisement.transport {
            AdvertisedTransport::Enabled(transport_id) => (true, transport_id),
            AdvertisedTransport::Disabled(transport_id) => (false, transport_id),
        };
        Self {
            interface_type: String::from(advertisement.interface_type.rns_name()),
            transport_enabled,
            transport_id: encode_hex(transport_id.as_bytes()),
            advertised_name: advertisement.name.clone(),
            location: ArchivedLocation::from_location(&advertisement.location),
            details: ArchivedDetails::from_details(&advertisement.details),
            published_ifac: advertisement
                .published_ifac
                .as_ref()
                .map(ArchivedIfac::from_ifac),
        }
    }

    fn to_advertisement(&self) -> Result<DiscoveryAdvertisement, ArchiveRecordError> {
        let interface_type = AdvertisedInterfaceType::from_rns_name(&self.interface_type)
            .ok_or_else(|| ArchiveRecordError::UnsupportedInterfaceType {
                value: self.interface_type.clone(),
            })?;
        let transport_id = TransportId::new(decode_hex(&self.transport_id).map_err(|source| {
            ArchiveRecordError::InvalidHex {
                field: "transport id",
                source,
            }
        })?);
        let details = self.details.to_details();
        if !details.matches(interface_type) {
            return Err(ArchiveRecordError::MismatchedDetails { interface_type });
        }
        let transport = if self.transport_enabled {
            AdvertisedTransport::Enabled(transport_id)
        } else {
            AdvertisedTransport::Disabled(transport_id)
        };
        let advertisement = DiscoveryAdvertisement {
            interface_type,
            transport,
            name: self.advertised_name.clone(),
            location: self.location.to_location()?,
            details,
            published_ifac: self.published_ifac.as_ref().map(ArchivedIfac::to_ifac),
        };
        if let Some(address) = invalid_reachable_on(&advertisement) {
            return Err(ArchiveRecordError::InvalidReachableAddress {
                value: address.to_owned(),
            });
        }
        Ok(advertisement)
    }
}

#[derive(Serialize, Deserialize)]
struct ArchivedLocation {
    latitude: Option<ArchivedFloat>,
    longitude: Option<ArchivedFloat>,
    height: Option<ArchivedFloat>,
}

impl ArchivedLocation {
    fn from_location(location: &GeographicLocation) -> Self {
        Self {
            latitude: location.latitude.map(ArchivedFloat::from_value),
            longitude: location.longitude.map(ArchivedFloat::from_value),
            height: location.height.map(ArchivedFloat::from_value),
        }
    }

    fn to_location(&self) -> Result<GeographicLocation, ArchiveRecordError> {
        Ok(GeographicLocation {
            latitude: decode_optional_float("latitude", self.latitude.as_ref())?,
            longitude: decode_optional_float("longitude", self.longitude.as_ref())?,
            height: decode_optional_float("height", self.height.as_ref())?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum ArchivedFloat {
    Finite(f64),
    NonFinite(String),
}

impl ArchivedFloat {
    pub fn from_value(value: f64) -> Self {
        if value.is_finite() {
            Self::Finite(value)
        } else {
            Self::NonFinite(value.to_string())
        }
    }

    pub fn decode(&self, field: &'static str) -> Result<f64, ArchiveRecordError> {
        match self {
            Self::Finite(value) => Ok(*value),
            Self::NonFinite(value) => value.parse().map_err(|_| ArchiveRecordError::InvalidFloat {
                field,
                value: value.clone(),
            }),
        }
    }
}

fn decode_optional_float(
    field: &'static str,
    value: Option<&ArchivedFloat>,
) -> Result<Option<f64>, ArchiveRecordError> {
    value.map(|value| value.decode(field)).transpose()
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArchivedDetails {
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

impl ArchivedDetails {
    fn from_details(details: &AdvertisementDetails) -> Self {
        match details {
            AdvertisementDetails::None => Self::None,
            AdvertisementDetails::Reachable { host, port } => Self::Reachable {
                host: host.clone(),
                port: *port,
            },
            AdvertisementDetails::I2p { address } => Self::I2p {
                address: address.clone(),
            },
            AdvertisementDetails::RNode {
                frequency_hz,
                bandwidth_hz,
                spreading_factor,
                coding_rate,
            } => Self::RNode {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                spreading_factor: *spreading_factor,
                coding_rate: *coding_rate,
            },
            AdvertisementDetails::Weave {
                frequency_hz,
                bandwidth_hz,
                channel,
                modulation,
            } => Self::Weave {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                channel: *channel,
                modulation: modulation.clone(),
            },
            AdvertisementDetails::Kiss {
                frequency_hz,
                bandwidth_hz,
                modulation,
            } => Self::Kiss {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                modulation: modulation.clone(),
            },
        }
    }

    fn to_details(&self) -> AdvertisementDetails {
        match self {
            Self::None => AdvertisementDetails::None,
            Self::Reachable { host, port } => AdvertisementDetails::Reachable {
                host: host.clone(),
                port: *port,
            },
            Self::I2p { address } => AdvertisementDetails::I2p {
                address: address.clone(),
            },
            Self::RNode {
                frequency_hz,
                bandwidth_hz,
                spreading_factor,
                coding_rate,
            } => AdvertisementDetails::RNode {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                spreading_factor: *spreading_factor,
                coding_rate: *coding_rate,
            },
            Self::Weave {
                frequency_hz,
                bandwidth_hz,
                channel,
                modulation,
            } => AdvertisementDetails::Weave {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                channel: *channel,
                modulation: modulation.clone(),
            },
            Self::Kiss {
                frequency_hz,
                bandwidth_hz,
                modulation,
            } => AdvertisementDetails::Kiss {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                modulation: modulation.clone(),
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ArchivedIfac {
    network_name: Option<String>,
    passphrase: Option<String>,
}

impl ArchivedIfac {
    fn from_ifac(ifac: &PublishedIfac) -> Self {
        Self {
            network_name: ifac.network_name.clone(),
            passphrase: ifac.passphrase.clone(),
        }
    }

    fn to_ifac(&self) -> PublishedIfac {
        PublishedIfac {
            network_name: self.network_name.clone(),
            passphrase: self.passphrase.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ArchivedProvenance {
    announced_by: String,
    hops: u8,
    received_on: String,
    received_at_unix_ms: u64,
    envelope_security: ArchivedEnvelopeSecurity,
    signed_flag: bool,
}

impl ArchivedProvenance {
    fn from_provenance(provenance: DiscoveryProvenance) -> Self {
        Self {
            announced_by: encode_hex(provenance.announced_by.as_bytes()),
            hops: provenance.hops.0,
            received_on: encode_hex(provenance.received_on.as_bytes()),
            received_at_unix_ms: provenance.received_at.0,
            envelope_security: ArchivedEnvelopeSecurity::from_security(
                provenance.envelope_security,
            ),
            signed_flag: provenance.signed_flag,
        }
    }

    fn to_provenance(&self) -> Result<DiscoveryProvenance, ArchiveRecordError> {
        let announced_by = IdentityHash::new(decode_hex(&self.announced_by).map_err(|source| {
            ArchiveRecordError::InvalidHex {
                field: "announcing identity",
                source,
            }
        })?);
        let received_on =
            InterfaceId::new(decode_hex::<INTERFACE_ID_LEN>(&self.received_on).map_err(
                |source| ArchiveRecordError::InvalidHex {
                    field: "receiving interface",
                    source,
                },
            )?);
        Ok(DiscoveryProvenance {
            announced_by,
            hops: HopCount(self.hops),
            received_on,
            received_at: InstantMillis(self.received_at_unix_ms),
            envelope_security: self.envelope_security.to_security(),
            signed_flag: self.signed_flag,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ArchivedEnvelopeSecurity {
    Plaintext,
    NetworkEncrypted,
}

impl ArchivedEnvelopeSecurity {
    fn from_security(security: DiscoveryEnvelopeSecurity) -> Self {
        match security {
            DiscoveryEnvelopeSecurity::Plaintext => Self::Plaintext,
            DiscoveryEnvelopeSecurity::NetworkEncrypted => Self::NetworkEncrypted,
        }
    }

    fn to_security(&self) -> DiscoveryEnvelopeSecurity {
        match self {
            Self::Plaintext => DiscoveryEnvelopeSecurity::Plaintext,
            Self::NetworkEncrypted => DiscoveryEnvelopeSecurity::NetworkEncrypted,
        }
    }
}
