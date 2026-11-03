//! Explicit resource selection for the transient engine.
use super::{DirectServiceStartError, LxmfMessage};

/// Bounds retained records and simultaneously admitted network work. The byte
/// budget counts the actual record Vec slots and message buffer capacities;
/// allocator/index overhead, bounded callback/peer lanes and caller snapshots
/// are separate. Deduplication has at most one key per retained inbound record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectLimits {
    pub max_messages: usize,
    pub max_message_bytes: usize,
    pub max_active_attempts: usize,
}
impl Default for DirectLimits {
    fn default() -> Self {
        Self {
            max_messages: 1024,
            max_message_bytes: 4 * 1024 * 1024,
            max_active_attempts: 8,
        }
    }
}
impl DirectLimits {
    pub fn validate(self) -> Result<(), DirectServiceStartError> {
        let slots = self
            .max_messages
            .checked_mul(core::mem::size_of::<LxmfMessage>())
            .ok_or(DirectServiceStartError::InvalidLimits)?;
        if self.max_messages == 0
            || self.max_active_attempts == 0
            || self.max_active_attempts > self.max_messages
            || slots
                .checked_add(3 * super::MAX_BASIC_LXMF_WIRE_BYTES)
                .is_none_or(|minimum| minimum > self.max_message_bytes)
            || slots > isize::MAX as usize
        {
            return Err(DirectServiceStartError::InvalidLimits);
        }
        Ok(())
    }
}
