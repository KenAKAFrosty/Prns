use alloc::vec::Vec;
use hopspot_hub_core::{
    DeviceLabel, DeviceLabelError, MAX_DEVICE_LABEL_BYTES, RememberedDevice, RememberedPairing,
};
use personal_rns::crypto::{Ed25519PublicKey, X25519PublicKey, sha256};
use personal_rns::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
};

const MAGIC: &[u8; 8] = b"HOPDEV01";
const CHECKSUM_LEN: usize = 32;
pub(super) const ARCHIVE_OVERHEAD: u64 = 44;
pub(super) const MAX_RECORD_BYTES: u64 = 194;

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceArchiveError {
    Truncated,
    ChecksumMismatch,
    UnsupportedFormat,
    TooManyDevices { count: u32, maximum: u32 },
    InvalidLabelUtf8,
    Label(DeviceLabelError),
    InvalidPairingTag { tag: u8 },
    TrailingBytes,
}

pub(super) fn encode(devices: &[RememberedDevice]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(devices.len() as u32).to_le_bytes());
    for device in devices {
        let label = device.label.as_str().as_bytes();
        bytes.push(label.len() as u8);
        bytes.extend_from_slice(label);
        match &device.pairing {
            RememberedPairing::Unpaired => bytes.push(0),
            RememberedPairing::Paired { target } => {
                bytes.push(1);
                bytes.extend_from_slice(&target.public_key_bytes());
            }
        }
    }
    bytes.extend_from_slice(&sha256(&bytes));
    bytes
}

pub(super) fn decode(
    bytes: &[u8],
    maximum: u32,
) -> Result<Vec<RememberedDevice>, DeviceArchiveError> {
    let payload_len = bytes
        .len()
        .checked_sub(CHECKSUM_LEN)
        .ok_or(DeviceArchiveError::Truncated)?;
    let (payload, checksum) = bytes.split_at(payload_len);
    if sha256(payload).as_slice() != checksum {
        return Err(DeviceArchiveError::ChecksumMismatch);
    }
    let mut reader = Reader(payload);
    if reader.take::<8>()? != MAGIC {
        return Err(DeviceArchiveError::UnsupportedFormat);
    }
    let count = u32::from_le_bytes(*reader.take::<4>()?);
    if count > maximum {
        return Err(DeviceArchiveError::TooManyDevices { count, maximum });
    }
    let mut devices = Vec::new();
    for _ in 0..count {
        let [length] = *reader.take::<1>()?;
        let (label, rest) = reader
            .0
            .split_at_checked(usize::from(length))
            .ok_or(DeviceArchiveError::Truncated)?;
        reader.0 = rest;
        let label =
            core::str::from_utf8(label).map_err(|_| DeviceArchiveError::InvalidLabelUtf8)?;
        let label = DeviceLabel::new(label).map_err(DeviceArchiveError::Label)?;
        let pairing = match *reader.take::<1>()? {
            [0] => RememberedPairing::Unpaired,
            [1] => RememberedPairing::Paired {
                target: IdentityPublicKeys {
                    encryption: IdentityEncryptionPublicKey::new(X25519PublicKey(
                        *reader.take::<32>()?,
                    )),
                    signing: IdentitySigningPublicKey::new(Ed25519PublicKey(*reader.take::<32>()?)),
                },
            },
            [tag] => return Err(DeviceArchiveError::InvalidPairingTag { tag }),
        };
        devices.push(RememberedDevice { label, pairing });
    }
    if !reader.0.is_empty() {
        return Err(DeviceArchiveError::TrailingBytes);
    }
    Ok(devices)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take<const N: usize>(&mut self) -> Result<&'a [u8; N], DeviceArchiveError> {
        let (value, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(DeviceArchiveError::Truncated)?;
        self.0 = rest;
        Ok(value)
    }
}

const _: () = assert!(MAX_DEVICE_LABEL_BYTES == 128);
