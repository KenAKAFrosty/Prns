use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::crypto::sha256_chunks;
use crate::identity::{DecryptError, IdentityHash};
use crate::interfaces::{InterfaceId, InterfaceOriginKind};
use crate::routing::announce::AnnounceObservation;
use crate::units::{HopCount, InstantMillis};
use crate::wire::TransportId;

use super::{
    decode_advertisement, decode_envelope, discovery_destination_hash, AdvertisementHash,
    DiscoveryAdvertisement, DiscoveryDecodeError, DiscoveryEnvelopeBody, DiscoveryEnvelopeError,
    InterfaceDiscoveryPolicy, StampCost, StampValidation, StampValue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryEnvelopeSecurity {
    Plaintext,
    NetworkEncrypted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryProvenance {
    pub announced_by: IdentityHash,
    pub hops: HopCount,
    pub received_on: InterfaceId,
    pub received_at: InstantMillis,
    pub envelope_security: DiscoveryEnvelopeSecurity,
    pub signed_flag: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceOrigin {
    Configured,
    Discovered(DiscoveryProvenance),
}

impl InterfaceOrigin {
    pub const fn kind(&self) -> InterfaceOriginKind {
        match self {
            Self::Configured => InterfaceOriginKind::Configured,
            Self::Discovered(_) => InterfaceOriginKind::Discovered,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoveredInterfaceId([u8; 32]);

impl DiscoveredInterfaceId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, PartialEq)]
pub struct DiscoveredInterface {
    pub id: DiscoveredInterfaceId,
    pub name: String,
    pub advertisement: DiscoveryAdvertisement,
    pub stamp_value: StampValue,
    pub provenance: DiscoveryProvenance,
}

impl DiscoveredInterface {
    pub const fn origin(&self) -> InterfaceOrigin {
        InterfaceOrigin::Discovered(self.provenance)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryNotApplicable {
    Disabled,
    PathResponse,
    DifferentAspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryDecryptionError {
    NetworkIdentityUnavailable,
    Identity(DecryptError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryRejectionKind {
    UnauthorizedSource,
    MalformedEnvelope,
    Decryption,
    StampBelowCost,
    MalformedAdvertisement,
    InvalidReachableOn,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryRejection {
    UnauthorizedSource {
        source: IdentityHash,
    },
    MalformedEnvelope(DiscoveryEnvelopeError),
    Decryption(DiscoveryDecryptionError),
    StampBelowCost {
        value: StampValue,
        required: StampCost,
    },
    MalformedAdvertisement(DiscoveryDecodeError),
    InvalidReachableOn {
        value: String,
    },
}

impl DiscoveryRejection {
    pub const fn kind(&self) -> DiscoveryRejectionKind {
        match self {
            Self::UnauthorizedSource { .. } => DiscoveryRejectionKind::UnauthorizedSource,
            Self::MalformedEnvelope(_) => DiscoveryRejectionKind::MalformedEnvelope,
            Self::Decryption(_) => DiscoveryRejectionKind::Decryption,
            Self::StampBelowCost { .. } => DiscoveryRejectionKind::StampBelowCost,
            Self::MalformedAdvertisement(_) => DiscoveryRejectionKind::MalformedAdvertisement,
            Self::InvalidReachableOn { .. } => DiscoveryRejectionKind::InvalidReachableOn,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum DiscoveryIntake {
    NotApplicable(DiscoveryNotApplicable),
    Rejected(DiscoveryRejection),
    Discovered(Box<DiscoveredInterface>),
}

pub fn ingest_discovery_announce(
    policy: &InterfaceDiscoveryPolicy,
    observation: AnnounceObservation<'_>,
    decrypt: impl FnOnce(&[u8]) -> Result<Vec<u8>, DiscoveryDecryptionError>,
) -> DiscoveryIntake {
    let Some(enabled) = policy.enabled_policy() else {
        return DiscoveryIntake::NotApplicable(DiscoveryNotApplicable::Disabled);
    };
    if observation.is_path_response {
        return DiscoveryIntake::NotApplicable(DiscoveryNotApplicable::PathResponse);
    }
    let expected_destination = discovery_destination_hash(&observation.announced_identity);
    if observation.destination != expected_destination {
        return DiscoveryIntake::NotApplicable(DiscoveryNotApplicable::DifferentAspect);
    }
    if !enabled.sources().accepts(&observation.announced_identity) {
        return DiscoveryIntake::Rejected(DiscoveryRejection::UnauthorizedSource {
            source: observation.announced_identity,
        });
    }
    if observation.app_data.len() <= super::STAMP_SIZE + 1 {
        return DiscoveryIntake::Rejected(DiscoveryRejection::MalformedEnvelope(
            DiscoveryEnvelopeError::PayloadTooShort,
        ));
    }

    let envelope = match decode_envelope(observation.app_data) {
        Ok(envelope) => envelope,
        Err(error) => {
            return DiscoveryIntake::Rejected(DiscoveryRejection::MalformedEnvelope(error));
        }
    };
    let signed_flag = envelope.signed;
    let (body, envelope_security) = match envelope.body {
        DiscoveryEnvelopeBody::Plaintext {
            packed_advertisement,
            stamp,
        } => (
            PlaintextBody::Borrowed {
                packed_advertisement,
                stamp,
            },
            DiscoveryEnvelopeSecurity::Plaintext,
        ),
        DiscoveryEnvelopeBody::Encrypted { ciphertext } => {
            let decrypted = match decrypt(ciphertext) {
                Ok(decrypted) => decrypted,
                Err(error) => {
                    return DiscoveryIntake::Rejected(DiscoveryRejection::Decryption(error));
                }
            };
            let (packed_len, stamp) = match split_plaintext(&decrypted) {
                Some((packed_advertisement, stamp)) => (packed_advertisement.len(), *stamp),
                None => {
                    return DiscoveryIntake::Rejected(DiscoveryRejection::MalformedEnvelope(
                        DiscoveryEnvelopeError::MissingPlaintextOrStamp,
                    ));
                }
            };
            (
                PlaintextBody::Owned {
                    decrypted,
                    packed_len,
                    stamp,
                },
                DiscoveryEnvelopeSecurity::NetworkEncrypted,
            )
        }
    };

    finish_intake(
        enabled.required_stamp_cost(),
        body,
        DiscoveryProvenance {
            announced_by: observation.announced_identity,
            hops: observation.hops,
            received_on: observation.source_interface,
            received_at: observation.arrived_at,
            envelope_security,
            signed_flag,
        },
    )
}

enum PlaintextBody<'a> {
    Borrowed {
        packed_advertisement: &'a [u8],
        stamp: &'a [u8; super::STAMP_SIZE],
    },
    Owned {
        decrypted: Vec<u8>,
        packed_len: usize,
        stamp: [u8; super::STAMP_SIZE],
    },
}

impl PlaintextBody<'_> {
    fn parts(&self) -> (&[u8], &[u8; super::STAMP_SIZE]) {
        match self {
            Self::Borrowed {
                packed_advertisement,
                stamp,
            } => (packed_advertisement, stamp),
            Self::Owned {
                decrypted,
                packed_len,
                stamp,
            } => (&decrypted[..*packed_len], stamp),
        }
    }
}

fn split_plaintext(bytes: &[u8]) -> Option<(&[u8], &[u8; super::STAMP_SIZE])> {
    let split = bytes.len().checked_sub(super::STAMP_SIZE)?;
    if split == 0 {
        return None;
    }
    let (packed, stamp) = bytes.split_at(split);
    Some((packed, stamp.try_into().ok()?))
}

fn finish_intake(
    required_stamp_cost: StampCost,
    body: PlaintextBody<'_>,
    provenance: DiscoveryProvenance,
) -> DiscoveryIntake {
    let (packed, stamp) = body.parts();
    let advertisement_hash = AdvertisementHash::for_advertisement(packed);
    let stamp_value = match super::validate_stamp(&advertisement_hash, stamp, required_stamp_cost) {
        StampValidation::MeetsCost { value } => value,
        StampValidation::BelowCost { value, required } => {
            return DiscoveryIntake::Rejected(DiscoveryRejection::StampBelowCost {
                value,
                required,
            });
        }
    };
    let advertisement = match decode_advertisement(packed) {
        Ok(advertisement) => advertisement,
        Err(error) => {
            return DiscoveryIntake::Rejected(DiscoveryRejection::MalformedAdvertisement(error));
        }
    };
    if let Some(value) = super::advertisement::invalid_reachable_on(&advertisement) {
        return DiscoveryIntake::Rejected(DiscoveryRejection::InvalidReachableOn {
            value: String::from(value),
        });
    }
    let name = effective_name(&advertisement);
    let id = discovered_interface_id(advertisement.transport.transport_id(), &name);
    DiscoveryIntake::Discovered(Box::new(DiscoveredInterface {
        id,
        name,
        advertisement,
        stamp_value,
        provenance,
    }))
}

fn effective_name(advertisement: &DiscoveryAdvertisement) -> String {
    let mut name = advertisement
        .name
        .as_deref()
        .map(sanitize_name)
        .unwrap_or_default();
    if name.is_empty() {
        name = format!("Discovered {}", advertisement.interface_type.rns_name());
    }
    name
}

fn sanitize_name(name: &str) -> String {
    let ascii: String = name.chars().filter(char::is_ascii).collect();
    let mut sanitized = String::from(ascii.trim());
    for run in [5, 3, 2] {
        sanitized = sanitized.replace(&" ".repeat(run), " ");
    }
    let start = sanitized
        .as_bytes()
        .iter()
        .position(u8::is_ascii_alphanumeric)
        .unwrap_or(sanitized.len());
    let end = sanitized
        .as_bytes()
        .iter()
        .rposition(|byte| byte.is_ascii_alphanumeric() || *byte == b')')
        .map_or(start, |index| index + 1);
    if start >= end {
        String::new()
    } else {
        String::from(&sanitized[start..end])
    }
}

fn discovered_interface_id(transport: &TransportId, name: &str) -> DiscoveredInterfaceId {
    let mut transport_hex = [0u8; 32];
    let alphabet = b"0123456789abcdef";
    for (index, byte) in transport.as_bytes().iter().copied().enumerate() {
        transport_hex[index * 2] = alphabet[usize::from(byte >> 4)];
        transport_hex[index * 2 + 1] = alphabet[usize::from(byte & 0x0f)];
    }
    DiscoveredInterfaceId(sha256_chunks(&[&transport_hex, name.as_bytes()]))
}

#[cfg(test)]
mod tests;
