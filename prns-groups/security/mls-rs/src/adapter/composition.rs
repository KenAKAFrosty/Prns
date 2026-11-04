use super::*;
use core::{cell::Cell, convert::Infallible};
use prns_groups_core::{checked_charge, Resource};

/// Application keys have a reserved zero-byte prefix in the shared backend.
/// The full prefixed key must also fit the backend's configured key limit.
pub const MAX_APPLICATION_KEY_BYTES: usize = 255;

/// Interpret an already staged backend key using this adapter's application
/// partition. This confers no authentication; consumers must only inspect an
/// exclusively owned, validated composition transaction.
pub fn application_key(key: &[u8]) -> Option<&[u8]> {
    key.strip_prefix(&[0])
        .filter(|key| key.len() <= MAX_APPLICATION_KEY_BYTES)
}

/// MLS work whose output and application facts publish in one transaction.
pub enum Command<'a> {
    Create {
        group: GroupId,
    },
    KeyPackage {
        now_secs: u64,
    },
    Join {
        group: GroupId,
        welcome: &'a [u8],
        now_secs: u64,
    },
    Add {
        group: GroupId,
        packages: &'a [&'a [u8]],
        now_secs: u64,
    },
    Rekey {
        group: GroupId,
    },
    Remove {
        group: GroupId,
        leaf: u32,
    },
    RemoveMember {
        group: GroupId,
        member: crate::MemberBinding,
    },
    /// The caller must authenticate the terminal control and stage its durable
    /// facts and exact notification work in the same composition transaction.
    Retire {
        group: GroupId,
    },
    ProposeUpdate {
        group: GroupId,
    },
    Send {
        group: GroupId,
        body: &'a [u8],
    },
    Receive {
        group: GroupId,
        bytes: &'a [u8],
    },
}

#[derive(Debug)]
pub enum CompositionError<E, A> {
    Mls(Error<E>),
    Application(A),
    /// An application storage call failed but the callback discarded its error.
    ApplicationStorageFault,
}

impl<E: fmt::Debug, A: fmt::Debug> fmt::Display for CompositionError<E, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mls(error) => fmt::Display::fmt(error, f),
            Self::Application(_) => f.write_str("group application staging failed"),
            Self::ApplicationStorageFault => f.write_str("group application storage failed"),
        }
    }
}
impl<E: fmt::Debug, A: fmt::Debug> core::error::Error for CompositionError<E, A> {}

impl<E, A> From<Error<E>> for CompositionError<E, A> {
    fn from(error: Error<E>) -> Self {
        Self::Mls(error)
    }
}
impl<E, A> From<MlsError> for CompositionError<E, A> {
    fn from(error: MlsError) -> Self {
        Self::Mls(error.into())
    }
}
impl<E, A> From<ProviderError> for CompositionError<E, A> {
    fn from(error: ProviderError) -> Self {
        Self::Mls(error.into())
    }
}
impl<E, A> From<PolicyError> for CompositionError<E, A> {
    fn from(error: PolicyError) -> Self {
        Self::Mls(error.into())
    }
}
impl<E, A> From<Limit> for CompositionError<E, A> {
    fn from(error: Limit) -> Self {
        Self::Mls(error.into())
    }
}
impl<E, A> From<StoreError<E>> for CompositionError<E, A> {
    fn from(error: StoreError<E>) -> Self {
        Self::Mls(error.into())
    }
}
impl<E> CompositionError<E, Infallible> {
    pub(super) fn into_mls(self) -> Error<E> {
        match self {
            Self::Mls(error) => error,
            Self::Application(never) => match never {},
            Self::ApplicationStorageFault => Error::CorruptState,
        }
    }
}

/// A borrowed, non-cloneable application-only view of the active MLS transaction.
/// Reads include staged writes. No crypto key, output, commit, or coordinator
/// handle is exposed. Any failed call prevents the enclosing operation committing,
/// even when the callback ignores its error. Returned bytes are zeroized on drop.
pub struct ApplicationTransaction<'a, B> {
    transaction: &'a Transaction<B>,
    failed: Cell<bool>,
    max_pending_outputs: usize,
    max_groups: usize,
}

/// Canonical bounded backend key for an MLS application's reserved partition.
/// Constructing a key grants no authorization; use only an owning coordinator
/// or a validated composition transaction when reading the corresponding value.
pub struct ApplicationKey {
    bytes: [u8; MAX_APPLICATION_KEY_BYTES + 1],
    len: usize,
}
impl ApplicationKey {
    pub fn new(key: &[u8]) -> Result<Self, prns_groups_core::CapacityError> {
        checked_charge(
            0,
            0,
            key.len(),
            MAX_APPLICATION_KEY_BYTES,
            Resource::KeyBytes,
        )?;
        let mut bytes = [0; MAX_APPLICATION_KEY_BYTES + 1];
        bytes[1..key.len() + 1].copy_from_slice(key);
        Ok(Self {
            bytes,
            len: key.len() + 1,
        })
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
impl<B: AtomicBackend> ApplicationTransaction<'_, B> {
    /// Group identifiers in this transaction's bounded MLS catalog. This exposes
    /// no key material or result body and permits accounting shared application
    /// obligations against one consistent transaction snapshot.
    pub fn groups(&self) -> Result<Vec<GroupId>, StoreError<B::Error>> {
        self.checked((|| {
            let bytes = self.transaction.read(b"groups")?.unwrap_or_default();
            if bytes.len() % 32 != 0 || bytes.len() / 32 > self.max_groups {
                return Err(StoreError::BackendContractViolation);
            }
            let ids = bytes.as_chunks::<32>().0;
            if ids
                .iter()
                .enumerate()
                .any(|(index, id)| ids[..index].contains(id))
            {
                return Err(StoreError::BackendContractViolation);
            }
            let mut groups = Vec::new();
            groups
                .try_reserve_exact(ids.len())
                .map_err(|_| StoreError::AllocationFailed)?;
            groups.extend(ids.iter().copied().map(GroupId));
            Ok(groups)
        })())
    }
    /// Remaining durable MLS-result slots, including the current operation's
    /// already reserved output. Applications can reserve space for owed receipts
    /// without access to crypto state or keys.
    pub fn remaining_output_capacity(&self) -> Result<usize, StoreError<B::Error>> {
        self.checked((|| {
            let bytes = self.transaction.read(b"outputs")?.unwrap_or_default();
            if bytes.len() % 32 != 0 || bytes.len() / 32 > self.max_pending_outputs {
                return Err(StoreError::BackendContractViolation);
            }
            Ok(self.max_pending_outputs - bytes.len() / 32)
        })())
    }
    fn checked<T>(
        &self,
        result: Result<T, StoreError<B::Error>>,
    ) -> Result<T, StoreError<B::Error>> {
        if result.is_err() {
            self.failed.set(true);
        }
        result
    }
    pub fn read(&self, key: &[u8]) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError<B::Error>> {
        self.checked((|| {
            self.transaction
                .read(ApplicationKey::new(key)?.bytes())
                .map(|value| value.map(Zeroizing::new))
        })())
    }
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<(), StoreError<B::Error>> {
        self.checked((|| {
            self.transaction
                .put(ApplicationKey::new(key)?.bytes(), value)
        })())
    }
    pub fn delete(&self, key: &[u8]) -> Result<(), StoreError<B::Error>> {
        self.checked((|| {
            self.transaction.delete(ApplicationKey::new(key)?.bytes())
        })())
    }
}

impl<B: AtomicBackend + Send> Adapter<B> {
    /// Commit bounded application maintenance and retire consumed MLS outputs
    /// in the same transaction, without performing crypto or creating output.
    /// The caller must retain every live recovery obligation. Original operation
    /// tickets remain consumed by backend receipts and their retirement fence.
    pub fn maintain<A>(
        &self,
        operation: OperationId,
        completed: &[OperationId],
        stage: impl FnOnce(&ApplicationTransaction<'_, B>) -> Result<(), A>,
    ) -> Result<CommitReceipt, CompositionError<B::Error, A>> {
        if completed.len() > self.limits.max_pending_outputs {
            return Err(Limit::PendingOutputs.into());
        }
        let tx = self
            .coordinator
            .begin(operation, self.coordinator.head()?)?;
        let _attempt = Attempt(tx.clone());
        let provider = Providers::new(tx.clone(), self.limits);
        let result = (|| {
            self.configuration(&provider)?;
            let application = ApplicationTransaction {
                transaction: &tx,
                failed: Cell::new(false),
                max_pending_outputs: self.limits.max_pending_outputs,
                max_groups: self.limits.max_groups,
            };
            stage(&application).map_err(CompositionError::Application)?;
            if application.failed.get() {
                return Err(CompositionError::ApplicationStorageFault);
            }
            for completed in completed {
                if *completed == operation {
                    return Err(CompositionError::Mls(Error::CorruptState));
                }
                provider.delete(&key(b'O', &completed.0))?;
                provider.remove_index(b"outputs", &completed.0, self.limits.max_pending_outputs)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            tx.abort()?;
            return Err(provider
                .fault()
                .map_or(error, |error| CompositionError::Mls(Error::Provider(error))));
        }
        self.commit(tx, operation).map_err(CompositionError::Mls)
    }

    /// Bounded inventory of durable results, for a trusted owner's maintenance
    /// plan. This is not evidence that any remote recipient received them.
    pub fn pending_operations(&self) -> Result<Vec<OperationId>, Error<B::Error>> {
        let Some(bytes) = self.coordinator.read(b"outputs")? else {
            return Ok(Vec::new());
        };
        if bytes.len() % 32 != 0 || bytes.len() / 32 > self.limits.max_pending_outputs {
            return Err(Error::CorruptState);
        }
        let operations = bytes.as_chunks::<32>().0;
        if operations
            .iter()
            .enumerate()
            .any(|(i, id)| operations[..i].contains(id))
        {
            return Err(Error::CorruptState);
        }
        Ok(bytes
            .as_chunks::<32>()
            .0
            .iter()
            .copied()
            .map(OperationId)
            .collect())
    }

    /// Read a committed application fact. Recovery and active-operation fences
    /// apply exactly as they do to MLS output retries.
    pub fn application_read(
        &self,
        key: &[u8],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, Error<B::Error>> {
        Ok(self
            .coordinator
            .read(ApplicationKey::new(key).map_err(StoreError::from)?.bytes())?
            .map(Zeroizing::new))
    }

    /// Atomically stage application facts with MLS state and exact durable output.
    ///
    /// Callbacks must only inspect and stage local data. Do not publish output,
    /// perform external effects, or reenter the coordinator from a callback.
    /// `admission` sees authenticated application bodies and transaction facts;
    /// `stage` sees the resulting output, including authenticated rejection. The
    /// output is still provisional at that point. Only a successful return proves
    /// commitment. Errors and unwinding abort; uncertain commits stay fenced.
    ///
    /// Recovery uses `coordinator().recover`, `retry` and `application_read`; it
    /// never repeats callbacks or encryption for an already committed operation.
    pub fn compose<A>(
        &self,
        operation: OperationId,
        command: Command<'_>,
        admission: impl FnOnce(
            &ApplicationTransaction<'_, B>,
            &crate::MemberBinding,
            &[u8],
        ) -> Result<Admission, A>,
        stage: impl FnOnce(&ApplicationTransaction<'_, B>, &Output) -> Result<(), A>,
    ) -> Result<Committed, CompositionError<B::Error, A>> {
        self.compose_with_roster(operation, command, None, admission, stage)
    }

    /// Like `compose`, additionally require the resulting MLS tree to exactly
    /// equal an authenticated roster before committing any crypto or app state.
    pub fn compose_with_roster<A>(
        &self,
        operation: OperationId,
        command: Command<'_>,
        expected: Option<&[crate::MemberBinding]>,
        admission: impl FnOnce(
            &ApplicationTransaction<'_, B>,
            &crate::MemberBinding,
            &[u8],
        ) -> Result<Admission, A>,
        stage: impl FnOnce(&ApplicationTransaction<'_, B>, &Output) -> Result<(), A>,
    ) -> Result<Committed, CompositionError<B::Error, A>> {
        self.execute_transaction(operation, |provider, tx| {
            let application = ApplicationTransaction {
                transaction: tx,
                failed: Cell::new(false),
                max_pending_outputs: self.limits.max_pending_outputs,
                max_groups: self.limits.max_groups,
            };
            let output = match command {
                Command::Create { group } => self.prepare_create(provider, group)?,
                Command::KeyPackage { now_secs } => self.prepare_key_package(provider, now_secs)?,
                Command::Join {
                    group,
                    welcome,
                    now_secs,
                } => self.prepare_join(provider, group, welcome, now_secs)?,
                Command::Add {
                    group,
                    packages,
                    now_secs,
                } => self.prepare_add(provider, group, packages, now_secs)?,
                Command::Rekey { group } => self.prepare_rekey(provider, group)?,
                Command::Remove { group, leaf } => self.prepare_remove(provider, group, leaf)?,
                Command::RemoveMember { group, member } => {
                    self.prepare_remove_member(provider, group, &member)?
                }
                Command::Retire { group } => {
                    if expected.is_some() {
                        return Err(PolicyError::UnsupportedControl.into());
                    }
                    self.prepare_retire(provider, group)?
                }
                Command::ProposeUpdate { group } => self.prepare_propose_update(provider, group)?,
                Command::Send { group, body } => self.prepare_send(provider, group, body)?,
                Command::Receive { group, bytes } => {
                    self.prepare_receive(provider, group, bytes, expected, |binding, body| {
                        admission(&application, binding, body)
                    })?
                }
            };
            if let Some(expected) = expected.filter(|_| output.event != Event::Removed) {
                let context = output.context.ok_or(Error::UnexpectedMessage)?;
                let state = self.load(provider, context.group)?;
                let roster = state.roster();
                if roster.members_iter().count() != expected.len()
                    || expected.iter().any(|binding| {
                        !roster
                            .member_identities_iter()
                            .any(|identity| binding.matches(identity))
                    })
                    || expected.iter().enumerate().any(|(index, binding)| {
                        expected[..index]
                            .iter()
                            .any(|prior| prior.endpoint == binding.endpoint)
                    })
                {
                    return Err(PolicyError::Credential.into());
                }
            }
            if application.failed.get() {
                return Err(CompositionError::ApplicationStorageFault);
            }
            stage(&application, &output).map_err(CompositionError::Application)?;
            if application.failed.get() {
                return Err(CompositionError::ApplicationStorageFault);
            }
            Ok(output)
        })
    }
}
