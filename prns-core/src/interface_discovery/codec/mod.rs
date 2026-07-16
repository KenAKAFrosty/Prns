mod decoder;

use alloc::string::String;
use alloc::vec::Vec;

use decoder::Decoder;

use super::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveryAdvertisement,
    GeographicLocation, PublishedIfac, STAMP_SIZE,
};
use crate::wire::TransportId;

const INTERFACE_TYPE: u64 = 0x00;
const TRANSPORT: u64 = 0x01;
const REACHABLE_ON: u64 = 0x02;
const LATITUDE: u64 = 0x03;
const LONGITUDE: u64 = 0x04;
const HEIGHT: u64 = 0x05;
const PORT: u64 = 0x06;
const IFAC_NETNAME: u64 = 0x07;
const IFAC_NETKEY: u64 = 0x08;
const FREQUENCY: u64 = 0x09;
const BANDWIDTH: u64 = 0x0A;
const SPREADING_FACTOR: u64 = 0x0B;
const CODING_RATE: u64 = 0x0C;
const MODULATION: u64 = 0x0D;
const CHANNEL: u64 = 0x0E;
const TRANSPORT_ID: u64 = 0xFE;
const NAME: u64 = 0xFF;
const FLAG_SIGNED: u8 = 0b0000_0001;
const FLAG_ENCRYPTED: u8 = 0b0000_0010;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryField {
    InterfaceType,
    Transport,
    ReachableOn,
    Latitude,
    Longitude,
    Height,
    Port,
    IfacNetworkName,
    IfacPassphrase,
    Frequency,
    Bandwidth,
    SpreadingFactor,
    CodingRate,
    Modulation,
    Channel,
    TransportId,
    Name,
}

impl DiscoveryField {
    pub const fn name(self) -> &'static str {
        match self {
            Self::InterfaceType => "interface_type",
            Self::Transport => "transport",
            Self::ReachableOn => "reachable_on",
            Self::Latitude => "latitude",
            Self::Longitude => "longitude",
            Self::Height => "height",
            Self::Port => "port",
            Self::IfacNetworkName => "ifac_netname",
            Self::IfacPassphrase => "ifac_netkey",
            Self::Frequency => "frequency",
            Self::Bandwidth => "bandwidth",
            Self::SpreadingFactor => "spreading_factor",
            Self::CodingRate => "coding_rate",
            Self::Modulation => "modulation",
            Self::Channel => "channel",
            Self::TransportId => "transport_id",
            Self::Name => "name",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryDecodeError {
    MessagePack,
    TrailingData,
    ExpectedMap,
    DuplicateField(DiscoveryField),
    MissingField(DiscoveryField),
    InvalidField(DiscoveryField),
    UnsupportedInterfaceType,
}

impl core::fmt::Display for DiscoveryDecodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MessagePack => formatter.write_str("invalid MessagePack"),
            Self::TrailingData => formatter.write_str("trailing data"),
            Self::ExpectedMap => formatter.write_str("expected an interface discovery map"),
            Self::DuplicateField(field) => write!(formatter, "duplicate {} field", field.name()),
            Self::MissingField(field) => write!(formatter, "missing {} field", field.name()),
            Self::InvalidField(field) => write!(formatter, "invalid {} field", field.name()),
            Self::UnsupportedInterfaceType => {
                formatter.write_str("unsupported discovered interface type")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryDecodeError {}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryEncodeError {
    DetailsDoNotMatchInterfaceType,
    ValueTooLong(DiscoveryField),
    MessagePack,
}

impl core::fmt::Display for DiscoveryEncodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DetailsDoNotMatchInterfaceType => {
                formatter.write_str("interface discovery details do not match the interface type")
            }
            Self::ValueTooLong(field) => {
                write!(formatter, "{} is too long for MessagePack", field.name())
            }
            Self::MessagePack => {
                formatter.write_str("could not encode interface discovery MessagePack")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryEncodeError {}

#[derive(Debug, PartialEq, Eq)]
pub struct DiscoveryEnvelope<'a> {
    pub signed: bool,
    pub body: DiscoveryEnvelopeBody<'a>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryEnvelopeBody<'a> {
    Plaintext {
        packed_advertisement: &'a [u8],
        stamp: &'a [u8; STAMP_SIZE],
    },
    Encrypted {
        ciphertext: &'a [u8],
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryEnvelopeError {
    MissingFlags,
    MissingPlaintextOrStamp,
}

impl core::fmt::Display for DiscoveryEnvelopeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingFlags => formatter.write_str("interface discovery payload has no flags"),
            Self::MissingPlaintextOrStamp => formatter
                .write_str("plaintext interface discovery payload has no advertisement or stamp"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryEnvelopeError {}

pub fn decode_envelope(bytes: &[u8]) -> Result<DiscoveryEnvelope<'_>, DiscoveryEnvelopeError> {
    let (&flags, body) = bytes
        .split_first()
        .ok_or(DiscoveryEnvelopeError::MissingFlags)?;
    let body = if flags & FLAG_ENCRYPTED != 0 {
        DiscoveryEnvelopeBody::Encrypted { ciphertext: body }
    } else {
        let split = body
            .len()
            .checked_sub(STAMP_SIZE)
            .filter(|split| *split > 0)
            .ok_or(DiscoveryEnvelopeError::MissingPlaintextOrStamp)?;
        let (packed_advertisement, stamp) = body.split_at(split);
        DiscoveryEnvelopeBody::Plaintext {
            packed_advertisement,
            stamp: stamp
                .try_into()
                .map_err(|_| DiscoveryEnvelopeError::MissingPlaintextOrStamp)?,
        }
    };
    Ok(DiscoveryEnvelope {
        signed: flags & FLAG_SIGNED != 0,
        body,
    })
}

pub fn encode_plaintext_envelope(packed_advertisement: &[u8], stamp: &[u8; STAMP_SIZE]) -> Vec<u8> {
    let mut envelope = Vec::with_capacity(1 + packed_advertisement.len() + stamp.len());
    envelope.push(0);
    envelope.extend_from_slice(packed_advertisement);
    envelope.extend_from_slice(stamp);
    envelope
}

pub fn encode_encrypted_envelope(ciphertext: &[u8]) -> Vec<u8> {
    let mut envelope = Vec::with_capacity(1 + ciphertext.len());
    envelope.push(FLAG_ENCRYPTED);
    envelope.extend_from_slice(ciphertext);
    envelope
}

pub fn encode_advertisement(
    advertisement: &DiscoveryAdvertisement,
) -> Result<Vec<u8>, DiscoveryEncodeError> {
    if !advertisement.details.matches(advertisement.interface_type) {
        return Err(DiscoveryEncodeError::DetailsDoNotMatchInterfaceType);
    }
    let field_count = 7
        + detail_field_count(&advertisement.details)
        + usize::from(advertisement.published_ifac.is_some()) * 2;
    let mut encoded = Vec::new();
    map_encode(rmp::encode::write_map_len(&mut encoded, field_count as u32))?;
    write_key(&mut encoded, INTERFACE_TYPE)?;
    write_string(
        &mut encoded,
        advertisement.interface_type.rns_name(),
        DiscoveryField::InterfaceType,
    )?;
    write_key(&mut encoded, TRANSPORT)?;
    map_encode(rmp::encode::write_bool(
        &mut encoded,
        advertisement.transport.is_enabled(),
    ))?;
    write_key(&mut encoded, TRANSPORT_ID)?;
    map_encode(rmp::encode::write_bin(
        &mut encoded,
        advertisement.transport.transport_id().as_bytes(),
    ))?;
    write_key(&mut encoded, NAME)?;
    write_optional_string(
        &mut encoded,
        advertisement.name.as_deref(),
        DiscoveryField::Name,
    )?;
    write_key(&mut encoded, LATITUDE)?;
    write_optional_float(&mut encoded, advertisement.location.latitude)?;
    write_key(&mut encoded, LONGITUDE)?;
    write_optional_float(&mut encoded, advertisement.location.longitude)?;
    write_key(&mut encoded, HEIGHT)?;
    write_optional_float(&mut encoded, advertisement.location.height)?;
    write_details(&mut encoded, &advertisement.details)?;
    if let Some(ifac) = &advertisement.published_ifac {
        write_key(&mut encoded, IFAC_NETNAME)?;
        write_optional_string(
            &mut encoded,
            ifac.network_name.as_deref(),
            DiscoveryField::IfacNetworkName,
        )?;
        write_key(&mut encoded, IFAC_NETKEY)?;
        write_optional_string(
            &mut encoded,
            ifac.passphrase.as_deref(),
            DiscoveryField::IfacPassphrase,
        )?;
    }
    Ok(encoded)
}

pub fn decode_advertisement(bytes: &[u8]) -> Result<DiscoveryAdvertisement, DiscoveryDecodeError> {
    let mut decoder = Decoder::new(bytes);
    let field_count = decoder.map_len()?;
    let mut fields = DecodedFields::default();
    for _ in 0..field_count {
        match decoder.map_key()? {
            Some(key) => fields.insert(key, &mut decoder)?,
            None => decoder.skip_value(0)?,
        }
    }
    if !decoder.is_empty() {
        return Err(DiscoveryDecodeError::TrailingData);
    }
    fields.finish()
}

fn detail_field_count(details: &AdvertisementDetails) -> usize {
    match details {
        AdvertisementDetails::None => 0,
        AdvertisementDetails::Reachable { .. } => 2,
        AdvertisementDetails::I2p { .. } => 1,
        AdvertisementDetails::RNode { .. } | AdvertisementDetails::Weave { .. } => 4,
        AdvertisementDetails::Kiss { .. } => 3,
    }
}

fn write_details(
    encoded: &mut Vec<u8>,
    details: &AdvertisementDetails,
) -> Result<(), DiscoveryEncodeError> {
    match details {
        AdvertisementDetails::None => {}
        AdvertisementDetails::Reachable { host, port } => {
            write_key(encoded, REACHABLE_ON)?;
            write_string(encoded, host, DiscoveryField::ReachableOn)?;
            write_key(encoded, PORT)?;
            write_unsigned(encoded, u64::from(*port))?;
        }
        AdvertisementDetails::I2p { address } => {
            write_key(encoded, REACHABLE_ON)?;
            write_string(encoded, address, DiscoveryField::ReachableOn)?;
        }
        AdvertisementDetails::RNode {
            frequency_hz,
            bandwidth_hz,
            spreading_factor,
            coding_rate,
        } => {
            write_key(encoded, FREQUENCY)?;
            write_unsigned(encoded, *frequency_hz)?;
            write_key(encoded, BANDWIDTH)?;
            write_unsigned(encoded, u64::from(*bandwidth_hz))?;
            write_key(encoded, SPREADING_FACTOR)?;
            write_unsigned(encoded, u64::from(*spreading_factor))?;
            write_key(encoded, CODING_RATE)?;
            write_unsigned(encoded, u64::from(*coding_rate))?;
        }
        AdvertisementDetails::Weave {
            frequency_hz,
            bandwidth_hz,
            channel,
            modulation,
        } => {
            write_key(encoded, FREQUENCY)?;
            write_unsigned(encoded, *frequency_hz)?;
            write_key(encoded, BANDWIDTH)?;
            write_unsigned(encoded, u64::from(*bandwidth_hz))?;
            write_key(encoded, CHANNEL)?;
            write_unsigned(encoded, u64::from(*channel))?;
            write_key(encoded, MODULATION)?;
            write_string(encoded, modulation, DiscoveryField::Modulation)?;
        }
        AdvertisementDetails::Kiss {
            frequency_hz,
            bandwidth_hz,
            modulation,
        } => {
            write_key(encoded, FREQUENCY)?;
            write_unsigned(encoded, *frequency_hz)?;
            write_key(encoded, BANDWIDTH)?;
            write_unsigned(encoded, u64::from(*bandwidth_hz))?;
            write_key(encoded, MODULATION)?;
            write_string(encoded, modulation, DiscoveryField::Modulation)?;
        }
    }
    Ok(())
}

fn write_key(encoded: &mut Vec<u8>, key: u64) -> Result<(), DiscoveryEncodeError> {
    write_unsigned(encoded, key)
}

fn write_unsigned(encoded: &mut Vec<u8>, value: u64) -> Result<(), DiscoveryEncodeError> {
    map_encode(rmp::encode::write_uint(encoded, value)).map(|_| ())
}

fn write_string(
    encoded: &mut Vec<u8>,
    value: &str,
    field: DiscoveryField,
) -> Result<(), DiscoveryEncodeError> {
    if u32::try_from(value.len()).is_err() {
        return Err(DiscoveryEncodeError::ValueTooLong(field));
    }
    map_encode(rmp::encode::write_str(encoded, value))
}

fn write_optional_string(
    encoded: &mut Vec<u8>,
    value: Option<&str>,
    field: DiscoveryField,
) -> Result<(), DiscoveryEncodeError> {
    match value {
        Some(value) => write_string(encoded, value, field),
        None => map_encode(rmp::encode::write_nil(encoded)),
    }
}

fn write_optional_float(
    encoded: &mut Vec<u8>,
    value: Option<f64>,
) -> Result<(), DiscoveryEncodeError> {
    match value {
        Some(value) => map_encode(rmp::encode::write_f64(encoded, value)),
        None => map_encode(rmp::encode::write_nil(encoded)),
    }
}

fn map_encode<T, E>(result: Result<T, E>) -> Result<T, DiscoveryEncodeError> {
    result.map_err(|_| DiscoveryEncodeError::MessagePack)
}

#[derive(Default)]
struct DecodedFields {
    interface_type: Option<String>,
    transport: Option<bool>,
    transport_id: Option<TransportId>,
    name: Option<Option<String>>,
    latitude: Option<Option<f64>>,
    longitude: Option<Option<f64>>,
    height: Option<Option<f64>>,
    reachable_on: Option<String>,
    port: Option<u16>,
    ifac_netname: Option<Option<String>>,
    ifac_netkey: Option<Option<String>>,
    frequency: Option<u64>,
    bandwidth: Option<u32>,
    spreading_factor: Option<u8>,
    coding_rate: Option<u8>,
    modulation: Option<String>,
    channel: Option<u32>,
}

impl DecodedFields {
    fn insert(&mut self, key: u64, decoder: &mut Decoder<'_>) -> Result<(), DiscoveryDecodeError> {
        match key {
            INTERFACE_TYPE => store(
                &mut self.interface_type,
                decoder.string(DiscoveryField::InterfaceType)?,
                DiscoveryField::InterfaceType,
            ),
            TRANSPORT => store(
                &mut self.transport,
                decoder.boolean(DiscoveryField::Transport)?,
                DiscoveryField::Transport,
            ),
            TRANSPORT_ID => store(
                &mut self.transport_id,
                decoder.transport_id()?,
                DiscoveryField::TransportId,
            ),
            NAME => store(
                &mut self.name,
                decoder.optional_string(DiscoveryField::Name)?,
                DiscoveryField::Name,
            ),
            LATITUDE => store(
                &mut self.latitude,
                decoder.optional_float(DiscoveryField::Latitude)?,
                DiscoveryField::Latitude,
            ),
            LONGITUDE => store(
                &mut self.longitude,
                decoder.optional_float(DiscoveryField::Longitude)?,
                DiscoveryField::Longitude,
            ),
            HEIGHT => store(
                &mut self.height,
                decoder.optional_float(DiscoveryField::Height)?,
                DiscoveryField::Height,
            ),
            REACHABLE_ON => store(
                &mut self.reachable_on,
                decoder.string(DiscoveryField::ReachableOn)?,
                DiscoveryField::ReachableOn,
            ),
            PORT => store(
                &mut self.port,
                decoder
                    .unsigned(DiscoveryField::Port)?
                    .try_into()
                    .map_err(|_| DiscoveryDecodeError::InvalidField(DiscoveryField::Port))?,
                DiscoveryField::Port,
            ),
            IFAC_NETNAME => store(
                &mut self.ifac_netname,
                decoder.optional_string(DiscoveryField::IfacNetworkName)?,
                DiscoveryField::IfacNetworkName,
            ),
            IFAC_NETKEY => store(
                &mut self.ifac_netkey,
                decoder.optional_string(DiscoveryField::IfacPassphrase)?,
                DiscoveryField::IfacPassphrase,
            ),
            FREQUENCY => store(
                &mut self.frequency,
                decoder.unsigned(DiscoveryField::Frequency)?,
                DiscoveryField::Frequency,
            ),
            BANDWIDTH => store(
                &mut self.bandwidth,
                decoder
                    .unsigned(DiscoveryField::Bandwidth)?
                    .try_into()
                    .map_err(|_| DiscoveryDecodeError::InvalidField(DiscoveryField::Bandwidth))?,
                DiscoveryField::Bandwidth,
            ),
            SPREADING_FACTOR => store(
                &mut self.spreading_factor,
                decoder
                    .unsigned(DiscoveryField::SpreadingFactor)?
                    .try_into()
                    .map_err(|_| {
                        DiscoveryDecodeError::InvalidField(DiscoveryField::SpreadingFactor)
                    })?,
                DiscoveryField::SpreadingFactor,
            ),
            CODING_RATE => store(
                &mut self.coding_rate,
                decoder
                    .unsigned(DiscoveryField::CodingRate)?
                    .try_into()
                    .map_err(|_| DiscoveryDecodeError::InvalidField(DiscoveryField::CodingRate))?,
                DiscoveryField::CodingRate,
            ),
            MODULATION => store(
                &mut self.modulation,
                decoder.string(DiscoveryField::Modulation)?,
                DiscoveryField::Modulation,
            ),
            CHANNEL => store(
                &mut self.channel,
                decoder
                    .unsigned(DiscoveryField::Channel)?
                    .try_into()
                    .map_err(|_| DiscoveryDecodeError::InvalidField(DiscoveryField::Channel))?,
                DiscoveryField::Channel,
            ),
            _ => decoder.skip_value(0),
        }
    }

    fn finish(self) -> Result<DiscoveryAdvertisement, DiscoveryDecodeError> {
        let interface_type = AdvertisedInterfaceType::from_rns_name(&self.interface_type.ok_or(
            DiscoveryDecodeError::MissingField(DiscoveryField::InterfaceType),
        )?)
        .ok_or(DiscoveryDecodeError::UnsupportedInterfaceType)?;
        let transport = AdvertisedTransport::from_wire(
            self.transport.ok_or(DiscoveryDecodeError::MissingField(
                DiscoveryField::Transport,
            ))?,
            self.transport_id.ok_or(DiscoveryDecodeError::MissingField(
                DiscoveryField::TransportId,
            ))?,
        );
        let name = self
            .name
            .ok_or(DiscoveryDecodeError::MissingField(DiscoveryField::Name))?;
        let location = GeographicLocation {
            latitude: self
                .latitude
                .ok_or(DiscoveryDecodeError::MissingField(DiscoveryField::Latitude))?,
            longitude: self.longitude.ok_or(DiscoveryDecodeError::MissingField(
                DiscoveryField::Longitude,
            ))?,
            height: self
                .height
                .ok_or(DiscoveryDecodeError::MissingField(DiscoveryField::Height))?,
        };
        let details = match interface_type {
            AdvertisedInterfaceType::Backbone | AdvertisedInterfaceType::TcpServer => {
                AdvertisementDetails::Reachable {
                    host: required(self.reachable_on, DiscoveryField::ReachableOn)?,
                    port: required(self.port, DiscoveryField::Port)?,
                }
            }
            AdvertisedInterfaceType::TcpClient => AdvertisementDetails::None,
            AdvertisedInterfaceType::I2p => AdvertisementDetails::I2p {
                address: required(self.reachable_on, DiscoveryField::ReachableOn)?,
            },
            AdvertisedInterfaceType::RNode => AdvertisementDetails::RNode {
                frequency_hz: required(self.frequency, DiscoveryField::Frequency)?,
                bandwidth_hz: required(self.bandwidth, DiscoveryField::Bandwidth)?,
                spreading_factor: required(self.spreading_factor, DiscoveryField::SpreadingFactor)?,
                coding_rate: required(self.coding_rate, DiscoveryField::CodingRate)?,
            },
            AdvertisedInterfaceType::Weave => AdvertisementDetails::Weave {
                frequency_hz: required(self.frequency, DiscoveryField::Frequency)?,
                bandwidth_hz: required(self.bandwidth, DiscoveryField::Bandwidth)?,
                channel: required(self.channel, DiscoveryField::Channel)?,
                modulation: required(self.modulation, DiscoveryField::Modulation)?,
            },
            AdvertisedInterfaceType::Kiss => AdvertisementDetails::Kiss {
                frequency_hz: required(self.frequency, DiscoveryField::Frequency)?,
                bandwidth_hz: required(self.bandwidth, DiscoveryField::Bandwidth)?,
                modulation: required(self.modulation, DiscoveryField::Modulation)?,
            },
        };
        let published_ifac = if self.ifac_netname.is_none() && self.ifac_netkey.is_none() {
            None
        } else {
            Some(PublishedIfac {
                network_name: self.ifac_netname.flatten(),
                passphrase: self.ifac_netkey.flatten(),
            })
        };
        Ok(DiscoveryAdvertisement {
            interface_type,
            transport,
            name,
            location,
            details,
            published_ifac,
        })
    }
}

fn store<T>(
    slot: &mut Option<T>,
    value: T,
    field: DiscoveryField,
) -> Result<(), DiscoveryDecodeError> {
    if slot.replace(value).is_some() {
        return Err(DiscoveryDecodeError::DuplicateField(field));
    }
    Ok(())
}

fn required<T>(value: Option<T>, field: DiscoveryField) -> Result<T, DiscoveryDecodeError> {
    value.ok_or(DiscoveryDecodeError::MissingField(field))
}

#[cfg(test)]
mod tests;
