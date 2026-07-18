use alloc::string::String;
use alloc::vec::Vec;

use crate::identity::IdentityHash;
use crate::routing::{BlackholeExpiry, BlackholedIdentity};
use crate::units::InstantMillis;

use super::message_pack::MessagePackEncoder;
use super::wire_names::{blackhole, common};
use super::RnsManagementEncodeError;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RnsBlackholeEntry {
    identity: IdentityHash,
    source: IdentityHash,
    expiry: BlackholeExpiry,
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsBlackholeTable {
    entries: Vec<RnsBlackholeEntry>,
}

impl RnsBlackholeTable {
    pub fn from_entries<Reason: AsRef<str>>(
        entries: impl IntoIterator<Item = BlackholedIdentity<Reason>>,
    ) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|entry| RnsBlackholeEntry {
                    identity: entry.identity,
                    source: entry.source,
                    expiry: entry.expiry,
                    reason: entry.reason.map(|reason| String::from(reason.as_ref())),
                })
                .collect(),
        }
    }

    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
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
        encoder.map(self.entries.len())?;
        for entry in &self.entries {
            encoder.binary(entry.identity.as_bytes())?;
            encoder.map(3)?;
            encoder.field(blackhole::SOURCE)?;
            encoder.binary(entry.source.as_bytes())?;
            encoder.field(common::UNTIL)?;
            match entry.expiry {
                BlackholeExpiry::Indefinite => encoder.nil(),
                BlackholeExpiry::At(at) => encoder.float(blackhole_timestamp(at)),
            }
            encoder.field(common::REASON)?;
            match entry.reason.as_deref() {
                Some(reason) => encoder.string(reason)?,
                None => encoder.nil(),
            }
        }
        Ok(())
    }
}

fn blackhole_timestamp(timestamp: InstantMillis) -> f64 {
    timestamp.0 as f64 / 1_000.0
}
