use super::*;
pub(super) const CATALOG: TableDefinition<u8, &[u8]> =
    TableDefinition::new("prns_group_enrollment_v1");
impl<'db> Enrollment<'db> {
    pub(super) fn read(&self) -> Result<Catalog, EnrollmentError> {
        let read = Self::read_in(&self.database.begin_read().map_err(db_error)?, self.local)?;
        if read.header.limits != self.selection {
            return Err(EnrollmentError::Conflict);
        }
        Ok(read)
    }
    pub(super) fn read_in(
        tx: &redb::ReadTransaction,
        local: IdentityHash,
    ) -> Result<Catalog, EnrollmentError> {
        indexed::Read::open(tx, local)
    }

    /// Exact deletion fence in the same writer as selected archival.
    #[cfg(feature = "archive")]
    pub(crate) fn archive_binding_in(
        tx: &redb::WriteTransaction,
        local: IdentityHash,
        group: prns_groups_core::GroupId,
        namespace: StoreNamespace,
        incarnation: StoreIncarnation,
    ) -> Result<bool, EnrollmentError> {
        indexed::archive_binding_in(tx, local, group, namespace, incarnation)
    }

    pub(super) fn write(&self, catalog: indexed::Write) -> Result<(), EnrollmentError> {
        if catalog.before == catalog.after {
            return Ok(());
        }
        let mut tx = self.database.begin_write().map_err(db_error)?;
        tx.set_durability(redb::Durability::Immediate)
            .map_err(db_error)?;
        catalog.stage(&tx, self.local)?;
        if let Some(attempt) = &self.attempt {
            attempt.catalog(catalog.before, catalog.after)?;
            #[cfg(test)]
            attempt::fault::before_commit()?;
        }
        crate::commit::commit(tx, self.change_witness.as_ref(), self.change_stamp)
            .map_err(EnrollmentError::Storage)
    }

    pub(super) fn find(catalog: &Catalog, group: [u8; 32]) -> Result<Entry, EnrollmentError> {
        catalog.entry(group)?.ok_or(EnrollmentError::Missing)
    }
    pub(super) fn replay(
        catalog: &Catalog,
        id: [u8; 32],
        fingerprint: [u8; 32],
    ) -> Result<Option<[u8; 32]>, EnrollmentError> {
        Ok(catalog
            .receipt(id, fingerprint)?
            .map(|row| row.request.result))
    }
    pub(super) fn remember(
        catalog: &Catalog,
        edit: Edit,
        id: [u8; 32],
        fingerprint: [u8; 32],
        result: [u8; 32],
    ) -> Result<Edit, EnrollmentError> {
        edit.put_receipt(
            catalog,
            &indexed::rows::Receipt {
                request: Request {
                    id,
                    fingerprint,
                    result,
                },
                namespace: None,
            },
        )
    }
    pub(super) fn entry(
        &self,
        group: [u8; 32],
        title: &str,
        profile: crate::GroupProfile,
        owner: Option<Member>,
        now: u64,
    ) -> Result<Entry, EnrollmentError> {
        let identity =
            LocalIdentity::generate(random()?, 1).map_err(|_| EnrollmentError::Entropy)?;
        let (binding, secret) = identity.into_parts();
        let local = Member::new(self.local, binding);
        Ok(Entry {
            group,
            title: title.to_owned(),
            profile,
            owner: owner.unwrap_or_else(|| local.clone()),
            local,
            secret: secret.as_ref().to_vec(),
            namespace: random()?,
            incarnation: random()?,
            created: now,
            generation: 0,
            invitation: None,
        })
    }
    pub(super) fn initialize(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
    ) -> Result<(), EnrollmentError> {
        let ns = StoreNamespace(entry.namespace);
        let inc = StoreIncarnation::new(entry.incarnation).map_err(|_| EnrollmentError::Corrupt)?;
        match owner.open(ns, inc, self.selection.namespace) {
            Ok(_) => Ok(()),
            Err(RedbError::MissingNamespace) => {
                owner.initialize(ns, inc, self.selection.namespace)?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
    pub(super) fn adapter<T>(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
        roster: Option<&[Member]>,
        action: impl FnOnce(&Adapter<RedbBackend<'_>>, Snapshot) -> Result<T, EnrollmentError>,
    ) -> Result<T, EnrollmentError> {
        self.owned_adapter(owner, entry, roster, |adapter, snapshot| {
            action(&adapter, snapshot)
        })
    }
    pub(super) fn owned_adapter<T>(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
        roster: Option<&[Member]>,
        action: impl FnOnce(Adapter<RedbBackend<'_>>, Snapshot) -> Result<T, EnrollmentError>,
    ) -> Result<T, EnrollmentError> {
        self.owned_adapter_class(owner, entry, roster, AdmissionClass::Ordinary, action)
    }
    pub(super) fn management_adapter<T>(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
        roster: Option<&[Member]>,
        action: impl FnOnce(&Adapter<RedbBackend<'_>>, Snapshot) -> Result<T, EnrollmentError>,
    ) -> Result<T, EnrollmentError> {
        self.owned_adapter_class(
            owner,
            entry,
            roster,
            AdmissionClass::Management,
            |adapter, snapshot| action(&adapter, snapshot),
        )
    }
    fn owned_adapter_class<T>(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
        roster: Option<&[Member]>,
        class: AdmissionClass,
        action: impl FnOnce(Adapter<RedbBackend<'_>>, Snapshot) -> Result<T, EnrollmentError>,
    ) -> Result<T, EnrollmentError> {
        let l = self.selection.namespace;
        let mls_limits = entry
            .profile
            .mls_limits()
            .map_err(|_| EnrollmentError::Corrupt)?;
        let mut backend = owner.open(
            StoreNamespace(entry.namespace),
            StoreIncarnation::new(entry.incarnation).map_err(|_| EnrollmentError::Corrupt)?,
            l,
        )?;
        let snapshot = state::load_backend(&mut backend, l)?;
        let members = roster.map_or_else(
            || {
                snapshot
                    .current
                    .as_ref()
                    .map_or(if entry.owner == entry.local { 1 } else { 2 }, |s| {
                        s.manifest.members.len()
                    })
            },
            <[Member]>::len,
        );
        // A fresh owner Add can grow by one member. Fund that extra liability
        // before its commit; a larger declared profile does not reserve slots.
        let reserved_members = members
            .checked_add(usize::from(entry.owner == entry.local))
            .ok_or(EnrollmentError::Capacity)?;
        let backend = match class {
            AdmissionClass::Ordinary => backend.reserve_capacity(
                profile::managed_cleanup_reserve(l, entry.profile, reserved_members)?,
            )?,
            AdmissionClass::Management => backend,
        };
        let backend = backend.with_participant(std::sync::Arc::new(attempt::Participant {
            local: self.local,
            attempt: self.attempt.clone(),
            departure_ack: self.departure_ack,
            catalog: std::sync::Arc::new(self.read()?),
            limits: l,
        }));
        let coordinator = Coordinator::new(backend, l.transaction_limits()?)
            .map_err(|_| EnrollmentError::Corrupt)?;
        let bindings = if let Some(roster) = roster {
            roster.iter().map(Member::binding).collect()
        } else if let Some(signed) = &snapshot.current {
            signed.verify(entry.group, &entry.owner, &entry.title, entry.profile)?;
            signed
                .manifest
                .members
                .iter()
                .map(Member::binding)
                .collect()
        } else if entry.owner == entry.local {
            vec![entry.local.binding()]
        } else {
            vec![entry.owner.binding(), entry.local.binding()]
        };
        let policy = Policy::owner_managed(entry.owner.binding(), bindings)
            .map_err(|_| EnrollmentError::Authentication)?;
        let adapter = Adapter::new(coordinator, entry.identity()?, policy, mls_limits)
            .map_err(|_| EnrollmentError::Crypto)?;
        action(adapter, snapshot)
    }
    /// The fresh raw protocol phase never finalizes receipts. Its Recovery
    /// prefix owns cleanup, and the ordinary backend reserves more receipt slots
    /// than the historical two-slot check below. This allocation is read-only.
    pub(super) fn next_protocol(
        &self,
        adapter: &Adapter<RedbBackend<'_>>,
        entry: &Entry,
    ) -> Result<OperationId, EnrollmentError> {
        if self.attempt.is_none() {
            return Self::next(adapter, entry, self.selection.namespace);
        }
        let pending = adapter
            .pending_operations()
            .map_err(|_| EnrollmentError::Corrupt)?;
        if !Self::ordinary_outputs_available(
            adapter,
            entry,
            pending.len(),
            self.selection.namespace,
        )? {
            return Err(EnrollmentError::Capacity);
        }
        Self::next_maintenance(adapter, entry)
    }
    pub(super) fn next(
        adapter: &Adapter<RedbBackend<'_>>,
        entry: &Entry,
        limits: prns_groups_store_redb::RedbLimits,
    ) -> Result<OperationId, EnrollmentError> {
        // Leave two backend receipt slots for bounded cleanup while an older
        // service output still pins the contiguous retirement prefix.
        let pending = adapter
            .pending_operations()
            .map_err(|_| EnrollmentError::Corrupt)?;
        if !Self::ordinary_outputs_available(adapter, entry, pending.len(), limits)? {
            return Err(EnrollmentError::Capacity);
        }
        let (receipts, _) = Self::finalize_prefix(adapter, entry, &pending)?;
        if !Self::ordinary_receipt_available(
            receipts,
            usize::try_from(limits.max_receipts).map_err(|_| EnrollmentError::Capacity)?,
        ) {
            return Err(EnrollmentError::Capacity);
        }
        Self::next_maintenance(adapter, entry)
    }
    fn ordinary_outputs_available(
        adapter: &Adapter<RedbBackend<'_>>,
        entry: &Entry,
        pending: usize,
        limits: prns_groups_store_redb::RedbLimits,
    ) -> Result<bool, EnrollmentError> {
        let snapshot = state::load(adapter, limits)?;
        let members = snapshot
            .current
            .as_ref()
            .map_or(if entry.owner == entry.local { 1 } else { 2 }, |s| {
                s.manifest.members.len()
            });
        let future_members = members
            .checked_add(usize::from(entry.owner == entry.local))
            .ok_or(EnrollmentError::Capacity)?;
        let reserve = profile::management_output_reserve(future_members)?;
        // The fresh operation itself also occupies one exact output slot; its
        // Commit/Welcome messages belong to that same retained output.
        Ok(pending
            .checked_add(1)
            .and_then(|n| n.checked_add(reserve))
            .is_some_and(|n| {
                n <= entry
                    .profile
                    .mls_limits()
                    .map_or(0, |l| l.max_pending_outputs)
            }))
    }
    pub(super) fn ordinary_receipt_available(receipts: usize, max_receipts: usize) -> bool {
        receipts < max_receipts.saturating_sub(2)
    }
    pub(super) fn next_maintenance(
        adapter: &Adapter<RedbBackend<'_>>,
        entry: &Entry,
    ) -> Result<OperationId, EnrollmentError> {
        let head = adapter
            .coordinator()
            .head()
            .map_err(|_| EnrollmentError::Corrupt)?;
        Ok(OperationTicket::new(
            StoreIncarnation::new(entry.incarnation).map_err(|_| EnrollmentError::Corrupt)?,
            head.0.checked_add(1).ok_or(EnrollmentError::Capacity)?,
        )
        .map_err(|_| EnrollmentError::Corrupt)?
        .operation_id())
    }
    pub(super) fn stage(
        tx: &prns_groups_mls::ApplicationTransaction<'_, RedbBackend<'_>>,
        snapshot: &Snapshot,
    ) -> Result<(), EnrollmentError> {
        snapshot.stage(tx)
    }
    /// Resume only the selected, already admitted group. The fair catalog
    /// maintenance cursor chooses which namespace receives this bounded step.
    pub(super) fn recover_group(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
    ) -> Result<(), EnrollmentError> {
        if entry.owner.rns == self.local.into_bytes() {
            self.ensure_created(owner, entry)
        } else {
            self.ensure_package(owner, entry)
        }
    }

    pub(super) fn ensure_package(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
    ) -> Result<(), EnrollmentError> {
        let Some(id) = entry.invitation else {
            return Ok(());
        };
        let read = self.read()?;
        let Some(invite) = read.invitation(id)? else {
            return Ok(());
        };
        if invite.state != InvitationState::JoinRequested
            || read
                .terminal_subject(entry.group, &TerminalKind::Closed)?
                .is_some()
        {
            return Ok(());
        }
        self.initialize(owner, entry)?;
        self.adapter(owner, entry, None, |adapter, mut snapshot| {
            if snapshot.package.is_some()
                || snapshot.current.is_some()
                || snapshot.lifecycle != GroupLifecycle::Active
            {
                return Ok(());
            }
            adapter
                .compose(
                    Self::next(adapter, entry, snapshot.limits())?,
                    Command::KeyPackage {
                        now_secs: entry.created,
                    },
                    |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                    |tx, output| {
                        snapshot.package = Some(
                            output
                                .messages
                                .first()
                                .ok_or(EnrollmentError::Corrupt)?
                                .clone(),
                        );
                        Self::stage(tx, &snapshot)
                    },
                )
                .map_err(composition_error)?;
            Ok(())
        })
    }
}
impl EnrollmentError {
    pub fn is_invalid_input(&self) -> bool {
        matches!(
            self,
            Self::Invalid
                | Self::Authentication
                | Self::Expired
                | Self::Missing
                | Self::Conflict
                | Self::RequestConflict
                | Self::NotOwner
                | Self::NotJoined
                | Self::AlreadyJoined
                | Self::GroupClosed
                | Self::OwnerMustClose
                | Self::MemberNotFound
                | Self::StaleMembership
                | Self::UnsupportedVersion
                | Self::UnsupportedProfile
                | Self::Retry(
                    prns_groups_core::retry::RetryError::InvalidToken
                        | prns_groups_core::retry::RetryError::Expired
                )
        )
    }
}

#[cfg(test)]
mod validation_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn validator_uses_the_callers_snapshot_and_rejects_extra_catalog_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(dir.path().join("catalog")).unwrap();
        let id = IdentityHash::new([31; 16]);
        let before = db.begin_read().unwrap();
        assert!(matches!(
            Enrollment::validate_in(&before, id),
            Err(EnrollmentError::MissingCatalog)
        ));
        let tx = db.begin_write().unwrap();
        Enrollment::initialize_in(&tx, id).unwrap();
        tx.commit().unwrap();
        assert!(matches!(
            Enrollment::validate_in(&before, id),
            Err(EnrollmentError::MissingCatalog)
        ));
        drop(before);
        assert!(Enrollment::validate_in(&db.begin_read().unwrap(), id).is_ok());
        assert!(matches!(
            Enrollment::validate_in(&db.begin_read().unwrap(), IdentityHash::new([32; 16])),
            Err(EnrollmentError::Authentication)
        ));
        let tx = db.begin_write().unwrap();
        tx.open_table(CATALOG)
            .unwrap()
            .insert(1, b"extra".as_slice())
            .unwrap();
        tx.commit().unwrap();
        assert!(matches!(
            Enrollment::validate_in(&db.begin_read().unwrap(), id),
            Err(EnrollmentError::Corrupt)
        ));
        assert!(matches!(
            Enrollment::open_existing(&db, id),
            Err(EnrollmentError::Corrupt)
        ));
    }
}
