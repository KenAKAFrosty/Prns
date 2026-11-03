use crate::RedbError;

/// Canonical application-selected namespace limits, also used by optional
/// persistence and generated bindings. Fixed-width fields match the existing
/// u64 metadata slots; opening checks their representation on the current host.
/// Zero permits no usage. These are logical payload bounds, not limits on redb
/// pages, allocator overhead, resident memory or physical file size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupStoreLimits"))]
pub struct RedbLimits {
    pub max_changes: u64,
    pub max_key_bytes: u64,
    pub max_value_bytes: u64,
    pub max_staged_bytes: u64,
    pub max_live_objects: u64,
    pub max_live_bytes: u64,
    pub max_receipts: u64,
}

impl RedbLimits {
    /// Validate host representation without allocation or database mutation.
    /// This does not reserve the selected capacities or promise they are usable
    /// simultaneously; the normal transaction and live-usage checks still apply.
    pub fn validate(self) -> Result<(), RedbError> {
        self.checked().map(|_| ())
    }

    /// Reuse the portable coordinator's limits, with the same checked policy
    /// that the native backend uses. Callers need no parallel field mapping.
    pub fn transaction_limits(self) -> Result<prns_groups_store::Limits, RedbError> {
        Ok(self.checked()?.transaction)
    }

    pub(crate) fn checked(self) -> Result<CheckedLimits, RedbError> {
        let size = |value| usize::try_from(value).map_err(|_| RedbError::Configuration);
        Ok(CheckedLimits {
            transaction: prns_groups_store::Limits {
                max_changes: size(self.max_changes)?,
                max_key_bytes: size(self.max_key_bytes)?,
                max_value_bytes: size(self.max_value_bytes)?,
                max_staged_bytes: size(self.max_staged_bytes)?,
            },
            max_live_objects: size(self.max_live_objects)?,
            max_live_bytes: size(self.max_live_bytes)?,
            max_receipts: size(self.max_receipts)?,
        })
    }
}

/// Host-only execution representation. The transaction portion is the existing
/// portable type, not a second public configuration or binding record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CheckedLimits {
    pub transaction: prns_groups_store::Limits,
    pub max_live_objects: usize,
    pub max_live_bytes: usize,
    pub max_receipts: usize,
}

#[cfg(test)]
mod tests;
