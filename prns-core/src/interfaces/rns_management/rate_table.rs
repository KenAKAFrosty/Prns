use alloc::vec::Vec;

use crate::units::InstantMillis;
use crate::wire::DestinationHash;

use super::message_pack::MessagePackEncoder;
use super::wire_names::{common, rate};
use super::{rns_timestamp, RnsManagementEncodeError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsAnnounceRateEntry {
    destination: DestinationHash,
    last_allowed_announce_at: InstantMillis,
    blocked_until: InstantMillis,
    rate_violations: u16,
    observed_at: Vec<InstantMillis>,
}

impl RnsAnnounceRateEntry {
    pub fn new(
        destination: DestinationHash,
        last_allowed_announce_at: InstantMillis,
        blocked_until: InstantMillis,
        rate_violations: u16,
        observed_at: Vec<InstantMillis>,
    ) -> Self {
        Self {
            destination,
            last_allowed_announce_at,
            blocked_until,
            rate_violations,
            observed_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsAnnounceRateTable {
    entries: Vec<RnsAnnounceRateEntry>,
}

impl RnsAnnounceRateTable {
    pub fn new(entries: Vec<RnsAnnounceRateEntry>) -> Self {
        Self { entries }
    }

    pub fn encode_message_pack(&self) -> Result<Vec<u8>, RnsManagementEncodeError> {
        let mut encoder = MessagePackEncoder::new();
        self.encode_into(&mut encoder)?;
        Ok(encoder.finish())
    }

    pub(crate) fn encode_into(
        &self,
        encoder: &mut MessagePackEncoder,
    ) -> Result<(), RnsManagementEncodeError> {
        encoder.array(self.entries.len())?;
        for entry in &self.entries {
            encoder.map(5)?;
            encoder.field(common::HASH)?;
            encoder.binary(entry.destination.as_bytes())?;
            encoder.field(rate::LAST)?;
            encoder.float(rns_timestamp(entry.last_allowed_announce_at));
            encoder.field(rate::VIOLATIONS)?;
            encoder.unsigned(u64::from(entry.rate_violations));
            encoder.field(rate::BLOCKED_UNTIL)?;
            if entry.blocked_until.0 == 0 {
                encoder.unsigned(0);
            } else {
                encoder.float(rns_timestamp(entry.blocked_until));
            }
            encoder.field(rate::TIMESTAMPS)?;
            encoder.array(entry.observed_at.len())?;
            for timestamp in &entry.observed_at {
                encoder.float(rns_timestamp(*timestamp));
            }
        }
        Ok(())
    }
}
