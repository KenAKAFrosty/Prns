use super::*;
use crate::{Dependencies, DependencyScan, MAX_DEPENDENCY_NAMESPACES_PER_STEP};
use prns_groups_store_redb::change;

impl GroupManager<'_> {
    /// Start read-only inspection of a selected managed group domain. The
    /// optional change-stamp schema must have been explicitly initialized by
    /// the physical owner. Neither this method nor a step performs recovery.
    pub fn begin_dependency_scan(&self) -> Result<DependencyScan, Error> {
        let stamp = change::read(self.database)?.ok_or(Error::Configuration)?;
        let mut result = self.provisioning.managed()?.dependency_seed(stamp)?;
        if let Some(retained) = &self.retained {
            retained.dependencies(&mut result)?;
        }
        #[cfg(feature = "archive")]
        {
            use redb::ReadableDatabase;
            let read = self.database.begin_read().map_err(|e| {
                Error::Storage(prns_groups_store_redb::RedbError::Database(e.into()))
            })?;
            if let Some(usage) = crate::archive::usage_in(&read, self.identity)? {
                result.archived_records = usage.records;
                result.archived_bytes = usage.bytes;
            }
        }
        if change::read(self.database)? != Some(stamp) {
            return Err(Error::Busy);
        }
        Ok(DependencyScan {
            identity: self.identity,
            next: crate::dependencies::CatalogCursor::Incarnations(None),
            result,
        })
    }

    /// Visit at most `namespace_budget` catalog rows, opening at most that many
    /// namespaces. None requests another
    /// bounded turn; Some contains the complete unchanged revision. Busy means
    /// the caller must discard the continuation and start again. Any other
    /// error means evidence is incomplete, never that dependencies are empty.
    pub fn advance_dependency_scan(
        &mut self,
        scan: &mut DependencyScan,
        namespace_budget: usize,
    ) -> Result<Option<Dependencies>, Error> {
        if namespace_budget == 0 || namespace_budget > MAX_DEPENDENCY_NAMESPACES_PER_STEP {
            return Err(Error::InvalidInput);
        }
        if scan.identity != self.identity {
            return Err(Error::Configuration);
        }
        if change::read(self.database)? != Some(scan.result.stamp) {
            return Err(Error::Busy);
        }
        let mut result = scan.result;
        let mut next = scan.next;
        self.provisioning.managed()?.dependency_page(
            &mut self.owner,
            &mut next,
            namespace_budget,
            &mut result,
        )?;
        if change::read(self.database)? != Some(scan.result.stamp) {
            return Err(Error::Busy);
        }
        // A failed page never leaks partial totals into the next attempt.
        scan.result = result;
        scan.next = next;
        Ok((next == crate::dependencies::CatalogCursor::Complete).then_some(result))
    }
}
