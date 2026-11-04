mod composition;
mod management;
mod operations;

pub use composition::{
    application_key, ApplicationKey, ApplicationTransaction, Command, CompositionError,
    MAX_APPLICATION_KEY_BYTES,
};

use crate::{
    policy::Rules,
    provider::{key, Providers},
};
use crate::{
    Event, Limit, Limits, LocalIdentity, Output, OutputContext, Policy, PolicyError, ProviderError,
};
use alloc::{vec, vec::Vec};
use core::fmt;
use mls_rs::{
    client_builder::{
        BaseConfig, WithCryptoProvider, WithGroupStateStorage, WithIdentityProvider,
        WithKeyPackageRepo, WithMlsRules,
    },
    error::MlsError,
    group::{proposal::Proposal, CommitEffect, ReceivedMessage},
    CipherSuite, Client, Group, MlsMessage, WireFormat,
};
use mls_rs_codec::{MlsDecode, MlsEncode, MlsSize};
use mls_rs_crypto_rustcrypto::RustCryptoProvider;
use prns_groups_core::{GroupId, OperationId};
use prns_groups_store::{
    AtomicBackend, CommitOutcome, CommitReceipt, Coordinator, StoreError, Transaction,
};
use zeroize::Zeroizing;

type ConfigFor<B> = WithMlsRules<
    Rules,
    WithCryptoProvider<
        RustCryptoProvider,
        WithIdentityProvider<
            Policy,
            WithGroupStateStorage<Providers<B>, WithKeyPackageRepo<Providers<B>, BaseConfig>>,
        >,
    >,
>;
type GroupFor<B> = Group<ConfigFor<B>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Accept,
    Reject(u16),
}
#[derive(Debug)]
pub enum Error<E> {
    Store(StoreError<E>),
    Mls(MlsError),
    Provider(ProviderError),
    Capacity(Limit),
    Policy(PolicyError),
    CorruptState,
    ConfigurationChanged,
    GroupExists,
    GroupMissing,
    UnexpectedMessage,
    NotCommitted,
    Uncertain(OperationId),
    PriorAuthorizationEra,
    TimeUncertain,
}
impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "group MLS error: {self:?}")
    }
}
impl<E: fmt::Debug> core::error::Error for Error<E> {}
impl<E> From<MlsError> for Error<E> {
    fn from(error: MlsError) -> Self {
        Self::Mls(error)
    }
}
impl<E> From<StoreError<E>> for Error<E> {
    fn from(error: StoreError<E>) -> Self {
        Self::Store(error)
    }
}
impl<E> From<ProviderError> for Error<E> {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}
impl<E> From<Limit> for Error<E> {
    fn from(error: Limit) -> Self {
        Self::Capacity(error)
    }
}
impl<E> From<PolicyError> for Error<E> {
    fn from(error: PolicyError) -> Self {
        Self::Policy(error)
    }
}

#[derive(Debug)]
pub struct Committed {
    pub receipt: CommitReceipt,
    pub output: Output,
}

// On host unwinding, revoke provider handles before another operation can run.
// After commit this token is stale; an uncertain commit remains fenced because
// abort cannot clear a poisoned coordinator.
struct Attempt<B: AtomicBackend>(Transaction<B>);
impl<B: AtomicBackend> Drop for Attempt<B> {
    fn drop(&mut self) {
        let _ = self.0.abort();
    }
}

/// Own one exclusively coordinated store per endpoint. No MLS Group or client
/// escapes an operation. Returned output is always backed by a durable receipt.
pub struct Adapter<B> {
    coordinator: Coordinator<B>,
    identity: LocalIdentity,
    policy: Policy,
    limits: Limits,
}
impl<B: AtomicBackend + Send> Adapter<B> {
    pub fn new(
        coordinator: Coordinator<B>,
        identity: LocalIdentity,
        policy: Policy,
        limits: Limits,
    ) -> Result<Self, Error<B::Error>> {
        limits.validate()?;
        if policy.authorized.len() > limits.max_members {
            return Err(Limit::Members.into());
        }
        policy.validate(&identity.signing_identity())?;
        let adapter = Self {
            coordinator,
            identity,
            policy,
            limits,
        };
        if let Some(stored) = adapter.coordinator.read(b"configuration")? {
            if stored != adapter.configuration_bytes() {
                return Err(Error::ConfigurationChanged);
            }
        }
        Ok(adapter)
    }
    pub fn coordinator(&self) -> &Coordinator<B> {
        &self.coordinator
    }
    /// Reject oversized transport input before application hashing or lookup.
    pub const fn max_wire_bytes(&self) -> usize {
        self.limits.max_wire_bytes
    }
    /// The authenticated endpoint identity owned by this adapter.
    pub fn local_binding(&self) -> crate::MemberBinding {
        self.identity.binding()
    }

    /// A durable terminal fence for local removal or authenticated closure.
    /// Retained application history remains readable through application_read.
    pub fn is_retired(&self, group: GroupId) -> Result<bool, Error<B::Error>> {
        Ok(self.coordinator.read(&key(b'R', &group.0))?.is_some())
    }
    /// Check a directed join claim against the actual encoded KeyPackage.
    /// Full signature/lifetime validation still occurs in the atomic Add operation.
    pub fn key_package_matches(
        &self,
        bytes: &[u8],
        binding: &crate::MemberBinding,
    ) -> Result<bool, Error<B::Error>> {
        let message = self.decode(bytes)?;
        let package = message.into_key_package().ok_or(Error::UnexpectedMessage)?;
        Ok(package.cipher_suite() == CipherSuite::CURVE25519_AES128
            && binding.matches(package.signing_identity()))
    }
    /// Read only the group and epoch of a retained result, including results
    /// whose ciphertext is fenced by an authorization transition. This permits
    /// scoped maintenance without authorizing export through `retry`.
    pub fn output_context(
        &self,
        operation: OperationId,
    ) -> Result<Option<OutputContext>, Error<B::Error>> {
        let Some(bytes) = self.coordinator.read(&key(b'O', &operation.0))? else {
            return Ok(None);
        };
        let bytes = Zeroizing::new(bytes);
        let output = Output::decode(&bytes, self.limits).map_err(|_| Error::CorruptState)?;
        Ok(output.context)
    }
    pub fn retry(&self, operation: OperationId) -> Result<Option<Output>, Error<B::Error>> {
        let Some(bytes) = self.coordinator.read(&key(b'O', &operation.0))? else {
            return Ok(None);
        };
        let bytes = Zeroizing::new(bytes);
        let output = Output::decode(&bytes, self.limits).map_err(|_| Error::CorruptState)?;
        if matches!(output.event, Event::Sent | Event::Proposal) && !output.messages.is_empty() {
            let context = output.context.ok_or(Error::CorruptState)?;
            let floor = self
                .coordinator
                .read(&key(b'B', &context.group.0))?
                .ok_or(Error::CorruptState)?;
            let floor = u64::from_be_bytes(
                floor
                    .as_slice()
                    .try_into()
                    .map_err(|_| Error::CorruptState)?,
            );
            if context.epoch < floor
                || self
                    .coordinator
                    .read(&key(b'R', &context.group.0))?
                    .is_some()
            {
                return Err(Error::PriorAuthorizationEra);
            }
        }
        Ok(Some(output))
    }
    /// Delete a consumed durable result using a separate unique operation ID.
    /// Backends retain the original commit receipt, so acknowledgement does not
    /// authorize reusing the original operation ID for fresh encryption.
    pub fn acknowledge(
        &self,
        operation: OperationId,
        completed: OperationId,
    ) -> Result<CommitReceipt, Error<B::Error>> {
        let tx = self
            .coordinator
            .begin(operation, self.coordinator.head()?)?;
        let _attempt = Attempt(tx.clone());
        let provider = Providers::new(tx.clone(), self.limits);
        let result = (|| {
            self.configuration(&provider)?;
            provider.delete(&key(b'O', &completed.0))?;
            provider.remove_index(b"outputs", &completed.0, self.limits.max_pending_outputs)?;
            Ok(())
        })();
        if let Err(error) = result {
            tx.abort()?;
            return Err(error);
        }
        self.commit(tx, operation)
    }
    fn context(mut output: Output, group: GroupId, epoch: u64) -> Output {
        output.context = Some(OutputContext { group, epoch });
        output
    }
    fn client(&self, provider: Providers<B>) -> Client<ConfigFor<B>> {
        Client::builder()
            .key_package_repo(provider.clone())
            .group_state_storage(provider)
            .identity_provider(self.policy.clone())
            .crypto_provider(RustCryptoProvider::with_enabled_cipher_suites(vec![
                CipherSuite::CURVE25519_AES128,
            ]))
            .mls_rules(Rules {
                policy: self.policy.clone(),
                max_members: self.limits.max_members,
                max_proposals: self.limits.max_pending_proposals,
            })
            .signing_identity(
                self.identity.signing_identity(),
                self.identity.secret.clone(),
                CipherSuite::CURVE25519_AES128,
            )
            .build()
    }
    fn load(
        &self,
        provider: &Providers<B>,
        group: GroupId,
    ) -> Result<GroupFor<B>, Error<B::Error>> {
        let groups = provider.index(b"groups", 32, self.limits.max_groups)?;
        if !groups.as_chunks::<32>().0.contains(&group.0)
            || provider.read(&key(b'R', &group.0))?.is_some()
        {
            return Err(Error::GroupMissing);
        }
        let client = self.client(provider.clone());
        let state = client.load_group(&group.0)?;
        if state.group_id() != group.0 {
            return Err(Error::CorruptState);
        }
        self.validate_group(&state)?;
        Ok(state)
    }
    fn validate_group(&self, state: &GroupFor<B>) -> Result<(), Error<B::Error>> {
        if state.cipher_suite() != CipherSuite::CURVE25519_AES128
            || !self
                .identity
                .binding
                .matches(state.current_member_signing_identity()?)
        {
            return Err(PolicyError::Credential.into());
        }
        let roster = state.roster();
        if roster.members_iter().count() > self.limits.max_members {
            return Err(Limit::Members.into());
        }
        let mut owner = false;
        for identity in roster.member_identities_iter() {
            self.policy.validate(identity)?;
            owner |= self.policy.owner.matches(identity);
        }
        if !owner {
            return Err(PolicyError::Owner.into());
        }
        if state.get_cached_proposals().len() > self.limits.max_pending_proposals {
            return Err(Limit::Proposals.into());
        }
        Ok(())
    }
    fn owner(&self) -> Result<(), Error<B::Error>> {
        if self.identity.binding != self.policy.owner {
            return Err(PolicyError::Owner.into());
        }
        Ok(())
    }
    fn reserve_group(
        &self,
        provider: &Providers<B>,
        group: GroupId,
    ) -> Result<(), Error<B::Error>> {
        let groups = provider.index(b"groups", 32, self.limits.max_groups)?;
        if groups.as_chunks::<32>().0.contains(&group.0) {
            return Err(Error::GroupExists);
        }
        provider.add_index(b"groups", &group.0, self.limits.max_groups, Limit::Groups)?;
        Ok(())
    }
    fn charge_proposal(
        &self,
        provider: &Providers<B>,
        group: GroupId,
    ) -> Result<(), Error<B::Error>> {
        let name = key(b'P', &group.0);
        let count = match provider.read(&name)? {
            Some(value) => u32::from_be_bytes(
                value
                    .as_slice()
                    .try_into()
                    .map_err(|_| Error::CorruptState)?,
            ) as usize,
            None => 0,
        };
        if count >= self.limits.max_pending_proposals {
            return Err(Limit::Proposals.into());
        }
        provider.put(&name, &((count + 1) as u32).to_be_bytes())?;
        Ok(())
    }
    fn decode(&self, bytes: &[u8]) -> Result<MlsMessage, Error<B::Error>> {
        if bytes.len() > self.limits.max_wire_bytes {
            return Err(Limit::WireBytes.into());
        }
        let mut cursor = bytes;
        let message = MlsMessage::mls_decode(&mut cursor).map_err(MlsError::from)?;
        if !cursor.is_empty() {
            return Err(Error::UnexpectedMessage);
        }
        Ok(message)
    }
    fn encode(&self, message: MlsMessage) -> Result<Vec<u8>, Error<B::Error>> {
        let size = message.mls_encoded_len();
        if size > self.limits.max_wire_bytes {
            return Err(Limit::WireBytes.into());
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| Limit::Allocation)?;
        message.mls_encode(&mut bytes).map_err(MlsError::from)?;
        Ok(bytes)
    }
    fn configuration_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![2];
        for field in self.limits.fields() {
            bytes.extend_from_slice(&(field as u32).to_be_bytes());
        }
        self.policy.encode(&mut bytes);
        bytes.extend_from_slice(&self.identity.binding.endpoint);
        bytes.extend_from_slice(&self.identity.binding.incarnation.to_be_bytes());
        bytes.extend_from_slice(&self.identity.binding.signing_key);
        bytes
    }
    fn configuration(&self, provider: &Providers<B>) -> Result<(), Error<B::Error>> {
        let bytes = self.configuration_bytes();
        match provider.read(b"configuration")? {
            Some(old) if old.as_slice() != bytes => Err(Error::ConfigurationChanged),
            Some(_) => Ok(()),
            None => {
                provider.put(b"configuration", &bytes)?;
                Ok(())
            }
        }
    }
    fn execute(
        &self,
        operation: OperationId,
        work: impl FnOnce(&Providers<B>) -> Result<Output, Error<B::Error>>,
    ) -> Result<Committed, Error<B::Error>> {
        self.execute_transaction(operation, |provider, _| {
            work(provider).map_err(CompositionError::Mls)
        })
        .map_err(CompositionError::into_mls)
    }
    fn execute_transaction<A>(
        &self,
        operation: OperationId,
        work: impl FnOnce(
            &Providers<B>,
            &Transaction<B>,
        ) -> Result<Output, CompositionError<B::Error, A>>,
    ) -> Result<Committed, CompositionError<B::Error, A>> {
        let tx = self
            .coordinator
            .begin(operation, self.coordinator.head()?)?;
        let _attempt = Attempt(tx.clone());
        let provider = Providers::new(tx.clone(), self.limits);
        let prepared = (|| {
            self.configuration(&provider)?;
            provider.add_index(
                b"outputs",
                &operation.0,
                self.limits.max_pending_outputs,
                Limit::PendingOutputs,
            )?;
            let output = work(&provider, &tx)?;
            let encoded = output.encode(self.limits)?;
            provider.put(&key(b'O', &operation.0), &encoded)?;
            Ok(output)
        })();
        let output = match prepared {
            Ok(output) => output,
            Err(error) => {
                tx.abort()?;
                return Err(provider
                    .fault()
                    .map_or(error, |error| CompositionError::Mls(Error::Provider(error))));
            }
        };
        let receipt = self.commit(tx, operation)?;
        Ok(Committed { receipt, output })
    }
    fn commit(
        &self,
        tx: Transaction<B>,
        operation: OperationId,
    ) -> Result<CommitReceipt, Error<B::Error>> {
        match tx.commit()? {
            CommitOutcome::Committed(receipt) => Ok(receipt),
            CommitOutcome::NotCommitted { .. } => Err(Error::NotCommitted),
            CommitOutcome::Uncertain => Err(Error::Uncertain(operation)),
            CommitOutcome::Rejected(reason) => Err(StoreError::OperationRejected(reason).into()),
        }
    }
}
