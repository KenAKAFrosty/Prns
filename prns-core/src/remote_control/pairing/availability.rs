use core::num::NonZeroU32;

use crate::identity::IdentitySigner;
use crate::interfaces::InterfaceId;
use crate::routing::announce::{
    write_announce_wire_packet, Announce, AnnounceBuildError, AnnounceId, AnnounceValidationError,
    DottedNameHash, ANNOUNCE_FIXED_FIELDS_LEN,
};
use crate::routing::RouteExpiresAfter;
use crate::units::{DurationMillis, HopCount, InstantMillis};
use crate::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    WireError, WirePacketHeader, BROADCAST_MDU, HEADER_MIN_LEN,
};
use heapless::Vec as HeaplessVec;

use super::{
    RemoteControlPairingEndpoint, RemoteControlPairingIdentity,
    REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH,
};

const PAIRING_AVAILABILITY_ENVELOPE_HEADER_LEN: usize = 2;
const PAIRING_AVAILABILITY_SIGNED_HEADER_LEN: usize = 2;
const PAIRING_AVAILABILITY_EXPIRES_AFTER_LEN: usize = 4;
const PAIRING_AVAILABILITY_SIGNED_METADATA_LEN: usize =
    PAIRING_AVAILABILITY_SIGNED_HEADER_LEN.saturating_add(PAIRING_AVAILABILITY_EXPIRES_AFTER_LEN);
const PAIRING_AVAILABILITY_OVERHEAD: usize = PAIRING_AVAILABILITY_ENVELOPE_HEADER_LEN
    .saturating_add(HEADER_MIN_LEN)
    .saturating_add(ANNOUNCE_FIXED_FIELDS_LEN)
    .saturating_add(PAIRING_AVAILABILITY_SIGNED_METADATA_LEN);

pub const MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN: usize =
    BROADCAST_MDU.saturating_sub(PAIRING_AVAILABILITY_OVERHEAD);
pub const MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER: DurationMillis =
    DurationMillis(10 * 60 * 1_000);

const REMOTE_CONTROL_PAIRING_AVAILABILITY_DESTINATION_HASH: DestinationHash =
    DestinationHash::new([
        0x5b, 0x3c, 0xb7, 0x4e, 0xf5, 0x7b, 0xf5, 0x2e, 0x83, 0xe2, 0x07, 0x69, 0x90, 0xd0, 0x8f,
        0x87,
    ]);

const _: () = assert!(PAIRING_AVAILABILITY_OVERHEAD < BROADCAST_MDU);
const _: () = assert!(MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER.0 <= u32::MAX as u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingAvailabilityDestination {
    destination_hash: DestinationHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAvailabilityDestinationError {
    NonCanonical { found: DestinationHash },
}

impl RemoteControlPairingAvailabilityDestination {
    #[must_use]
    pub const fn canonical() -> Self {
        Self {
            destination_hash: REMOTE_CONTROL_PAIRING_AVAILABILITY_DESTINATION_HASH,
        }
    }

    #[must_use]
    pub const fn destination_hash(self) -> DestinationHash {
        self.destination_hash
    }
}

impl From<RemoteControlPairingAvailabilityDestination> for DestinationHash {
    fn from(destination: RemoteControlPairingAvailabilityDestination) -> Self {
        destination.destination_hash
    }
}

impl TryFrom<DestinationHash> for RemoteControlPairingAvailabilityDestination {
    type Error = RemoteControlPairingAvailabilityDestinationError;

    fn try_from(destination_hash: DestinationHash) -> Result<Self, Self::Error> {
        let canonical = Self::canonical();
        if destination_hash != canonical.destination_hash {
            return Err(
                RemoteControlPairingAvailabilityDestinationError::NonCanonical {
                    found: destination_hash,
                },
            );
        }
        Ok(canonical)
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlPairingAvailabilityProtocolVersion {
        V1 = 1,
    }
}

impl RemoteControlPairingAvailabilityProtocolVersion {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    const fn from_wire(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::V1),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlPairingAvailabilityKind {
        PairingAvailable = 1,
    }
}

impl RemoteControlPairingAvailabilityKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    const fn from_wire(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::PairingAvailable),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingExpiresAfterError {
    Zero,
    TooLong {
        actual: DurationMillis,
        maximum: DurationMillis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingExpiresAfter(NonZeroU32);

impl RemoteControlPairingExpiresAfter {
    #[must_use]
    pub const fn duration(self) -> DurationMillis {
        DurationMillis(self.0.get() as u64)
    }

    #[must_use]
    pub const fn deadline_from(self, observed_at: InstantMillis) -> InstantMillis {
        observed_at.saturating_add(self.duration())
    }

    const fn to_wire(self) -> [u8; PAIRING_AVAILABILITY_EXPIRES_AFTER_LEN] {
        self.0.get().to_le_bytes()
    }
}

impl TryFrom<DurationMillis> for RemoteControlPairingExpiresAfter {
    type Error = RemoteControlPairingExpiresAfterError;

    fn try_from(duration: DurationMillis) -> Result<Self, Self::Error> {
        if duration > MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER {
            return Err(RemoteControlPairingExpiresAfterError::TooLong {
                actual: duration,
                maximum: MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER,
            });
        }
        let Some(millis) = NonZeroU32::new(duration.0 as u32) else {
            return Err(RemoteControlPairingExpiresAfterError::Zero);
        };
        Ok(Self(millis))
    }
}

impl From<RemoteControlPairingExpiresAfter> for RouteExpiresAfter {
    fn from(expires_after: RemoteControlPairingExpiresAfter) -> Self {
        Self::from_nonzero_millis(expires_after.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingPublicAppDataError {
    TooLong { actual: usize, maximum: usize },
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingPublicAppData<'a> {
    bytes: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlPairingPublicAppDataBytes(
    HeaplessVec<u8, MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN>,
);

impl RemoteControlPairingPublicAppDataBytes {
    #[must_use]
    pub fn as_borrowed(&self) -> RemoteControlPairingPublicAppData<'_> {
        RemoteControlPairingPublicAppData {
            bytes: self.0.as_slice(),
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl TryFrom<&[u8]> for RemoteControlPairingPublicAppDataBytes {
    type Error = RemoteControlPairingPublicAppDataError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let bounded = HeaplessVec::from_slice(bytes).map_err(|()| {
            RemoteControlPairingPublicAppDataError::TooLong {
                actual: bytes.len(),
                maximum: MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN,
            }
        })?;
        Ok(Self(bounded))
    }
}

impl<'a> RemoteControlPairingPublicAppData<'a> {
    #[must_use]
    pub const fn empty() -> Self {
        Self { bytes: &[] }
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

impl<'a> TryFrom<&'a [u8]> for RemoteControlPairingPublicAppData<'a> {
    type Error = RemoteControlPairingPublicAppDataError;

    fn try_from(bytes: &'a [u8]) -> Result<Self, Self::Error> {
        if bytes.len() > MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN {
            return Err(RemoteControlPairingPublicAppDataError::TooLong {
                actual: bytes.len(),
                maximum: MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN,
            });
        }
        Ok(Self { bytes })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAvailabilityHeaderError {
    Truncated,
    UnsupportedVersion { found: u8 },
    UnknownKind { found: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAvailabilityInnerHeaderError {
    AuthenticatedIfac,
    RatchetPresent,
    TransportPropagation,
    UnexpectedDestinationType { found: DestinationType },
    UnexpectedPacketType { found: PacketType },
    NonZeroHops { found: u8 },
    TransportIdPresent,
    UnexpectedContext { found: crate::wire::WireContext },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAvailabilityParseError {
    PayloadTooLong {
        actual: usize,
        maximum: usize,
    },
    EnvelopeHeader(RemoteControlPairingAvailabilityHeaderError),
    InnerWire(WireError),
    InnerHeader(RemoteControlPairingAvailabilityInnerHeaderError),
    InnerAnnounce(AnnounceValidationError),
    UnexpectedDottedNameHash {
        found: DottedNameHash,
    },
    SignedHeader(RemoteControlPairingAvailabilityHeaderError),
    HeaderMismatch {
        envelope_version: RemoteControlPairingAvailabilityProtocolVersion,
        envelope_kind: RemoteControlPairingAvailabilityKind,
        signed_version: RemoteControlPairingAvailabilityProtocolVersion,
        signed_kind: RemoteControlPairingAvailabilityKind,
    },
    SignedMetadataTruncated,
    InvalidExpiresAfter(RemoteControlPairingExpiresAfterError),
    InvalidPublicAppData(RemoteControlPairingPublicAppDataError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAvailabilityWriteError {
    BufferTooShort { required: usize, actual: usize },
    BuildAnnounce(AnnounceBuildError),
    WriteAnnounce(WireError),
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingAvailability<'a> {
    announce: Announce<'a>,
    expires_after: RemoteControlPairingExpiresAfter,
    public_app_data: RemoteControlPairingPublicAppData<'a>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingAvailabilityObservation<'a> {
    endpoint: RemoteControlPairingEndpoint,
    observed_at: InstantMillis,
    expires_at: InstantMillis,
    hops: HopCount,
    source_interface: InterfaceId,
    public_app_data: RemoteControlPairingPublicAppData<'a>,
}

impl RemoteControlPairingAvailabilityObservation<'_> {
    #[must_use]
    pub const fn endpoint(&self) -> RemoteControlPairingEndpoint {
        self.endpoint
    }

    #[must_use]
    pub const fn observed_at(&self) -> InstantMillis {
        self.observed_at
    }

    #[must_use]
    pub const fn expires_at(&self) -> InstantMillis {
        self.expires_at
    }

    #[must_use]
    pub const fn source_interface(&self) -> InterfaceId {
        self.source_interface
    }

    #[must_use]
    pub const fn hops(&self) -> HopCount {
        self.hops
    }

    #[must_use]
    pub const fn public_app_data(&self) -> &RemoteControlPairingPublicAppData<'_> {
        &self.public_app_data
    }
}

pub struct RemoteControlPairingAvailabilityVerifyOwed {
    payload: HeaplessVec<u8, BROADCAST_MDU>,
    observed_at: InstantMillis,
    source_interface: InterfaceId,
    received_hops: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAvailabilityVerification {
    Valid,
    Invalid,
}

impl RemoteControlPairingAvailabilityVerifyOwed {
    pub(crate) fn new(
        payload: &[u8],
        observed_at: InstantMillis,
        source_interface: InterfaceId,
        received_hops: u8,
    ) -> Result<Self, RemoteControlPairingAvailabilityParseError> {
        let payload = HeaplessVec::from_slice(payload).map_err(|()| {
            RemoteControlPairingAvailabilityParseError::PayloadTooLong {
                actual: payload.len(),
                maximum: BROADCAST_MDU,
            }
        })?;
        Ok(Self {
            payload,
            observed_at,
            source_interface,
            received_hops,
        })
    }

    #[must_use]
    pub fn verify(&self) -> RemoteControlPairingAvailabilityVerification {
        match RemoteControlPairingAvailability::parse_without_signature_verification(&self.payload)
        {
            Ok(availability) if availability.announce.signature_is_valid() => {
                RemoteControlPairingAvailabilityVerification::Valid
            }
            Ok(_) | Err(_) => RemoteControlPairingAvailabilityVerification::Invalid,
        }
    }

    pub(crate) fn parse_verified(
        &self,
    ) -> Result<RemoteControlPairingAvailability<'_>, RemoteControlPairingAvailabilityParseError>
    {
        RemoteControlPairingAvailability::parse_without_signature_verification(&self.payload)
    }

    pub(crate) const fn observed_at(&self) -> InstantMillis {
        self.observed_at
    }

    pub const fn source_interface(&self) -> InterfaceId {
        self.source_interface
    }

    pub(crate) const fn received_hops(&self) -> u8 {
        self.received_hops
    }
}

impl<'a> RemoteControlPairingAvailability<'a> {
    #[must_use]
    pub const fn announce(&self) -> &Announce<'a> {
        &self.announce
    }

    #[must_use]
    pub fn pairing_identity(&self) -> RemoteControlPairingIdentity {
        RemoteControlPairingIdentity::new(self.announce.public_keys.identity_hash())
    }

    #[must_use]
    pub fn pairing_endpoint(&self) -> RemoteControlPairingEndpoint {
        self.pairing_identity().endpoint()
    }

    #[must_use]
    pub const fn expires_after(&self) -> RemoteControlPairingExpiresAfter {
        self.expires_after
    }

    #[must_use]
    pub const fn public_app_data(&self) -> &RemoteControlPairingPublicAppData<'a> {
        &self.public_app_data
    }

    #[must_use]
    pub fn into_announce(self) -> Announce<'a> {
        self.announce
    }

    pub fn parse(bytes: &'a [u8]) -> Result<Self, RemoteControlPairingAvailabilityParseError> {
        let availability = Self::parse_without_signature_verification(bytes)?;
        if !availability.announce.signature_is_valid() {
            return Err(RemoteControlPairingAvailabilityParseError::InnerAnnounce(
                AnnounceValidationError::InvalidSignature,
            ));
        }
        Ok(availability)
    }

    pub(crate) fn into_observation(
        self,
        observed_at: InstantMillis,
        source_interface: InterfaceId,
        received_hops: u8,
    ) -> (
        RemoteControlPairingAvailabilityObservation<'a>,
        Announce<'a>,
    ) {
        let endpoint = self.pairing_endpoint();
        let expires_at = self.expires_after.deadline_from(observed_at);
        (
            RemoteControlPairingAvailabilityObservation {
                endpoint,
                observed_at,
                expires_at,
                hops: HopCount(received_hops),
                source_interface,
                public_app_data: self.public_app_data,
            },
            self.announce,
        )
    }

    pub(crate) fn parse_without_signature_verification(
        bytes: &'a [u8],
    ) -> Result<Self, RemoteControlPairingAvailabilityParseError> {
        if bytes.len() > BROADCAST_MDU {
            return Err(RemoteControlPairingAvailabilityParseError::PayloadTooLong {
                actual: bytes.len(),
                maximum: BROADCAST_MDU,
            });
        }
        let (envelope_version, envelope_kind, inner_wire) = parse_availability_header(bytes)
            .map_err(RemoteControlPairingAvailabilityParseError::EnvelopeHeader)?;
        let (inner_header, inner_payload) = WirePacketHeader::parse(inner_wire)
            .map_err(RemoteControlPairingAvailabilityParseError::InnerWire)?;
        validate_inner_header(&inner_header)
            .map_err(RemoteControlPairingAvailabilityParseError::InnerHeader)?;
        let announce = Announce::from_wire_unverified(&inner_header, inner_payload)
            .map_err(RemoteControlPairingAvailabilityParseError::InnerAnnounce)?;
        if announce.dotted_name_hash != REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH {
            return Err(
                RemoteControlPairingAvailabilityParseError::UnexpectedDottedNameHash {
                    found: announce.dotted_name_hash,
                },
            );
        }
        let (signed_version, signed_kind, signed_metadata) =
            parse_availability_header(announce.app_data)
                .map_err(RemoteControlPairingAvailabilityParseError::SignedHeader)?;
        if envelope_version != signed_version || envelope_kind != signed_kind {
            return Err(RemoteControlPairingAvailabilityParseError::HeaderMismatch {
                envelope_version,
                envelope_kind,
                signed_version,
                signed_kind,
            });
        }
        let Some((expires_after, public_app_data)) = signed_metadata.split_first_chunk() else {
            return Err(RemoteControlPairingAvailabilityParseError::SignedMetadataTruncated);
        };
        let expires_after = RemoteControlPairingExpiresAfter::try_from(DurationMillis(u64::from(
            u32::from_le_bytes(*expires_after),
        )))
        .map_err(RemoteControlPairingAvailabilityParseError::InvalidExpiresAfter)?;
        let public_app_data = RemoteControlPairingPublicAppData::try_from(public_app_data)
            .map_err(RemoteControlPairingAvailabilityParseError::InvalidPublicAppData)?;
        Ok(Self {
            announce,
            expires_after,
            public_app_data,
        })
    }

    #[must_use]
    pub fn encoded_len(public_app_data: &RemoteControlPairingPublicAppData<'_>) -> usize {
        PAIRING_AVAILABILITY_OVERHEAD.saturating_add(public_app_data.as_bytes().len())
    }

    pub fn write_signed(
        signer: &impl IdentitySigner,
        announce_id: AnnounceId,
        expires_after: RemoteControlPairingExpiresAfter,
        public_app_data: RemoteControlPairingPublicAppData<'_>,
        output: &mut [u8],
    ) -> Result<usize, RemoteControlPairingAvailabilityWriteError> {
        let required = Self::encoded_len(&public_app_data);
        if output.len() < required {
            return Err(RemoteControlPairingAvailabilityWriteError::BufferTooShort {
                required,
                actual: output.len(),
            });
        }

        let signed_app_data_len = PAIRING_AVAILABILITY_SIGNED_METADATA_LEN
            .saturating_add(public_app_data.as_bytes().len());
        let mut signed_app_data = [0u8; PAIRING_AVAILABILITY_SIGNED_METADATA_LEN
            + MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN];
        let Some(signed_app_data) = signed_app_data.get_mut(..signed_app_data_len) else {
            return Err(RemoteControlPairingAvailabilityWriteError::BuildAnnounce(
                AnnounceBuildError::AnnounceTooLarge,
            ));
        };
        let Some((signed_version, signed_body)) = signed_app_data.split_first_mut() else {
            return Err(RemoteControlPairingAvailabilityWriteError::BuildAnnounce(
                AnnounceBuildError::AnnounceTooLarge,
            ));
        };
        let Some((signed_kind, signed_body)) = signed_body.split_first_mut() else {
            return Err(RemoteControlPairingAvailabilityWriteError::BuildAnnounce(
                AnnounceBuildError::AnnounceTooLarge,
            ));
        };
        let (expires_after_output, public_app_data_output) =
            signed_body.split_at_mut(PAIRING_AVAILABILITY_EXPIRES_AFTER_LEN);
        *signed_version = RemoteControlPairingAvailabilityProtocolVersion::V1.wire_value();
        *signed_kind = RemoteControlPairingAvailabilityKind::PairingAvailable.wire_value();
        expires_after_output.copy_from_slice(&expires_after.to_wire());
        public_app_data_output.copy_from_slice(public_app_data.as_bytes());

        let announce = Announce::build_signed(
            signer,
            REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH,
            announce_id,
            None,
            signed_app_data,
        )
        .map_err(RemoteControlPairingAvailabilityWriteError::BuildAnnounce)?;

        let Some((envelope_header, inner_output)) = output
            .get_mut(..required)
            .map(|output| output.split_at_mut(PAIRING_AVAILABILITY_ENVELOPE_HEADER_LEN))
        else {
            return Err(RemoteControlPairingAvailabilityWriteError::BufferTooShort {
                required,
                actual: output.len(),
            });
        };
        let inner_len = write_announce_wire_packet(&announce, 0, inner_output)
            .map_err(RemoteControlPairingAvailabilityWriteError::WriteAnnounce)?;
        let Some((envelope_version, envelope_kind)) = envelope_header.split_first_mut() else {
            return Err(RemoteControlPairingAvailabilityWriteError::BufferTooShort {
                required,
                actual: output.len(),
            });
        };
        *envelope_version = RemoteControlPairingAvailabilityProtocolVersion::V1.wire_value();
        let Some(envelope_kind) = envelope_kind.first_mut() else {
            return Err(RemoteControlPairingAvailabilityWriteError::BufferTooShort {
                required,
                actual: output.len(),
            });
        };
        *envelope_kind = RemoteControlPairingAvailabilityKind::PairingAvailable.wire_value();
        Ok(PAIRING_AVAILABILITY_ENVELOPE_HEADER_LEN.saturating_add(inner_len))
    }
}

fn parse_availability_header(
    bytes: &[u8],
) -> Result<
    (
        RemoteControlPairingAvailabilityProtocolVersion,
        RemoteControlPairingAvailabilityKind,
        &[u8],
    ),
    RemoteControlPairingAvailabilityHeaderError,
> {
    let Some((version, rest)) = bytes.split_first() else {
        return Err(RemoteControlPairingAvailabilityHeaderError::Truncated);
    };
    let Some((kind, body)) = rest.split_first() else {
        return Err(RemoteControlPairingAvailabilityHeaderError::Truncated);
    };
    let Some(version) = RemoteControlPairingAvailabilityProtocolVersion::from_wire(*version) else {
        return Err(
            RemoteControlPairingAvailabilityHeaderError::UnsupportedVersion { found: *version },
        );
    };
    let Some(kind) = RemoteControlPairingAvailabilityKind::from_wire(*kind) else {
        return Err(RemoteControlPairingAvailabilityHeaderError::UnknownKind { found: *kind });
    };
    Ok((version, kind, body))
}

fn validate_inner_header(
    header: &WirePacketHeader,
) -> Result<(), RemoteControlPairingAvailabilityInnerHeaderError> {
    if header.ifac_flag != IfacFlag::Open {
        return Err(RemoteControlPairingAvailabilityInnerHeaderError::AuthenticatedIfac);
    }
    if header.context_flag != ContextFlag::Unset {
        return Err(RemoteControlPairingAvailabilityInnerHeaderError::RatchetPresent);
    }
    if header.propagation != PropagationType::Broadcast {
        return Err(RemoteControlPairingAvailabilityInnerHeaderError::TransportPropagation);
    }
    if header.destination_type != DestinationType::Single {
        return Err(
            RemoteControlPairingAvailabilityInnerHeaderError::UnexpectedDestinationType {
                found: header.destination_type,
            },
        );
    }
    if header.packet_type != PacketType::Announce {
        return Err(
            RemoteControlPairingAvailabilityInnerHeaderError::UnexpectedPacketType {
                found: header.packet_type,
            },
        );
    }
    if header.hops != 0 {
        return Err(
            RemoteControlPairingAvailabilityInnerHeaderError::NonZeroHops { found: header.hops },
        );
    }
    if header.transport_id.is_some() {
        return Err(RemoteControlPairingAvailabilityInnerHeaderError::TransportIdPresent);
    }
    if header.context != crate::wire::WireContext::None {
        return Err(
            RemoteControlPairingAvailabilityInnerHeaderError::UnexpectedContext {
                found: header.context,
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::IDENTITY_SECRET_KEY_LEN;
    use crate::routing::announce::{derive_plain_destination_hash, expand_name, AnnounceEntropy};
    use crate::wire::TRUNCATED_HASH_BYTE_LEN;

    fn signer() -> InMemoryNodeIdentity {
        InMemoryNodeIdentity::from_secret_key_bytes(&[0x41; IDENTITY_SECRET_KEY_LEN])
    }

    fn announce_id() -> AnnounceId {
        AnnounceId::mint(
            AnnounceEntropy::new([0x51; AnnounceEntropy::LEN]),
            InstantMillis(1_000),
        )
    }

    fn expires_after(millis: u64) -> RemoteControlPairingExpiresAfter {
        RemoteControlPairingExpiresAfter::try_from(DurationMillis(millis)).unwrap()
    }

    fn public_app_data(bytes: &[u8]) -> RemoteControlPairingPublicAppData<'_> {
        RemoteControlPairingPublicAppData::try_from(bytes).unwrap()
    }

    fn write_availability(app_data: &[u8]) -> ([u8; BROADCAST_MDU], usize) {
        let mut output = [0u8; BROADCAST_MDU];
        let len = RemoteControlPairingAvailability::write_signed(
            &signer(),
            announce_id(),
            expires_after(60_000),
            public_app_data(app_data),
            &mut output,
        )
        .unwrap();
        (output, len)
    }

    fn write_availability_with_signed_app_data(
        signed_app_data: &[u8],
    ) -> ([u8; BROADCAST_MDU], usize) {
        write_availability_with_name_hash(REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH, signed_app_data)
    }

    fn write_availability_with_name_hash(
        dotted_name_hash: DottedNameHash,
        signed_app_data: &[u8],
    ) -> ([u8; BROADCAST_MDU], usize) {
        let announce = Announce::build_signed(
            &signer(),
            dotted_name_hash,
            announce_id(),
            None,
            signed_app_data,
        )
        .unwrap();
        let mut output = [0u8; BROADCAST_MDU];
        let (envelope, inner) = output.split_at_mut(PAIRING_AVAILABILITY_ENVELOPE_HEADER_LEN);
        envelope.copy_from_slice(&[
            RemoteControlPairingAvailabilityProtocolVersion::V1.wire_value(),
            RemoteControlPairingAvailabilityKind::PairingAvailable.wire_value(),
        ]);
        let inner_len = write_announce_wire_packet(&announce, 0, inner).unwrap();
        (
            output,
            PAIRING_AVAILABILITY_ENVELOPE_HEADER_LEN.saturating_add(inner_len),
        )
    }

    fn assert_inner_header_byte_is_rejected(
        offset: usize,
        value: u8,
        expected: RemoteControlPairingAvailabilityInnerHeaderError,
    ) {
        let (mut encoded, encoded_len) = write_availability(b"");
        *encoded.get_mut(offset).unwrap() = value;
        assert_eq!(
            RemoteControlPairingAvailability::parse(encoded.get(..encoded_len).unwrap()),
            Err(RemoteControlPairingAvailabilityParseError::InnerHeader(
                expected
            )),
        );
    }

    #[test]
    fn pairing_availability_uses_the_pairing_name_as_a_canonical_plain_destination() {
        let dotted_name_hash = expand_name(
            super::super::REMOTE_CONTROL_PAIRING_APPLICATION_NAME,
            super::super::REMOTE_CONTROL_PAIRING_APPLICATION_ASPECTS,
        )
        .unwrap();
        assert_eq!(dotted_name_hash, REMOTE_CONTROL_PAIRING_DOTTED_NAME_HASH);
        let canonical_hash = derive_plain_destination_hash(&dotted_name_hash);
        let canonical = RemoteControlPairingAvailabilityDestination::canonical();
        assert_eq!(
            RemoteControlPairingAvailabilityDestination::try_from(canonical_hash),
            Ok(canonical),
        );
        assert_eq!(canonical.destination_hash(), canonical_hash);
        let noncanonical = DestinationHash::new([0xA5; 16]);
        assert_eq!(
            RemoteControlPairingAvailabilityDestination::try_from(noncanonical),
            Err(
                RemoteControlPairingAvailabilityDestinationError::NonCanonical {
                    found: noncanonical,
                }
            ),
        );
    }

    #[test]
    fn expires_after_is_nonzero_bounded_and_derives_a_local_deadline() {
        assert_eq!(
            RemoteControlPairingExpiresAfter::try_from(DurationMillis(0)),
            Err(RemoteControlPairingExpiresAfterError::Zero),
        );
        assert_eq!(
            RemoteControlPairingExpiresAfter::try_from(DurationMillis(600_001)),
            Err(RemoteControlPairingExpiresAfterError::TooLong {
                actual: DurationMillis(600_001),
                maximum: MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER,
            }),
        );
        let maximum =
            RemoteControlPairingExpiresAfter::try_from(MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER)
                .unwrap();
        assert_eq!(maximum.duration(), DurationMillis(600_000));
        assert_eq!(
            maximum.deadline_from(InstantMillis(1_000)),
            InstantMillis(601_000),
        );
        assert_eq!(
            maximum.deadline_from(InstantMillis(u64::MAX)),
            InstantMillis(u64::MAX),
        );
    }

    #[test]
    fn owned_public_app_data_preserves_the_exact_wire_bound() {
        let maximum = [0xA5; MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN];
        let owned = RemoteControlPairingPublicAppDataBytes::try_from(maximum.as_slice()).unwrap();
        assert_eq!(owned.as_bytes(), maximum);
        assert_eq!(owned.as_borrowed(), public_app_data(&maximum));

        let too_long = [0xA5; MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN + 1];
        assert_eq!(
            RemoteControlPairingPublicAppDataBytes::try_from(too_long.as_slice()),
            Err(RemoteControlPairingPublicAppDataError::TooLong {
                actual: MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN + 1,
                maximum: MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN,
            }),
        );
    }

    #[test]
    fn public_app_data_exposes_exactly_the_remaining_plain_payload_budget() {
        let maximum = [0xA5; MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN];
        let oversized = [0x5A; MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN + 1];
        assert_eq!(
            RemoteControlPairingPublicAppData::try_from(maximum.as_slice())
                .as_ref()
                .map(RemoteControlPairingPublicAppData::as_bytes),
            Ok(maximum.as_slice()),
        );
        assert_eq!(
            RemoteControlPairingPublicAppData::try_from(oversized.as_slice()),
            Err(RemoteControlPairingPublicAppDataError::TooLong {
                actual: oversized.len(),
                maximum: MAX_REMOTE_CONTROL_PAIRING_PUBLIC_APP_DATA_LEN,
            }),
        );

        let (encoded, encoded_len) = write_availability(&maximum);
        assert_eq!(encoded_len, BROADCAST_MDU);
        let parsed =
            RemoteControlPairingAvailability::parse(encoded.get(..encoded_len).unwrap()).unwrap();
        assert_eq!(parsed.public_app_data().as_bytes(), maximum.as_slice());
    }

    #[test]
    fn signed_availability_round_trips_identity_expiry_and_public_app_data() {
        let signer = signer();
        let expected_identity = RemoteControlPairingIdentity::new(signer.identity_hash());
        let public_app_data = b"Nearby node";
        let (encoded, encoded_len) = write_availability(public_app_data);

        let parsed =
            RemoteControlPairingAvailability::parse(encoded.get(..encoded_len).unwrap()).unwrap();

        assert_eq!(parsed.pairing_identity(), expected_identity);
        assert_eq!(
            parsed.pairing_endpoint().destination_hash(),
            parsed.announce().destination,
        );
        assert_eq!(parsed.expires_after(), expires_after(60_000));
        assert_eq!(parsed.public_app_data().as_bytes(), public_app_data);
        assert!(parsed.announce().signature_is_valid());
        assert_eq!(parsed.announce().ratchet, None);
    }

    #[test]
    fn writer_refuses_every_short_output() {
        let data = public_app_data(b"node");
        let required = RemoteControlPairingAvailability::encoded_len(&data);
        let mut output = [0u8; BROADCAST_MDU];
        for actual in 0..required {
            let short = output.get_mut(..actual).unwrap();
            assert_eq!(
                RemoteControlPairingAvailability::write_signed(
                    &signer(),
                    announce_id(),
                    expires_after(1),
                    public_app_data(b"node"),
                    short,
                ),
                Err(RemoteControlPairingAvailabilityWriteError::BufferTooShort {
                    required,
                    actual,
                }),
            );
        }
    }

    #[test]
    fn parser_rejects_invalid_envelope_headers_before_inner_parsing() {
        assert_eq!(
            RemoteControlPairingAvailability::parse(&[]),
            Err(RemoteControlPairingAvailabilityParseError::EnvelopeHeader(
                RemoteControlPairingAvailabilityHeaderError::Truncated,
            )),
        );
        assert_eq!(
            RemoteControlPairingAvailability::parse(&[2, 1]),
            Err(RemoteControlPairingAvailabilityParseError::EnvelopeHeader(
                RemoteControlPairingAvailabilityHeaderError::UnsupportedVersion { found: 2 },
            )),
        );
        assert_eq!(
            RemoteControlPairingAvailability::parse(&[1, 2]),
            Err(RemoteControlPairingAvailabilityParseError::EnvelopeHeader(
                RemoteControlPairingAvailabilityHeaderError::UnknownKind { found: 2 },
            )),
        );
    }

    #[test]
    fn parser_rejects_a_valid_signature_for_a_non_pairing_destination_name() {
        let signed_app_data = [
            RemoteControlPairingAvailabilityProtocolVersion::V1.wire_value(),
            RemoteControlPairingAvailabilityKind::PairingAvailable.wire_value(),
            0,
            0,
            0,
            1,
        ];
        let wrong_name = DottedNameHash::new([0xA5; crate::wire::DOTTED_NAME_HASH_BYTE_LEN]);
        let (encoded, encoded_len) =
            write_availability_with_name_hash(wrong_name, &signed_app_data);

        assert_eq!(
            RemoteControlPairingAvailability::parse(encoded.get(..encoded_len).unwrap()),
            Err(
                RemoteControlPairingAvailabilityParseError::UnexpectedDottedNameHash {
                    found: wrong_name,
                },
            ),
        );
    }

    #[test]
    fn parser_rejects_every_noncanonical_embedded_announce_header_field() {
        let meta = PAIRING_AVAILABILITY_ENVELOPE_HEADER_LEN;
        let hops = meta.saturating_add(1);
        let context = meta.saturating_add(HEADER_MIN_LEN.saturating_sub(1));
        assert_inner_header_byte_is_rejected(
            meta,
            0x81,
            RemoteControlPairingAvailabilityInnerHeaderError::AuthenticatedIfac,
        );
        assert_inner_header_byte_is_rejected(
            meta,
            0x21,
            RemoteControlPairingAvailabilityInnerHeaderError::RatchetPresent,
        );
        assert_inner_header_byte_is_rejected(
            meta,
            0x11,
            RemoteControlPairingAvailabilityInnerHeaderError::TransportPropagation,
        );
        assert_inner_header_byte_is_rejected(
            meta,
            0x05,
            RemoteControlPairingAvailabilityInnerHeaderError::UnexpectedDestinationType {
                found: DestinationType::Group,
            },
        );
        assert_inner_header_byte_is_rejected(
            meta,
            0x00,
            RemoteControlPairingAvailabilityInnerHeaderError::UnexpectedPacketType {
                found: PacketType::Data,
            },
        );
        assert_inner_header_byte_is_rejected(
            hops,
            1,
            RemoteControlPairingAvailabilityInnerHeaderError::NonZeroHops { found: 1 },
        );
        assert_inner_header_byte_is_rejected(
            meta,
            0x41,
            RemoteControlPairingAvailabilityInnerHeaderError::TransportIdPresent,
        );
        assert_inner_header_byte_is_rejected(
            context,
            crate::wire::WireContext::Resource.to_byte(),
            RemoteControlPairingAvailabilityInnerHeaderError::UnexpectedContext {
                found: crate::wire::WireContext::Resource,
            },
        );
    }

    #[test]
    fn parser_preserves_signed_metadata_failures_after_signature_verification() {
        let (zero_expiry, zero_expiry_len) = write_availability_with_signed_app_data(&[
            RemoteControlPairingAvailabilityProtocolVersion::V1.wire_value(),
            RemoteControlPairingAvailabilityKind::PairingAvailable.wire_value(),
            0,
            0,
            0,
            0,
        ]);
        assert_eq!(
            RemoteControlPairingAvailability::parse(zero_expiry.get(..zero_expiry_len).unwrap(),),
            Err(
                RemoteControlPairingAvailabilityParseError::InvalidExpiresAfter(
                    RemoteControlPairingExpiresAfterError::Zero,
                )
            ),
        );

        let (unknown_signed_kind, unknown_signed_kind_len) =
            write_availability_with_signed_app_data(&[
                RemoteControlPairingAvailabilityProtocolVersion::V1.wire_value(),
                0x7f,
            ]);
        assert_eq!(
            RemoteControlPairingAvailability::parse(
                unknown_signed_kind.get(..unknown_signed_kind_len).unwrap(),
            ),
            Err(RemoteControlPairingAvailabilityParseError::SignedHeader(
                RemoteControlPairingAvailabilityHeaderError::UnknownKind { found: 0x7f },
            )),
        );
    }

    #[test]
    fn parser_rejects_tampered_signed_data() {
        let (mut encoded, encoded_len) = write_availability(b"node");
        let last = encoded.get_mut(encoded_len.saturating_sub(1)).unwrap();
        *last ^= 0x01;

        assert_eq!(
            RemoteControlPairingAvailability::parse(encoded.get(..encoded_len).unwrap()),
            Err(RemoteControlPairingAvailabilityParseError::InnerAnnounce(
                AnnounceValidationError::InvalidSignature,
            )),
        );
    }

    #[test]
    fn parser_refuses_every_truncation_of_a_valid_availability() {
        let (encoded, encoded_len) = write_availability(b"node");
        for truncated_len in 0..encoded_len {
            assert!(
                RemoteControlPairingAvailability::parse(encoded.get(..truncated_len).unwrap(),)
                    .is_err()
            );
        }
    }

    #[test]
    fn availability_identity_hashes_remain_reticulum_sized() {
        let (encoded, encoded_len) = write_availability(b"");
        let parsed =
            RemoteControlPairingAvailability::parse(encoded.get(..encoded_len).unwrap()).unwrap();
        assert_eq!(
            parsed.pairing_identity().identity_hash().as_bytes().len(),
            TRUNCATED_HASH_BYTE_LEN,
        );
    }
}
