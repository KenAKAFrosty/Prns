use super::*;

impl RedbBackend<'_> {
    /// Strictly inspect the namespace and visit each borrowed value during that
    /// same integrity pass and read snapshot, including when the cache is warm.
    /// Domain owners can validate their own key prefixes and detect orphan rows
    /// without collecting all payloads or adding scans to ordinary queries.
    ///
    /// The callback sees only rows whose native key/value bounds passed. It must
    /// not retain borrowed bytes or mutate the database. Neither callbacks nor
    /// their partial results establish validity until this method succeeds;
    /// aggregate counts and receipt continuity are also checked. An error or
    /// panic leaves prior cached validation invalidated. The mutable lease
    /// prevents callbacks from reentering this backend.
    pub fn inspect_with(
        &mut self,
        mut inspect: impl FnMut(&[u8], &[u8]) -> Result<(), RedbError>,
    ) -> Result<RedbUsage, RedbError> {
        validation::invalidate(self.validation.as_deref(), self.namespace);
        let read = self.database.begin_read().map_err(db_error)?;
        let metadata = read.open_table(self.names.metadata()).map_err(open_error)?;
        let values = read.open_table(self.names.values()).map_err(open_error)?;
        let receipts = read.open_table(self.names.receipts()).map_err(open_error)?;
        let state = State::load(&metadata, self.incarnation, self.limits)?;
        validation::inspect_values(
            self.validation.as_deref(),
            self.namespace,
            &state,
            &values,
            &receipts,
            &mut inspect,
        )?;
        Ok(state.usage())
    }
}

#[cfg(test)]
mod tests;
