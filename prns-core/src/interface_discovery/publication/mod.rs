use alloc::string::String;
use alloc::vec::Vec;

use crate::crypto::sealed_len;
use crate::identity::EncryptError;
use crate::interfaces::InterfaceId;
use crate::routing::announce::emit::MAX_ANNOUNCE_APP_DATA_LEN;
use crate::units::{DurationMillis, InstantMillis};

use super::{
    encode_advertisement, encode_encrypted_envelope, encode_plaintext_envelope, generate_stamp,
    validate_stamp, AdvertisementHash, DiscoveryAdvertisement, DiscoveryEncodeError,
    GeneratedStamp, StampCost, StampGeneration, StampValidation, StampValue, STAMP_SIZE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryPublicationSecurity {
    Plaintext,
    NetworkEncrypted,
}

#[derive(Debug, PartialEq)]
pub struct PreparedDiscoveryAdvertisement {
    packed_advertisement: Vec<u8>,
    advertisement_hash: AdvertisementHash,
    generated_stamp: GeneratedStamp,
    security: DiscoveryPublicationSecurity,
}

impl PreparedDiscoveryAdvertisement {
    pub fn packed_advertisement(&self) -> &[u8] {
        &self.packed_advertisement
    }

    pub const fn advertisement_hash(&self) -> &AdvertisementHash {
        &self.advertisement_hash
    }

    pub const fn stamp(&self) -> &[u8; STAMP_SIZE] {
        &self.generated_stamp.stamp
    }

    pub const fn stamp_value(&self) -> StampValue {
        self.generated_stamp.value
    }

    pub const fn stamp_attempts(&self) -> u64 {
        self.generated_stamp.attempts
    }

    pub const fn security(&self) -> DiscoveryPublicationSecurity {
        self.security
    }

    fn plaintext_body(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(self.packed_advertisement.len() + STAMP_SIZE);
        body.extend_from_slice(&self.packed_advertisement);
        body.extend_from_slice(&self.generated_stamp.stamp);
        body
    }
}

#[derive(Debug, PartialEq)]
pub enum DiscoveryPublicationPreparation<E> {
    Prepared(PreparedDiscoveryAdvertisement),
    Cancelled,
    EncodeFailed(DiscoveryEncodeError),
    InvalidReachableOn { value: String },
    EntropyFailed(E),
    AppDataTooLong { required: usize, maximum: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryPublicationEncryptionError {
    NetworkIdentityUnavailable,
    Identity(EncryptError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryPublicationFrameError {
    Encryption(DiscoveryPublicationEncryptionError),
    EncryptionOutputLength { actual: usize, expected: usize },
    AppDataTooLong { actual: usize, maximum: usize },
}

pub fn prepare_discovery_publication<E>(
    advertisement: &DiscoveryAdvertisement,
    stamp_cost: StampCost,
    security: DiscoveryPublicationSecurity,
    fill_random: impl FnMut(&mut [u8; STAMP_SIZE]) -> Result<(), E>,
    cancelled: impl FnMut() -> bool,
) -> DiscoveryPublicationPreparation<E> {
    prepare_discovery_publication_with_stamp_cache(
        advertisement,
        stamp_cost,
        security,
        |_| None,
        fill_random,
        cancelled,
    )
}

pub fn prepare_discovery_publication_with_stamp_cache<E>(
    advertisement: &DiscoveryAdvertisement,
    stamp_cost: StampCost,
    security: DiscoveryPublicationSecurity,
    cached_stamp: impl FnOnce(&AdvertisementHash) -> Option<[u8; STAMP_SIZE]>,
    fill_random: impl FnMut(&mut [u8; STAMP_SIZE]) -> Result<(), E>,
    cancelled: impl FnMut() -> bool,
) -> DiscoveryPublicationPreparation<E> {
    if let Some(value) = super::advertisement::invalid_reachable_on(advertisement) {
        return DiscoveryPublicationPreparation::InvalidReachableOn {
            value: String::from(value),
        };
    }
    let packed_advertisement = match encode_advertisement(advertisement) {
        Ok(packed) => packed,
        Err(error) => return DiscoveryPublicationPreparation::EncodeFailed(error),
    };
    let required = projected_app_data_len(packed_advertisement.len(), security);
    if required > MAX_ANNOUNCE_APP_DATA_LEN {
        return DiscoveryPublicationPreparation::AppDataTooLong {
            required,
            maximum: MAX_ANNOUNCE_APP_DATA_LEN,
        };
    }
    let advertisement_hash = AdvertisementHash::for_advertisement(&packed_advertisement);
    if let Some(stamp) = cached_stamp(&advertisement_hash) {
        if let StampValidation::MeetsCost { value } =
            validate_stamp(&advertisement_hash, &stamp, stamp_cost)
        {
            return DiscoveryPublicationPreparation::Prepared(PreparedDiscoveryAdvertisement {
                packed_advertisement,
                advertisement_hash,
                generated_stamp: GeneratedStamp {
                    stamp,
                    value,
                    attempts: 0,
                },
                security,
            });
        }
    }
    match generate_stamp(&advertisement_hash, stamp_cost, fill_random, cancelled) {
        StampGeneration::Generated(generated_stamp) => {
            DiscoveryPublicationPreparation::Prepared(PreparedDiscoveryAdvertisement {
                packed_advertisement,
                advertisement_hash,
                generated_stamp,
                security,
            })
        }
        StampGeneration::Cancelled => DiscoveryPublicationPreparation::Cancelled,
        StampGeneration::EntropyFailure(error) => {
            DiscoveryPublicationPreparation::EntropyFailed(error)
        }
    }
}

pub fn frame_discovery_publication(
    prepared: &PreparedDiscoveryAdvertisement,
    encrypt: impl FnOnce(&[u8]) -> Result<Vec<u8>, DiscoveryPublicationEncryptionError>,
) -> Result<Vec<u8>, DiscoveryPublicationFrameError> {
    let app_data = match prepared.security {
        DiscoveryPublicationSecurity::Plaintext => encode_plaintext_envelope(
            &prepared.packed_advertisement,
            &prepared.generated_stamp.stamp,
        ),
        DiscoveryPublicationSecurity::NetworkEncrypted => {
            let plaintext = prepared.plaintext_body();
            let expected = crate::identity::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN
                .saturating_add(sealed_len(plaintext.len()));
            let ciphertext =
                encrypt(&plaintext).map_err(DiscoveryPublicationFrameError::Encryption)?;
            if ciphertext.len() != expected {
                return Err(DiscoveryPublicationFrameError::EncryptionOutputLength {
                    actual: ciphertext.len(),
                    expected,
                });
            }
            encode_encrypted_envelope(&ciphertext)
        }
    };
    if app_data.len() > MAX_ANNOUNCE_APP_DATA_LEN {
        return Err(DiscoveryPublicationFrameError::AppDataTooLong {
            actual: app_data.len(),
            maximum: MAX_ANNOUNCE_APP_DATA_LEN,
        });
    }
    Ok(app_data)
}

fn projected_app_data_len(
    packed_advertisement_len: usize,
    security: DiscoveryPublicationSecurity,
) -> usize {
    let plaintext_len = packed_advertisement_len.saturating_add(STAMP_SIZE);
    match security {
        DiscoveryPublicationSecurity::Plaintext => plaintext_len.saturating_add(1),
        DiscoveryPublicationSecurity::NetworkEncrypted => {
            if plaintext_len > MAX_ANNOUNCE_APP_DATA_LEN {
                usize::MAX
            } else {
                1usize
                    .saturating_add(crate::identity::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN)
                    .saturating_add(sealed_len(plaintext_len))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryPublicationTiming {
    pub interface: InterfaceId,
    pub interval: DurationMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryPublicationRegistration {
    pub interface: InterfaceId,
    pub interval: DurationMillis,
    pub stamp_cost: StampCost,
    pub security: DiscoveryPublicationSecurity,
}

impl DiscoveryPublicationRegistration {
    pub const fn timing(self) -> DiscoveryPublicationTiming {
        DiscoveryPublicationTiming {
            interface: self.interface,
            interval: self.interval,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryPublicationScheduleError {
    ZeroInterval { interface: InterfaceId },
    DuplicateInterface { interface: InterfaceId },
    UnknownInterface { interface: InterfaceId },
}

pub struct DiscoveryPublicationSchedule {
    entries: Vec<ScheduledPublication>,
}

struct ScheduledPublication {
    timing: DiscoveryPublicationTiming,
    last_attempt: Option<InstantMillis>,
}

impl DiscoveryPublicationSchedule {
    pub fn new(
        timings: impl IntoIterator<Item = DiscoveryPublicationTiming>,
    ) -> Result<Self, DiscoveryPublicationScheduleError> {
        let mut entries = Vec::new();
        for timing in timings {
            if timing.interval.0 == 0 {
                return Err(DiscoveryPublicationScheduleError::ZeroInterval {
                    interface: timing.interface,
                });
            }
            if entries
                .iter()
                .any(|entry: &ScheduledPublication| entry.timing.interface == timing.interface)
            {
                return Err(DiscoveryPublicationScheduleError::DuplicateInterface {
                    interface: timing.interface,
                });
            }
            entries.push(ScheduledPublication {
                timing,
                last_attempt: None,
            });
        }
        Ok(Self { entries })
    }

    pub fn next_due(&self, now: InstantMillis) -> Option<InterfaceId> {
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.last_attempt.is_none())
        {
            return Some(entry.timing.interface);
        }
        self.entries
            .iter()
            .filter(|entry| {
                entry.last_attempt.is_some_and(|last_attempt| {
                    now.0 > last_attempt.saturating_add(entry.timing.interval).0
                })
            })
            .min_by_key(|entry| entry.last_attempt)
            .map(|entry| entry.timing.interface)
    }

    pub fn record_attempt(
        &mut self,
        interface: InterfaceId,
        now: InstantMillis,
    ) -> Result<(), DiscoveryPublicationScheduleError> {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.timing.interface == interface)
        else {
            return Err(DiscoveryPublicationScheduleError::UnknownInterface { interface });
        };
        entry.last_attempt = Some(now);
        Ok(())
    }

    pub fn last_attempt(
        &self,
        interface: InterfaceId,
    ) -> Result<Option<InstantMillis>, DiscoveryPublicationScheduleError> {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.timing.interface == interface)
        else {
            return Err(DiscoveryPublicationScheduleError::UnknownInterface { interface });
        };
        Ok(entry.last_attempt)
    }
}

#[cfg(test)]
mod tests;
