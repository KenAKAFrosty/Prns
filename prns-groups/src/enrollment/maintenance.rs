use super::*;
use prns_groups_core::retry::RetryPolicy;

pub const RETRY_HORIZON_SECS: u64 = 14 * 24 * 60 * 60;
pub const MAX_UNACCEPTED_INCOMING: usize = 4;
pub const MAX_INCOMING_PER_PEER: usize = 2;
pub(super) const RETRY_POLICY: RetryPolicy = RetryPolicy {
    horizon_secs: RETRY_HORIZON_SECS,
    future_skew_secs: 300,
};

/// Bounded catalog cleanup. These are logical record counts, not disk reclamation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Maintenance {
    pub retired_requests: usize,
    pub retired_invitations: usize,
    pub retired_outputs: usize,
    pub request_retired_before: u64,
}

impl<'db> Enrollment<'db> {
    /// Atomically retire API receipts and safe terminal invitation records with
    /// their replay fences. Group namespaces and MLS Welcome/control bytes stay.
    pub fn maintain(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        now: u64,
    ) -> Result<Maintenance, EnrollmentError> {
        let read = self.read()?;
        let plan = read.maintenance(now)?;
        let mut result = plan.result;
        // Row/index/counter deletion and its replay floor share one commit.
        // Cursor progress is retained before independent namespace cleanup;
        // an interrupted output cleanup is retried on the next bounded wrap.
        self.write(plan.write)?;
        self.purge_retired(owner)?;
        if let Some(entry) = plan.output {
            self.initialize(owner, &entry)?;
            self.recover_group(owner, &entry)?;
            result.retired_outputs = self.retire_outputs(owner, &entry)?;
        }
        Ok(result)
    }

    // Native managed namespaces are consumed only by enrollment and the group
    // service. The service identifies its retained operation facts; enrollment
    // publishes each replay obligation in Snapshot in the same MLS
    // transaction. Verify that proof as well as the operation's event before
    // deleting its redundant output. Backend operation receipts remain intact.
    fn retire_outputs(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
    ) -> Result<usize, EnrollmentError> {
        let max_receipts = usize::try_from(self.selection.namespace.max_receipts)
            .map_err(|_| EnrollmentError::Capacity)?;
        self.management_adapter(owner, entry, None, |adapter, snapshot| {
            let mut completed = Vec::new();
            let pending = adapter
                .pending_operations()
                .map_err(|_| EnrollmentError::Crypto)?;
            let (receipts, oldest) = Self::finalize_prefix(adapter, entry, &pending)?;
            for operation in pending.iter().copied() {
                if prns_groups_service::has_operation_fact(adapter, operation)
                    .map_err(|_| EnrollmentError::Crypto)?
                {
                    continue;
                }
                let output = adapter
                    .retry(operation)
                    .map_err(|_| EnrollmentError::Crypto)?
                    .ok_or(EnrollmentError::Corrupt)?;
                if Self::output_replay_is_preserved(adapter, entry, &snapshot, &output)? {
                    completed.push(operation);
                    if completed.len() == 16 {
                        break;
                    }
                }
            }
            if !completed.is_empty() {
                if !Self::cleanup_receipt_available(
                    receipts,
                    max_receipts,
                    oldest.is_some_and(|operation| completed.contains(&operation)),
                ) {
                    return Ok(0);
                }
                adapter
                    .maintain(Self::next_maintenance(adapter, entry)?, &completed, |_| {
                        Ok::<_, EnrollmentError>(())
                    })
                    .map_err(|_| EnrollmentError::Crypto)?;
                let remaining = adapter
                    .pending_operations()
                    .map_err(|_| EnrollmentError::Crypto)?;
                Self::finalize_prefix(adapter, entry, &remaining)?;
            }
            Ok(completed.len())
        })
    }

    pub(super) fn cleanup_receipt_available(
        receipts: usize,
        max_receipts: usize,
        retires_oldest: bool,
    ) -> bool {
        receipts < max_receipts && (receipts < max_receipts.saturating_sub(1) || retires_oldest)
    }

    // Only the native owner may attest this prefix: all enrollment and service
    // recovery obligations in these namespaces retain an MLS output until their
    // own durable proof permits deletion. Finalization itself consumes no slot.
    pub(super) fn finalize_prefix(
        adapter: &Adapter<RedbBackend<'_>>,
        entry: &Entry,
        pending: &[OperationId],
    ) -> Result<(usize, Option<OperationId>), EnrollmentError> {
        let head = adapter
            .coordinator()
            .head()
            .map_err(|_| EnrollmentError::Corrupt)?
            .0;
        let incarnation =
            StoreIncarnation::new(entry.incarnation).map_err(|_| EnrollmentError::Corrupt)?;
        let mut oldest: Option<(u64, OperationId)> = None;
        for operation in pending {
            let ticket = OperationTicket::from_operation_id(*operation)
                .map_err(|_| EnrollmentError::Corrupt)?;
            if ticket.incarnation() != incarnation || ticket.sequence() > head {
                return Err(EnrollmentError::Corrupt);
            }
            if oldest.is_none_or(|(sequence, _)| ticket.sequence() < sequence) {
                oldest = Some((ticket.sequence(), *operation));
            }
        }
        let through = oldest.map_or(head, |(sequence, _)| sequence - 1);
        if through != 0 {
            let ticket =
                OperationTicket::new(incarnation, through).map_err(|_| EnrollmentError::Corrupt)?;
            match adapter
                .coordinator()
                .maintain(prns_groups_store_redb::RedbMaintenanceRequest::FinalizeThrough(ticket))
                .map_err(|_| EnrollmentError::Corrupt)?
            {
                prns_groups_store::MaintenanceOutcome::Complete(_) => {}
                _ => return Err(EnrollmentError::Corrupt),
            }
        }
        Ok((
            usize::try_from(head - through).map_err(|_| EnrollmentError::Capacity)?,
            oldest.map(|(_, operation)| operation),
        ))
    }

    pub(super) fn output_replay_is_preserved(
        adapter: &Adapter<RedbBackend<'_>>,
        entry: &Entry,
        snapshot: &Snapshot,
        output: &prns_groups_mls::Output,
    ) -> Result<bool, EnrollmentError> {
        use prns_groups_mls::Event;
        if output.event == Event::KeyPackage {
            return Ok(output.messages.len() == 1
                && (snapshot.package.as_ref() == output.messages.first()
                    || snapshot.current.is_some()));
        }
        let (Some(current), Some(context)) = (&snapshot.current, output.context) else {
            return Ok(false);
        };
        if context.group.0 != entry.group || context.epoch > current.manifest.epoch {
            return Err(EnrollmentError::Corrupt);
        }
        match output.event {
            Event::Created | Event::Joined => Ok(output.messages.is_empty()),
            Event::Retired => {
                Ok(snapshot.lifecycle != GroupLifecycle::Active && output.messages.is_empty())
            }
            Event::Control => {
                let Some(control) = snapshot.control(adapter, context.epoch)? else {
                    return Ok(false);
                };
                control
                    .signed
                    .verify(entry.group, &entry.owner, &entry.title, entry.profile)?;
                match output.messages.as_slice() {
                    [] => Ok(true),
                    [commit] => Ok(commit == &control.commit),
                    [commit, welcome] => Ok(commit == &control.commit
                        && snapshot
                            .added_epoch(adapter, context.epoch)?
                            .is_some_and(|added| {
                                added.signed == control.signed && &added.welcome == welcome
                            })),
                    _ => Ok(false),
                }
            }
            // Never infer ownership of service output, including a missing or
            // damaged application fact, merely from the absence of that fact.
            _ => Ok(false),
        }
    }

    /// Exact receipts from both catalog and namespace commits share one index.
    /// Absence never falls back to scanning namespaces.
    pub(super) fn retained_binding(
        &self,
        _owner: &mut RedbStoreOwner<'db>,
        id: [u8; 32],
        fingerprint: [u8; 32],
    ) -> Result<(Catalog, Option<[u8; 32]>), EnrollmentError> {
        let catalog = self.read()?;
        let result = Self::replay(&catalog, id, fingerprint)?;
        Ok((catalog, result))
    }

    pub(super) fn admission_class(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        request: &crate::Request,
        fingerprint: [u8; 32],
        context: crate::ExecutionContext,
        _class: AdmissionClass,
        allow_new: bool,
    ) -> Result<Option<Admitted>, EnrollmentError> {
        let id = request.id.0;
        let (catalog, replay) = self.retained_binding(owner, id, fingerprint)?;
        if let Some(result) = replay {
            return Ok(Some(Admitted {
                request: id,
                fingerprint,
                catalog,
                replay: Some(result),
                clock: None,
            }));
        }
        if !allow_new {
            return Ok(None);
        }
        if let Some(group) = request.intent.group() {
            let expected = request.original_scope.ok_or(EnrollmentError::Invalid)?;
            let entry = Self::find(&catalog, group.0)?;
            if self.entry_scope(owner, &entry)? != expected {
                return Err(EnrollmentError::StaleMembership);
            }
        }
        let prns_groups_core::ClockReading::Trusted(interval) = context.clock else {
            return Err(EnrollmentError::Clock);
        };
        let now = interval.lower().0;
        if now < 3600 {
            return Err(EnrollmentError::Clock);
        }
        self.maintain(owner, now)?;
        let catalog = self.read()?;
        RETRY_POLICY
            .validate_new(
                id,
                now.max(catalog.header.invitation_time_floor),
                catalog.header.request_retired_before,
            )
            .map_err(EnrollmentError::Retry)?;
        // The exact planned rows, indexes and funded obligations are charged
        // before committing by Edit. There is no count-derived frame reserve.
        Ok(Some(Admitted {
            request: id,
            fingerprint,
            catalog,
            replay: None,
            clock: Some(interval),
        }))
    }

    pub(super) fn retained_add(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        catalog: &Catalog,
        invitation: [u8; 32],
        requester: IdentityHash,
    ) -> Result<Option<(Entry, Snapshot, Added)>, EnrollmentError> {
        let Some(binding) = catalog.invitation_binding(invitation)? else {
            return Ok(None);
        };
        if binding.member.rns != requester.into_bytes() {
            return Err(EnrollmentError::Authentication);
        }
        let row = catalog
            .incarnation(indexed::keys::incarnation(
                binding.namespace.group,
                binding.namespace.generation,
            ))?
            .ok_or(EnrollmentError::Corrupt)?;
        let entry = match row {
            indexed::rows::Incarnation::Live(entry)
            | indexed::rows::Incarnation::Archived(entry) => entry,
            indexed::rows::Incarnation::Retired(_) => return Ok(None),
        };
        if entry.owner.rns != self.local.into_bytes() {
            return Ok(None);
        }
        self.adapter(owner, &entry, None, |adapter, snapshot| {
            let added = snapshot
                .added(adapter, invitation)?
                .ok_or(EnrollmentError::Corrupt)?;
            if added.member != binding.member {
                return Err(EnrollmentError::Corrupt);
            }
            Ok(Some((entry.clone(), snapshot, added)))
        })
    }
}
