//! Directed, authenticated owner-managed enrollment. All group crypto work and
//! its public control facts commit together on the caller's sole native store lane.
//! The caller authenticates `handle`'s requester using an identified Reticulum
//! Link and authenticates response peers against each Work's recipient.
pub(crate) mod attempt;
mod carrier;
mod dependencies;
mod indexed;
mod inspection;
mod mutations;
#[cfg(test)]
mod request_helpers;
use mutations::Admitted;
mod history;
mod maintenance;
mod management;
mod model;
mod state;
pub use maintenance::{
    Maintenance, MAX_INCOMING_PER_PEER, MAX_UNACCEPTED_INCOMING, RETRY_HORIZON_SECS,
};
use state::Snapshot;
mod protocol;
mod storage;
mod work;
pub use indexed::policy::CatalogLimits;
use indexed::{Edit, Read as Catalog};

/// Scheduling evidence from a correlated, validated completion. This reports
/// committed control work, not packet arrival or an untrusted reply tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionActivity {
    Idle,
    Progress,
}

use crate::{profile, GroupLifecycle, Provisioned};
use model::*;
pub(crate) use model::{required_frame_bytes, required_frame_bytes_for_members};
pub use model::{
    Invitation, InvitationState, Work, WorkPage, INVITATION_LIFETIME_SECS, MAX_FRAME_BYTES,
};
use prns_groups_core::{
    Endpoint, EndpointId, EndpointIncarnation, GroupId, OperationId, Timestamp,
};
use prns_groups_mls::{Adapter, Admission, Command, LocalIdentity, MemberBinding, Policy};
use prns_groups_store::{Coordinator, OperationTicket, StoreIncarnation};
use prns_groups_store_redb::{RedbBackend, RedbError, RedbStoreOwner, StoreNamespace};
use prns_host::IdentityHash;
use redb::{
    Database, ReadableDatabase, ReadableTable, TableDefinition, TableHandle, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub(crate) use state::{check_provisioned, historical_author};
use zeroize::Zeroizing;

#[derive(Debug)]
pub enum EnrollmentError {
    /// No managed catalog exists. Opening never initializes one implicitly.
    MissingCatalog,
    /// Even an empty existing catalog table requires inspection, not replacement.
    AlreadyInitialized,
    Invalid,
    Clock,
    Retry(prns_groups_core::retry::RetryError),
    Authentication,
    Expired,
    Capacity,
    Missing,
    Conflict,
    /// The request identifier already belongs to different immutable input.
    RequestConflict,
    NotOwner,
    NotJoined,
    AlreadyJoined,
    GroupClosed,
    OwnerMustClose,
    MemberNotFound,
    StaleMembership,
    UnsupportedVersion,
    UnsupportedProfile,
    Corrupt,
    Entropy,
    Storage(RedbError),
    Database(redb::Error),
    Crypto,
}
impl std::fmt::Display for EnrollmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "group enrollment: {self:?}")
    }
}
impl std::error::Error for EnrollmentError {}
impl From<RedbError> for EnrollmentError {
    fn from(e: RedbError) -> Self {
        Self::Storage(e)
    }
}
fn db_error(e: impl Into<redb::Error>) -> EnrollmentError {
    EnrollmentError::Database(e.into())
}

#[derive(Clone, Copy)]
enum AdmissionClass {
    Ordinary,
    Management,
}

fn composition_error(
    error: prns_groups_mls::CompositionError<RedbError, EnrollmentError>,
) -> EnrollmentError {
    use prns_groups_mls::{CompositionError, Error as MlsError, ProviderError};
    use prns_groups_store::{OperationRejection, StoreError};
    match error {
        CompositionError::Application(error) => error,
        CompositionError::Mls(
            MlsError::Capacity(_)
            | MlsError::Provider(ProviderError::Capacity(_))
            | MlsError::Store(
                StoreError::Capacity(_)
                | StoreError::Backend(RedbError::Capacity)
                | StoreError::OperationRejected(
                    OperationRejection::RecoveryWindowFull
                    | OperationRejection::ReservedCapacity
                    | OperationRejection::StorageCapacity,
                ),
            ),
        ) => EnrollmentError::Capacity,
        _ => EnrollmentError::Crypto,
    }
}

pub struct Enrollment<'db> {
    database: &'db Database,
    local: IdentityHash,
    change_witness: Option<prns_groups_store_redb::ChangeWitness>,
    attempt: Option<attempt::Attempt>,
    departure_ack: Option<[u8; 32]>,
    profile_support: Option<crate::ProfileSupport>,
    change_stamp: Option<StoreIncarnation>,
    selection: CatalogLimits,
}
impl<'db> Enrollment<'db> {
    fn with_attempt(&self, attempt: attempt::Attempt) -> Self {
        Self {
            database: self.database,
            local: self.local,
            change_witness: self.change_witness.clone(),
            attempt: Some(attempt),
            departure_ack: self.departure_ack,
            profile_support: self.profile_support,
            change_stamp: self.change_stamp,
            selection: self.selection,
        }
    }
    fn with_departure_ack(&self, departure_ack: Option<[u8; 32]>) -> Self {
        Self {
            database: self.database,
            local: self.local,
            change_witness: self.change_witness.clone(),
            attempt: self.attempt.clone(),
            departure_ack,
            profile_support: self.profile_support,
            change_stamp: self.change_stamp,
            selection: self.selection,
        }
    }
    pub(crate) fn with_change_witness(
        mut self,
        witness: prns_groups_store_redb::ChangeWitness,
    ) -> Self {
        self.change_witness = Some(witness);
        self
    }

    pub(crate) fn with_profile_support(mut self, support: Option<crate::ProfileSupport>) -> Self {
        self.profile_support = support;
        self
    }

    pub(crate) fn with_change_stamp(mut self, incarnation: StoreIncarnation) -> Self {
        self.change_stamp = Some(incarnation);
        self
    }

    /// Validate first, then install a local owner-lifetime capability. Failed
    /// or wider runner attempts cannot partially replace an existing bound.
    pub(crate) fn set_profile_support(
        &mut self,
        support: crate::ProfileSupport,
    ) -> Result<crate::ProfileSupport, EnrollmentError> {
        let support = self
            .profile_support
            .map_or(support, |prior| prior.intersect(support));
        let catalog = self.read()?;
        for row in catalog.incarnations.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let key = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            let row = catalog.incarnation(key)?.ok_or(EnrollmentError::Corrupt)?;
            support.require(row.profile())?;
        }
        for row in catalog.invitations.iter().map_err(db_error)? {
            let (key, _) = row.map_err(db_error)?;
            if key.value().is_empty() {
                continue;
            }
            let id = key
                .value()
                .try_into()
                .map_err(|_| EnrollmentError::Corrupt)?;
            support.require(
                catalog
                    .invitation(id)?
                    .ok_or(EnrollmentError::Corrupt)?
                    .offer
                    .profile,
            )?;
        }
        self.profile_support = Some(support);
        Ok(support)
    }

    /// Validate this selected catalog using the caller's existing read snapshot.
    /// Complete absence remains MissingCatalog; this never initializes state.
    pub fn validate_in(
        transaction: &redb::ReadTransaction,
        local: IdentityHash,
    ) -> Result<(), EnrollmentError> {
        Self::inspect_in(transaction, local).map(|_| ())
    }
    pub(crate) fn inspect_in(
        transaction: &redb::ReadTransaction,
        local: IdentityHash,
    ) -> Result<CatalogLimits, EnrollmentError> {
        let read = Self::read_in(transaction, local)?;
        read.inspect()?;
        Ok(read.header.limits)
    }

    /// Initialize a new managed catalog and durably commit it. Existing tables,
    /// including empty or partial ones, are never overwritten.
    pub fn create_new(
        database: &'db Database,
        local: IdentityHash,
    ) -> Result<Self, EnrollmentError> {
        let mut transaction = database.begin_write().map_err(db_error)?;
        transaction
            .set_durability(redb::Durability::Immediate)
            .map_err(db_error)?;
        Self::initialize_in(&transaction, local)?;
        prns_groups_store_redb::change::commit(transaction, None)
            .map_err(EnrollmentError::Storage)?;
        Self::open_existing(database, local)
    }

    /// Initialize only this domain's top-level schema in a caller-owned write
    /// transaction. The physical owner can atomically bind the identity and other
    /// selected schemas in that same transaction. It must commit with Immediate
    /// durability before announcing readiness. No nested transaction is opened.
    pub fn initialize_in(
        transaction: &WriteTransaction,
        local: IdentityHash,
    ) -> Result<(), EnrollmentError> {
        Self::initialize_with_limits_in(transaction, local, CatalogLimits::default())
    }

    /// Select bounded local capacity only while atomically initializing a fresh
    /// catalog. Existing stores retain their persisted selection on every open.
    pub fn initialize_with_limits_in(
        transaction: &WriteTransaction,
        local: IdentityHash,
        limits: CatalogLimits,
    ) -> Result<(), EnrollmentError> {
        indexed::initialize_in(transaction, local, limits)
    }

    /// Validate a configured catalog and its immutable identity without writes.
    /// Missing, corrupt, mismatched or unsupported state remains untouched.
    /// Pending group-namespace transitions retain their own recovery protocol.
    pub fn open_existing(
        database: &'db Database,
        local: IdentityHash,
    ) -> Result<Self, EnrollmentError> {
        let read = Self::read_in(&database.begin_read().map_err(db_error)?, local)?;
        read.inspect()?;
        Self::inspect_namespaces(database, local, read.header.limits)?;
        Self::open_attached(database, local, read.header.limits)
    }

    /// Reconstruct a lexical view of a catalog already strictly validated by
    /// the retained attachment. Selection and schema markers are still checked.
    pub(crate) fn open_attached(
        database: &'db Database,
        local: IdentityHash,
        selection: CatalogLimits,
    ) -> Result<Self, EnrollmentError> {
        let read = Self::read_in(&database.begin_read().map_err(db_error)?, local)?;
        if read.header.limits != selection {
            return Err(EnrollmentError::Conflict);
        }
        Ok(Self {
            database,
            local,
            change_witness: None,
            attempt: None,
            departure_ack: None,
            profile_support: None,
            change_stamp: None,
            selection,
        })
    }
    /// The authoritative persisted selection; reading it never reconfigures data.
    pub fn limits(&self) -> Result<CatalogLimits, EnrollmentError> {
        Ok(self.selection)
    }
    fn frame<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, EnrollmentError> {
        frame_with_limit(
            value,
            usize::try_from(self.limits()?.max_enrollment_frame_bytes)
                .map_err(|_| EnrollmentError::Corrupt)?,
        )
    }
    fn decode<T: serde::de::DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, EnrollmentError> {
        decode_with_limit(
            bytes,
            usize::try_from(self.limits()?.max_enrollment_frame_bytes)
                .map_err(|_| EnrollmentError::Corrupt)?,
        )
    }
    fn require_profile(&self, selected: crate::GroupProfile) -> Result<(), EnrollmentError> {
        selected
            .validate()
            .map_err(|_| EnrollmentError::UnsupportedProfile)?;
        if let Some(support) = self.profile_support {
            support.require(selected)?;
        }
        let local = self.limits()?;
        if selected.max_enrollment_frame_bytes > local.max_enrollment_frame_bytes
            || u64::from(selected.max_snapshot_bytes) > local.namespace.max_value_bytes
            || u64::try_from(required_frame_bytes(selected)?)
                .map_err(|_| EnrollmentError::UnsupportedProfile)?
                > local.namespace.max_value_bytes
        {
            return Err(EnrollmentError::UnsupportedProfile);
        }
        Ok(())
    }
    fn create_admitted(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        title: &str,
        profile: crate::GroupProfile,
    ) -> Result<GroupId, EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            clock,
        } = admitted;
        let group = if let Some(group) = replay {
            group
        } else {
            self.require_profile(profile)?;
            let now = clock.ok_or(EnrollmentError::Clock)?.lower().0;
            let group = random()?;
            let entry = self.entry(group, title, profile, None, now)?;
            let edit = Edit::new(&catalog)?
                .put_current(&catalog, group, &indexed::rows::Current::pending(&entry))?
                .put_incarnation(
                    &catalog,
                    indexed::keys::incarnation(group, entry.generation),
                    &indexed::rows::Incarnation::Live(entry),
                )?;
            self.write(Self::remember(&catalog, edit, request, fingerprint, group)?.finish()?)?;
            group
        };
        // Exact replay after local Delete preserves the outcome without
        // recreating a namespace. Otherwise resume only this admitted Create.
        if let Some(entry) = self.read()?.entry(group)? {
            self.ensure_created(owner, &entry)?;
        }
        Ok(GroupId(group))
    }
    fn ensure_created(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
    ) -> Result<(), EnrollmentError> {
        self.initialize(owner, entry)?;
        self.adapter(owner, entry, None, |adapter, mut snapshot| {
            if snapshot.current.is_some() {
                return Ok(());
            }
            let expected = [entry.local.binding()];
            let identity = entry.identity()?;
            adapter
                .compose_with_roster(
                    Self::next(adapter, entry, snapshot.limits())?,
                    Command::Create {
                        group: GroupId(entry.group),
                    },
                    Some(&expected),
                    |_, _, _| Ok::<_, EnrollmentError>(Admission::Reject(1)),
                    |tx, output| {
                        let manifest = Manifest {
                            version: WIRE_VERSION,
                            group: entry.group,
                            owner: self.local.into_bytes(),
                            title: entry.title.clone(),
                            profile: entry.profile,
                            parent: [0; 32],
                            revision: 0,
                            epoch: output.context.ok_or(EnrollmentError::Corrupt)?.epoch,
                            commit_digest: [0; 32],
                            kind: ControlKind::Create,
                            members: vec![entry.local.clone()],
                        };
                        let signed = Signed {
                            signature: identity
                                .sign_enrollment(&encode(&manifest)?)
                                .map_err(|_| EnrollmentError::Crypto)?,
                            manifest,
                        };
                        snapshot.current = Some(signed);
                        Self::stage(tx, &snapshot)
                    },
                )
                .map_err(composition_error)?;
            Ok(())
        })?;
        Ok(())
    }

    fn invite_admitted(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        group: GroupId,
        recipient: IdentityHash,
    ) -> Result<[u8; 32], EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            clock,
        } = admitted;
        if let Some(id) = replay {
            return Ok(id);
        }
        let now = clock
            .ok_or(EnrollmentError::Clock)?
            .lower()
            .0
            .max(catalog.header.invitation_time_floor);
        let entry = Self::find(&catalog, group.0)?;
        if entry.owner.rns != self.local.into_bytes() {
            return Err(EnrollmentError::NotOwner);
        }
        if catalog
            .pending_recipient(group.0, recipient.into_bytes())?
            .is_some()
        {
            return Err(EnrollmentError::Conflict);
        }
        let current = catalog.current(group.0)?.ok_or(EnrollmentError::Corrupt)?;
        self.adapter(owner, &entry, None, |_, snapshot| {
            Self::require_active(&snapshot)?;
            let manifest = &snapshot
                .current
                .as_ref()
                .ok_or(EnrollmentError::NotJoined)?
                .manifest;
            if manifest
                .members
                .iter()
                .any(|member| member.rns == recipient.into_bytes())
            {
                return Err(EnrollmentError::Conflict);
            }
            let members =
                u64::try_from(manifest.members.len()).map_err(|_| EnrollmentError::Capacity)?;
            if members
                .checked_add(u64::from(current.pending_offers))
                .is_none_or(|n| n >= u64::from(entry.profile.max_members))
            {
                return Err(EnrollmentError::Capacity);
            }
            Ok(())
        })?;
        let id = random()?;
        let invite = Invite {
            admission_revision: 0,
            offer: Offer {
                id,
                group: group.0,
                title: entry.title.clone(),
                profile: entry.profile,
                owner: entry.owner.clone(),
                recipient: recipient.into_bytes(),
                created: now,
                expires: now
                    .checked_add(INVITATION_LIFETIME_SECS)
                    .ok_or(EnrollmentError::Invalid)?,
            },
            state: InvitationState::Pending,
            acknowledged: false,
            terminal_at: None,
            deferred_join: None,
            dismissed: false,
        };
        let edit = Edit::new(&catalog)?.put_invitation(&catalog, &invite)?;
        self.write(Self::remember(&catalog, edit, request, fingerprint, id)?.finish()?)?;
        Ok(id)
    }
    /// Resolve an invitation's stable group without maintenance or other writes.
    /// Dismissed and locally deleted invitations retain this association.
    pub fn invitation_group(
        &self,
        invitation: [u8; 32],
    ) -> Result<Option<GroupId>, EnrollmentError> {
        let read = self.read()?;
        let mut result = read
            .invitation(invitation)?
            .map(|invite| invite.offer.group);
        for group in [
            read.invitation_binding(invitation)?
                .map(|binding| binding.namespace.group),
            read.cancellation(invitation)?
                .map(|cancellation| cancellation.group),
        ]
        .into_iter()
        .flatten()
        {
            if result.is_some_and(|prior| prior != group) {
                return Err(EnrollmentError::Corrupt);
            }
            result = Some(group);
        }
        Ok(result.map(GroupId))
    }
    fn respond_admitted(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        admitted: Admitted,
        id: [u8; 32],
        accept: bool,
    ) -> Result<(), EnrollmentError> {
        let Admitted {
            request,
            fingerprint,
            catalog,
            replay,
            clock,
        } = admitted;
        if replay.is_some() {
            if accept {
                if let Some(binding) = catalog.invitation_binding(id)? {
                    if let Some(entry) = catalog.entry(binding.namespace.group)? {
                        if entry.invitation == Some(id) {
                            self.ensure_package(owner, &entry)?;
                        }
                    }
                }
            }
            return Ok(());
        }
        let mut invite = catalog.invitation(id)?.ok_or(EnrollmentError::Missing)?;
        if invite.state == InvitationState::Expired {
            return Err(EnrollmentError::Expired);
        }
        if invite.offer.recipient != self.local.into_bytes()
            || invite.state != InvitationState::Pending
            || catalog.invitation_deleted(&invite)?
        {
            return Err(EnrollmentError::Conflict);
        }
        let interval = clock.ok_or(EnrollmentError::Clock)?;
        let now = interval.lower().0.max(catalog.header.invitation_time_floor);
        prns_groups_core::enrollment::validate_invitation_time(
            Timestamp(invite.offer.created),
            Timestamp(invite.offer.expires),
            Timestamp(now),
            Timestamp(interval.upper().0.max(catalog.header.invitation_time_floor)),
            INVITATION_LIFETIME_SECS,
        )
        .map_err(|_| EnrollmentError::Expired)?;
        if catalog
            .terminal_subject(invite.offer.group, &TerminalKind::Closed)?
            .is_some()
            || catalog.cancellation(id)?.is_some()
        {
            return Err(EnrollmentError::GroupClosed);
        }
        let mut edit = Edit::new(&catalog)?;
        let mut accepted = None;
        if accept {
            self.require_profile(invite.offer.profile)?;
            let prior = catalog.current(invite.offer.group)?;
            let mut entry = self.entry(
                invite.offer.group,
                &invite.offer.title,
                invite.offer.profile,
                Some(invite.offer.owner.clone()),
                now,
            )?;
            entry.invitation = Some(id);
            let purge_through = prior.as_ref().and_then(|row| row.purge_through);
            if let Some(current) = &prior {
                let old = catalog
                    .incarnation(indexed::keys::incarnation(entry.group, current.generation))?
                    .ok_or(EnrollmentError::Corrupt)?;
                if old.owner() != &entry.owner || old.profile() != entry.profile {
                    return Err(EnrollmentError::Authentication);
                }
                entry.generation = current
                    .generation
                    .checked_add(1)
                    .ok_or(EnrollmentError::Capacity)?;
                if let indexed::rows::Incarnation::Live(old) = old {
                    let expired_unjoined = if current.head.is_none() {
                        old.invitation
                            .map(|id| catalog.invitation(id))
                            .transpose()?
                            .flatten()
                            .is_some_and(|invite| {
                                matches!(
                                    invite.state,
                                    InvitationState::Cancelled | InvitationState::Expired
                                ) || now >= invite.offer.expires
                            })
                    } else {
                        false
                    };
                    if current.lifecycle != GroupLifecycle::Removed && !expired_unjoined {
                        return Err(EnrollmentError::Conflict);
                    }
                    let key = indexed::keys::incarnation(old.group, old.generation);
                    let row = if current.head.is_some() {
                        indexed::rows::Incarnation::Archived(old)
                    } else {
                        indexed::rows::Incarnation::Retired(
                            indexed::rows::RetiredIncarnation::from_entry(&old, false),
                        )
                    };
                    edit = edit.put_incarnation(&catalog, key, &row)?;
                }
            }
            let mut current = indexed::rows::Current::pending(&entry);
            current.purge_through = purge_through;
            current.invitations_deleted_through = prior
                .as_ref()
                .map_or(0, |row| row.invitations_deleted_through);
            edit = edit
                .put_current(&catalog, entry.group, &current)?
                .put_incarnation(
                    &catalog,
                    indexed::keys::incarnation(entry.group, entry.generation),
                    &indexed::rows::Incarnation::Live(entry.clone()),
                )?
                .bind_invitation(
                    &catalog,
                    id,
                    &indexed::rows::InvitationBinding {
                        namespace: indexed::rows::NamespaceReceipt {
                            group: entry.group,
                            generation: entry.generation,
                            incarnation: entry.incarnation,
                        },
                        member: entry.local.clone(),
                    },
                )?;
            accepted = Some(entry);
        }
        invite.state = if accept {
            InvitationState::JoinRequested
        } else {
            InvitationState::Declined
        };
        invite.acknowledged = false;
        edit = edit.put_invitation(&catalog, &invite)?;
        self.write(Self::remember(&catalog, edit, request, fingerprint, id)?.finish()?)?;
        if let Some(entry) = accepted {
            self.ensure_package(owner, &entry)?;
        }
        Ok(())
    }
    pub fn provisioned(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        group: GroupId,
    ) -> Result<Provisioned, EnrollmentError> {
        let catalog = self.read()?;
        let entry = Self::find(&catalog, group.0)?;
        self.provisioned_entry(owner, &entry, &catalog)
    }
    fn provisioned_entry(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        entry: &Entry,
        catalog: &Catalog,
    ) -> Result<Provisioned, EnrollmentError> {
        let group = GroupId(entry.group);
        self.adapter(owner, entry, None, |_adapter, snapshot| {
            if snapshot.lifecycle == GroupLifecycle::Active && !snapshot.ready()? {
                return Err(EnrollmentError::NotJoined);
            }
            let signed = snapshot
                .current
                .as_ref()
                .ok_or(EnrollmentError::NotJoined)?;
            signed.verify(entry.group, &entry.owner, &entry.title, entry.profile)?;
            let readers = signed
                .manifest
                .members
                .iter()
                .map(Member::endpoint)
                .collect::<Result<Vec<_>, _>>()?;
            let bindings = signed
                .manifest
                .members
                .iter()
                .map(Member::binding)
                .collect();
            let head = prns_groups_core::ControlId(signed.head()?);
            let peers = signed
                .manifest
                .members
                .iter()
                .filter(|m| m.rns != self.local.into_bytes())
                .map(|member| {
                    Ok(prns_groups_reticulum::Peer {
                        endpoint: member.endpoint()?,
                        identity: IdentityHash::new(member.rns),
                        destination: prns_groups_reticulum::destination_for(IdentityHash::new(
                            member.rns,
                        ))
                        .map_err(|_| EnrollmentError::Invalid)?,
                    })
                })
                .collect::<Result<Vec<_>, EnrollmentError>>()?;
            let lifecycle = if catalog.current(entry.group)?.is_some_and(|current| {
                current.generation == entry.generation && current.departure.is_some()
            }) && snapshot.lifecycle == GroupLifecycle::Active
            {
                GroupLifecycle::Leaving
            } else {
                snapshot.lifecycle
            };
            Ok(Provisioned {
                lifecycle,
                history_generation: entry.generation,
                // Old authors are resolved by exact namespace key only when read.
                history_peers: Vec::new(),
                owner_identity: Some(IdentityHash::new(entry.owner.rns)),
                cleanup_reserve: Some(profile::managed_cleanup_reserve(
                    snapshot.limits(),
                    entry.profile,
                    signed.manifest.members.len(),
                )?),
                group,
                control: prns_groups_service::ControlReference {
                    head,
                    era: prns_groups_core::AuthorizationEra(head),
                },
                readers,
                identity: entry.identity()?,
                policy: Policy::owner_managed(entry.owner.binding(), bindings)
                    .map_err(|_| EnrollmentError::Corrupt)?,
                namespace: StoreNamespace(entry.namespace),
                incarnation: StoreIncarnation::new(entry.incarnation)
                    .map_err(|_| EnrollmentError::Corrupt)?,
                store_limits: snapshot.limits(),
                mls_limits: entry
                    .profile
                    .mls_limits()
                    .map_err(|_| EnrollmentError::Corrupt)?,
                service_limits: entry
                    .profile
                    .service_limits()
                    .map_err(|_| EnrollmentError::Corrupt)?,
                title: entry.title.clone(),
                peers,
                local_reticulum_identity: self.local,
            })
        })
    }
}

#[cfg(test)]
mod tests;
