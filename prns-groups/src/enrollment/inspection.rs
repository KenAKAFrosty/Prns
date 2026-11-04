//! Read-only cold readiness. Ordinary attached lexical views do not enter this
//! path. Every iteration owns one catalog row and one bounded namespace header.
use super::*;

impl Enrollment<'_> {
    pub(crate) fn inspect_namespaces(
        database: &Database,
        local: IdentityHash,
        selection: CatalogLimits,
    ) -> Result<(), EnrollmentError> {
        let transaction = database.begin_read().map_err(db_error)?;
        let catalog = Self::read_in(&transaction, local)?;
        if catalog.header.limits != selection {
            return Err(EnrollmentError::Conflict);
        }
        let before = catalog.header.fingerprint()?;
        let stamp = prns_groups_store_redb::change::read_in(&transaction)?;
        let mut owner = RedbStoreOwner::new(database);
        for row in catalog.incarnations.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let key: indexed::keys::IncarnationKey = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let (group, generation) = indexed::keys::split_incarnation(&key)?;
            let membership = catalog.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
            let current = catalog.current(group)?.ok_or(EnrollmentError::Corrupt)?;
            let current_membership = catalog
                .incarnation(indexed::keys::incarnation(group, current.generation))?
                .ok_or(EnrollmentError::Corrupt)?;
            if membership.owner() != current_membership.owner()
                || membership.profile() != current_membership.profile()
                || current.owner != membership.owner().rns
            {
                return Err(EnrollmentError::Corrupt);
            }
            let deleted = current.purge_through.is_some_and(|last| generation <= last);
            let (allow_missing, purged, title) = match &membership {
                indexed::rows::Incarnation::Live(entry)
                | indexed::rows::Incarnation::Archived(entry) => {
                    if entry.title != current.title {
                        return Err(EnrollmentError::Corrupt);
                    }
                    let admitted = current.generation == generation
                        && current.head.is_none()
                        && !current.ready
                        && matches!(&membership, indexed::rows::Incarnation::Live(_));
                    if admitted {
                        if entry.owner == entry.local {
                            if entry.invitation.is_some() {
                                return Err(EnrollmentError::Corrupt);
                            }
                        } else {
                            let invitation = entry.invitation.ok_or(EnrollmentError::Corrupt)?;
                            let binding = catalog
                                .invitation_binding(invitation)?
                                .ok_or(EnrollmentError::Corrupt)?;
                            if binding.namespace.group != group
                                || binding.namespace.generation != generation
                                || binding.namespace.incarnation != entry.incarnation
                                || binding.member != entry.local
                            {
                                return Err(EnrollmentError::Corrupt);
                            }
                        }
                    }
                    (deleted || admitted, false, entry.title.as_str())
                }
                indexed::rows::Incarnation::Retired(entry) => {
                    (true, entry.purged, current.title.as_str())
                }
            };
            let incarnation = StoreIncarnation::new(membership.store_incarnation())
                .map_err(|_| EnrollmentError::Corrupt)?;
            let mut backend = match owner.open(
                StoreNamespace(membership.namespace()),
                incarnation,
                selection.namespace,
            ) {
                Ok(backend) if !purged => backend,
                Ok(_) => return Err(EnrollmentError::Corrupt),
                Err(RedbError::MissingNamespace) if allow_missing => continue,
                Err(error) => return Err(error.into()),
            };
            let mut inspection = state::Inspection::new();
            backend.inspect_with(|key, value| inspection.observe(key, value))?;
            inspection.finish()?;
            let snapshot = state::load_backend(&mut backend, selection.namespace)?;
            let coordinator = Coordinator::new(backend, selection.namespace.transaction_limits()?)
                .map_err(|_| EnrollmentError::Corrupt)?;
            let context = state::InspectionContext {
                group,
                owner: membership.owner(),
                title,
                profile: membership.profile(),
            };
            snapshot.inspect_context(&coordinator, &context)?;
            if generation == current.generation && current.visible {
                let count = snapshot.current.as_ref().map_or(Ok(0), |signed| {
                    u32::try_from(signed.manifest.members.len())
                        .map_err(|_| EnrollmentError::Corrupt)
                })?;
                if current.lifecycle != snapshot.lifecycle
                    || current.ready != snapshot.ready()?
                    || current.head != snapshot.current.as_ref().map(Signed::head).transpose()?
                    || current.member_count != count
                {
                    return Err(EnrollmentError::Corrupt);
                }
            }
            for ordinal in 0..snapshot.added_count {
                let added = snapshot.added_at(&coordinator, ordinal)?;
                let binding = catalog
                    .invitation_binding(added.invitation)?
                    .ok_or(EnrollmentError::Corrupt)?;
                if binding.namespace.group != group
                    || binding.namespace.generation != generation
                    || binding.namespace.incarnation != membership.store_incarnation()
                    || binding.member != added.member
                {
                    return Err(EnrollmentError::Corrupt);
                }
            }
            for ordinal in 0..snapshot.terminal_count {
                let terminal = snapshot.terminal_at(&coordinator, ordinal)?;
                if catalog
                    .terminal(terminal.id()?)?
                    .is_none_or(|row| row.proof != terminal)
                {
                    return Err(EnrollmentError::Corrupt);
                }
            }
            snapshot.inspect_catalog_requests(
                &coordinator,
                &catalog,
                group,
                generation,
                membership.store_incarnation(),
            )?;
        }
        // The owning lane is exclusive; the selected physical stamp also
        // rejects an overlapping ordinary namespace commit. Standalone callers
        // without that optional schema retain the documented exclusive-write
        // contract, plus the catalog's exact header witness.
        let after = database.begin_read().map_err(db_error)?;
        if Self::read_in(&after, local)?.header.fingerprint()? != before
            || prns_groups_store_redb::change::read_in(&after)? != stamp
        {
            return Err(EnrollmentError::Conflict);
        }
        Ok(())
    }
}
