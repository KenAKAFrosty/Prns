//! Exact membership selection and bounded descending history traversal. Local
//! deletion fences reads before its physical namespace purge has finished.
use super::*;
use indexed::{keys, rows::Incarnation};

impl Catalog {
    fn history_incarnation(
        &self,
        group: GroupId,
        incarnation: StoreIncarnation,
    ) -> Result<Option<(u32, Incarnation)>, EnrollmentError> {
        let Some(current) = self.current(group.0)?.filter(|row| row.visible) else {
            return Ok(None);
        };
        let Some(key) = self
            .incarnation_ids
            .get(incarnation.as_bytes().as_slice())
            .map_err(db_error)?
        else {
            return Ok(None);
        };
        let key: keys::IncarnationKey = key
            .value()
            .try_into()
            .map_err(|_| EnrollmentError::Corrupt)?;
        let (bound_group, generation) = keys::split_incarnation(&key)?;
        if bound_group != group.0 || generation > current.generation {
            return Ok(None);
        }
        if current
            .purge_through
            .is_some_and(|floor| generation <= floor)
        {
            return Ok(None);
        }
        let row = self.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
        if row.store_incarnation() != *incarnation.as_bytes() {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(Some((generation, row)))
    }

    fn history_entry(
        &self,
        group: GroupId,
        incarnation: StoreIncarnation,
    ) -> Result<Option<Entry>, EnrollmentError> {
        let Some((_, row)) = self.history_incarnation(group, incarnation)? else {
            return Ok(None);
        };
        match row {
            Incarnation::Live(entry) | Incarnation::Archived(entry) => Ok(Some(entry)),
            Incarnation::Retired(_) => Ok(None),
        }
    }
}

impl<'db> Enrollment<'db> {
    /// Select at most one page plus one unopened continuation. The existing
    /// inverse index resolves a caller's exact incarnation before the range;
    /// there is no scan to find a cursor among older memberships.
    pub(crate) fn history_configurations(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        group: GroupId,
        start: Option<StoreIncarnation>,
        limit: usize,
    ) -> Result<history::HistoryConfigurations, EnrollmentError> {
        if limit == 0 || limit > crate::MAX_HISTORY_NAMESPACES_PER_PAGE {
            return Err(EnrollmentError::Invalid);
        }
        let catalog = self.read()?;
        let current = Self::find(&catalog, group.0)?;
        let current_incarnation =
            StoreIncarnation::new(current.incarnation).map_err(|_| EnrollmentError::Corrupt)?;
        let summary = catalog.current(group.0)?.ok_or(EnrollmentError::Corrupt)?;
        let lower_generation = summary.purge_through.map_or(Ok(0), |floor| {
            floor.checked_add(1).ok_or(EnrollmentError::Corrupt)
        })?;
        let start_generation = match start {
            Some(incarnation) => {
                catalog
                    .history_incarnation(group, incarnation)?
                    .ok_or(EnrollmentError::Invalid)?
                    .0
            }
            None => current.generation,
        };
        if start_generation < lower_generation {
            return Err(EnrollmentError::Invalid);
        }
        let lower = keys::incarnation(group.0, lower_generation);
        let upper = keys::incarnation(group.0, start_generation);
        let mut configurations = Vec::new();
        configurations
            .try_reserve_exact(limit)
            .map_err(|_| EnrollmentError::Capacity)?;
        let mut next = None;
        for (visited, row) in catalog
            .incarnations
            .range::<&[u8]>(lower.as_slice()..=upper.as_slice())
            .map_err(db_error)?
            .rev()
            .take(limit + 1)
            .enumerate()
        {
            let (key, _) = row.map_err(db_error)?;
            let key = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let row = catalog.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
            if visited == limit {
                next = Some(
                    StoreIncarnation::new(row.store_incarnation())
                        .map_err(|_| EnrollmentError::Corrupt)?,
                );
                break;
            }
            let entry = match row {
                Incarnation::Live(entry) | Incarnation::Archived(entry) => entry,
                // Failed, unjoined attempts have no history, but still consume
                // this page's candidate budget and can be continuation keys.
                Incarnation::Retired(_) => continue,
            };
            configurations.push(self.provisioned_entry(owner, &entry, &catalog)?);
        }
        Ok(history::HistoryConfigurations {
            current_incarnation,
            configurations,
            next,
        })
    }

    /// A record ID alone cannot select among independent memberships.
    pub(crate) fn history_configuration(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        group: GroupId,
        incarnation: StoreIncarnation,
    ) -> Result<Provisioned, EnrollmentError> {
        self.recovery_configuration(owner, group, incarnation)?
            .ok_or(EnrollmentError::Invalid)
    }

    /// No retained namespace is not permission to execute the request again.
    /// The caller must independently pass fresh membership/time admission.
    pub(crate) fn recovery_configuration(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        group: GroupId,
        incarnation: StoreIncarnation,
    ) -> Result<Option<Provisioned>, EnrollmentError> {
        let catalog = self.read()?;
        catalog
            .history_entry(group, incarnation)?
            .map(|entry| self.provisioned_entry(owner, &entry, &catalog))
            .transpose()
    }
}

pub(crate) struct HistoryConfigurations {
    pub current_incarnation: StoreIncarnation,
    pub configurations: Vec<Provisioned>,
    pub next: Option<StoreIncarnation>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn exact_history_selection_ignores_unrelated_rows_and_obeys_delete_generation() {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::create(dir.path().join("history-index")).unwrap();
        let identity = IdentityHash::new([73; 16]);
        let enrollment = Enrollment::create_new(&database, identity).unwrap();
        let mut owner = RedbStoreOwner::new(&database);
        let group = enrollment
            .create(
                &mut owner,
                prns_groups_core::retry::RequestToken::new(1_800_000_000, [7; 20])
                    .unwrap()
                    .into_bytes(),
                "history",
                1_800_000_000,
            )
            .unwrap();
        let read = enrollment.read().unwrap();
        let old = Enrollment::find(&read, group.0).unwrap();
        let old_incarnation = StoreIncarnation::new(old.incarnation).unwrap();
        // Only catalog selection is under test here. These synthetic descriptors
        // make no claim about MLS joining or opening the new namespace.
        let mut next = old.clone();
        next.generation += 1;
        next.namespace[0] ^= 1;
        next.incarnation[0] ^= 1;
        let next_incarnation = StoreIncarnation::new(next.incarnation).unwrap();
        let mut current = read.current(group.0).unwrap().unwrap();
        current.generation = next.generation;
        let edit = Edit::new(&read)
            .unwrap()
            .put_incarnation(
                &read,
                keys::incarnation(group.0, old.generation),
                &Incarnation::Archived(old.clone()),
            )
            .unwrap()
            .put_incarnation(
                &read,
                keys::incarnation(group.0, next.generation),
                &Incarnation::Live(next.clone()),
            )
            .unwrap()
            .put_current(&read, group.0, &current)
            .unwrap();
        enrollment.write(edit.finish().unwrap()).unwrap();
        let read = enrollment.read().unwrap();
        assert_eq!(
            read.history_entry(group, old_incarnation)
                .unwrap()
                .unwrap()
                .generation,
            old.generation
        );
        assert_eq!(
            read.history_entry(group, next_incarnation)
                .unwrap()
                .unwrap()
                .generation,
            next.generation
        );
        let mut different_group = group;
        different_group.0[0] ^= 1;
        assert!(read
            .history_entry(different_group, old_incarnation)
            .unwrap()
            .is_none());

        let old_key = keys::incarnation(group.0, old.generation);
        let bytes = read
            .incarnations
            .get(old_key.as_slice())
            .unwrap()
            .unwrap()
            .value()
            .to_vec();
        let tx = database.begin_write().unwrap();
        tx.open_table(indexed::INCARNATIONS)
            .unwrap()
            .insert(old_key.as_slice(), b"{".as_slice())
            .unwrap();
        tx.commit().unwrap();
        let read = enrollment.read().unwrap();
        assert!(read
            .history_entry(group, next_incarnation)
            .unwrap()
            .is_some());
        assert!(matches!(
            read.history_entry(group, old_incarnation),
            Err(EnrollmentError::Corrupt)
        ));
        let tx = database.begin_write().unwrap();
        tx.open_table(indexed::INCARNATIONS)
            .unwrap()
            .insert(old_key.as_slice(), bytes.as_slice())
            .unwrap();
        tx.commit().unwrap();

        let read = enrollment.read().unwrap();
        current.purge_through = Some(old.generation);
        let edit = Edit::new(&read)
            .unwrap()
            .put_current(&read, group.0, &current)
            .unwrap();
        enrollment.write(edit.finish().unwrap()).unwrap();
        let read = enrollment.read().unwrap();
        assert!(read
            .history_entry(group, old_incarnation)
            .unwrap()
            .is_none());
        assert!(read
            .history_entry(group, next_incarnation)
            .unwrap()
            .is_some());
        assert!(read.incarnations.get(old_key.as_slice()).unwrap().is_some());
    }
}
