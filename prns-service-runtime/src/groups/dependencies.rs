use super::*;

impl<const I: usize> Local<I> {
    /// Poll one bounded namespace page, independent of subsequent caller jobs.
    /// The durable stamp fences scans across intervening domain writes.
    pub fn poll_observations(&mut self, context: &mut Context<'_>) {
        let count = self.entries.len();
        if count == 0 {
            return;
        }
        let mut selected = None;
        for offset in 0..count {
            let index = (self.next_scan + offset) % count;
            if self.entries[index].dirty && self.entries[index].failure.is_none() {
                selected = Some(index);
                break;
            }
        }
        let Some(index) = selected else {
            return;
        };
        self.next_scan = (index + 1) % count;
        let entry = &mut self.entries[index];
        let result = self.stores.with_existing(entry.identity, |database| {
            let stamp = prns_groups_store_redb::change::read(database)
                .map_err(|error| SchemaError::Group(groups::Error::Storage(error)))?
                .ok_or(SchemaError::Group(groups::Error::NotProvisioned))?;
            if entry
                .dependencies
                .as_ref()
                .is_some_and(|value| value.stamp == stamp)
            {
                entry.dirty = false;
                return Ok(());
            }
            entry.dependencies = None;
            let mut manager = groups::GroupManager::open_attached(database, &mut entry.attachment)
                .map_err(SchemaError::Group)?;
            if entry.scan.is_none() {
                entry.scan = Some(
                    manager
                        .begin_dependency_scan()
                        .map_err(SchemaError::Group)?,
                );
            }
            if let Some(scan) = entry.scan.as_mut() {
                match manager.advance_dependency_scan(scan, 4) {
                    Ok(Some(value)) => {
                        entry.dependencies = Some(value);
                        entry.scan = None;
                        entry.dirty = false;
                    }
                    Ok(None) => (),
                    Err(groups::Error::Busy) => {
                        entry.scan = None;
                    }
                    Err(error) => return Err(SchemaError::Group(error)),
                }
            }
            Ok(())
        });
        if let Err(error) = result {
            entry.scan = None;
            entry.dependencies = None;
            entry.failure = Some(Arc::new(error));
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.dirty && entry.failure.is_none())
        {
            context.waker().wake_by_ref();
        }
    }
    pub fn dependency(
        &mut self,
        identity: core::IdentityReference,
    ) -> Result<core::ServiceDependency, Error> {
        self.attach(identity)?;
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.identity == identity)
            .ok_or(Error::Capacity)?;
        if let Some(error) = entry.failure.take() {
            // An explicit fresh dependency query can retry a failed read. Keep
            // the original typed failure observable; never synthesize emptiness.
            entry.dirty = true;
            self.wake.wake_by_ref();
            return Err(Error::Observation(error));
        }
        let current = self
            .stores
            .with_existing(identity, |database| {
                prns_groups_store_redb::change::read(database)
                    .map_err(|error| SchemaError::Group(groups::Error::Storage(error)))?
                    .ok_or(SchemaError::Group(groups::Error::NotProvisioned))
            })
            .map_err(Error::Store)?;
        let Some(facts) = entry
            .dependencies
            .as_ref()
            .filter(|value| value.stamp == current)
        else {
            entry.dirty = true;
            self.wake.wake_by_ref();
            return Err(Error::Observing);
        };
        let pending_messages = u32::try_from(
            facts
                .pending_publication_targets
                .checked_add(facts.pending_receipts)
                .ok_or(Error::Capacity)?,
        )
        .map_err(|_| Error::Capacity)?;
        let unsettled_control = [
            facts.pending_enrollment,
            facts.pending_departures,
            facts.pending_terminal_notices,
            facts.pending_local_purges,
        ]
        .into_iter()
        .try_fold(0u64, |sum, value| sum.checked_add(value))
        .ok_or(Error::Capacity)?;
        Ok(core::ServiceDependency {
            service: SERVICE,
            revision: facts.stamp.revision,
            pending_messages,
            unsettled_control: u32::try_from(unsettled_control).map_err(|_| Error::Capacity)?,
            // Every active membership owns protocol obligations, including a
            // non-owner's membership. Passive archives and fences do not.
            authority_count: u32::try_from(facts.active_memberships)
                .map_err(|_| Error::Capacity)?,
            recovery_pending: facts.incomplete_setup != 0 || facts.unresolved_operations != 0,
            quiescence_supported: true,
        })
    }
    pub fn quiesce(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::QuiescedService>>, Error> {
        let identity = reference(scope.identity);
        let dependency = match self.dependency(identity) {
            Ok(value) => value,
            Err(Error::Observing) => return Ok(runtime::Progress::Pending),
            Err(error) => return Err(error),
        };
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.identity == identity)
            .ok_or(Error::Capacity)?;
        entry.quiesced = Some(scope.owner);
        // This concrete local attachment has never registered host resources,
        // acquired private signing uses, or admitted background effects. Durable
        // obligations above remain visible and can block retirement/removal.
        Ok(runtime::Progress::Complete(runtime::Evidence {
            scope,
            value: runtime::QuiescedService {
                evidence: runtime::QuiescenceEvidence::COMPLETE,
                dependencies: dependency,
            },
        }))
    }
    pub fn released_dependency(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<Option<core::ServiceDependency>, Error> {
        let identity = reference(scope.identity);
        if self
            .entries
            .iter()
            .any(|entry| entry.identity == identity && entry.quiesced == Some(scope.owner))
        {
            self.dependency(identity).map(Some)
        } else {
            Ok(None)
        }
    }
}
