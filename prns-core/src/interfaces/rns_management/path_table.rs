use alloc::vec::Vec;

use crate::engine::RouteSnapshot;
use crate::units::InstantMillis;

use super::message_pack::MessagePackEncoder;
use super::wire_names::{common, path};
use super::{interface_name, next_hop_bytes, rns_timestamp, RnsManagementEncodeError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsPathTable {
    entries: Vec<RouteSnapshot>,
}

impl RnsPathTable {
    pub fn new(entries: Vec<RouteSnapshot>) -> Self {
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
            encoder.map(6)?;
            encoder.field(common::HASH)?;
            encoder.binary(entry.destination.as_bytes())?;
            encoder.field(path::VIA)?;
            encoder.binary(&next_hop_bytes(entry))?;
            encoder.unsigned_field(path::HOPS, u64::from(entry.hops))?;
            encoder.field(path::TIMESTAMP)?;
            encoder.float(rns_timestamp(InstantMillis(
                entry.learned_at.0.max(entry.last_relayed_at.0),
            )));
            encoder.field(path::EXPIRES)?;
            encoder.float(rns_timestamp(entry.expires_at));
            encoder.string_field(path::INTERFACE, &interface_name(entry.interface))?;
        }
        Ok(())
    }
}
