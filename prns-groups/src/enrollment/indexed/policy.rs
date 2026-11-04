//! One persisted local policy. GroupProfile authenticates interoperability;
//! these limits select this owner's storage and bounded work, independently.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupCatalogLimits"))]
pub struct CatalogLimits {
    /// Optional explicit quota for current memberships. None selects no count
    /// quota; byte, operation and storage policies still govern admission.
    pub max_groups: Option<u64>,
    pub max_catalog_bytes: u64,
    pub max_enrollment_frame_bytes: u32,
    /// One encoded catalog row, before decoding. Namespace rows use namespace.
    pub max_row_bytes: u32,
    /// Changed primary/index rows in one atomic catalog projection.
    pub max_transaction_rows: u32,
    /// Owned keys, encoded values and vector slots for one catalog projection.
    /// The fixed header and one candidate encoded/decoded row are separate
    /// bounded scratch; coalesced edits charge only their final owned values.
    pub max_transaction_bytes: u64,
    /// Maximum candidates inspected in one maintenance step, including rows
    /// which cannot yet be retired. A continuation advances past those rows.
    pub max_maintenance_rows: u32,
    /// Unaccepted incoming offers, independent of configured memberships.
    pub max_incoming_invitations: u32,
    pub max_incoming_per_peer: u32,
    /// The owning backend's canonical policy, checked before creating files and
    /// fixed for every namespace initialized by this catalog. A ceiling is not
    /// a reservation: cleanup liability follows actual admitted obligations.
    pub namespace: prns_groups_store_redb::RedbLimits,
}

impl Default for CatalogLimits {
    fn default() -> Self {
        Self {
            max_groups: None,
            max_catalog_bytes: 2 * 1024 * 1024,
            max_enrollment_frame_bytes: crate::GroupProfile::DEFAULT.max_enrollment_frame_bytes,
            max_row_bytes: 16 * 1024,
            max_transaction_rows: 128,
            max_transaction_bytes: 4 * 1024 * 1024,
            max_maintenance_rows: 32,
            max_incoming_invitations: 4,
            max_incoming_per_peer: 2,
            namespace: profile::store_limits(),
        }
    }
}

impl CatalogLimits {
    pub fn validate(self) -> Result<(), EnrollmentError> {
        self.namespace
            .validate()
            .map_err(|_| EnrollmentError::Invalid)?;
        let rows =
            usize::try_from(self.max_transaction_rows).map_err(|_| EnrollmentError::Invalid)?;
        let bytes =
            usize::try_from(self.max_transaction_bytes).map_err(|_| EnrollmentError::Invalid)?;
        let row_bytes =
            usize::try_from(self.max_row_bytes).map_err(|_| EnrollmentError::Invalid)?;
        let minimum_transaction = row_bytes
            .checked_add(std::mem::size_of::<keys::TerminalSubjectKey>())
            .and_then(|n| n.checked_add(write::row_slot_bytes()))
            .ok_or(EnrollmentError::Invalid)?;
        if self.max_groups == Some(0)
            || self.max_enrollment_frame_bytes == 0
            || usize::try_from(self.max_enrollment_frame_bytes).is_err()
            || row_bytes == 0
            || rows == 0
            || bytes > isize::MAX as usize
            || bytes < minimum_transaction
            || self.max_catalog_bytes < u64::from(self.max_row_bytes)
            || self.max_maintenance_rows == 0
            || usize::try_from(self.max_maintenance_rows).is_err()
            || self.max_incoming_per_peer > self.max_incoming_invitations
            || (self.max_incoming_invitations != 0 && self.max_incoming_per_peer == 0)
        {
            return Err(EnrollmentError::Invalid);
        }
        Ok(())
    }

    pub(super) fn transaction(self) -> Result<write::Budget, EnrollmentError> {
        self.validate()?;
        Ok(write::Budget {
            rows: usize::try_from(self.max_transaction_rows)
                .map_err(|_| EnrollmentError::Invalid)?,
            bytes: usize::try_from(self.max_transaction_bytes)
                .map_err(|_| EnrollmentError::Invalid)?,
            row_bytes: usize::try_from(self.max_row_bytes).map_err(|_| EnrollmentError::Invalid)?,
        })
    }

    pub(super) fn row_bytes(self) -> Result<usize, EnrollmentError> {
        usize::try_from(self.max_row_bytes).map_err(|_| EnrollmentError::Corrupt)
    }
}
