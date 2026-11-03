//! Bounded caller-owned creation requests in the same durable catalog envelope.
//!
//! Persist a scoped request ID before submission. Retrying that exact ID recovers
//! the same operation; choosing a new scope/nonce is new work, never a retry.

use crate::{files, Error, IdentitySecretKey, IdentityStore, Result};
use prns_core::crypto::sha256;
use prns_core::identity::PrivateIdentityMaterial;
use prns_identities_core::{
    validate_create_request, Action, Catalog, Command, CommandDigest, Coordinator, CreateRequest,
    Fault, KeySource, OperationResult, OperationState, OperationToken, Storage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "IdentityCreateRequestScope")
)]
pub struct CreateRequestScope {
    pub incarnation: [u8; 16],
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityCreateRequestId"))]
pub struct CreateRequestId {
    pub scope: CreateRequestScope,
    pub nonce: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "IdentityCreateOutcome"))]
pub struct CreateOutcome {
    pub operation: OperationToken,
    pub result: OperationResult,
}

#[derive(Debug)]
pub enum CreateError {
    Storage(Error),
    Authority(Fault),
    Conflict,
    Expired,
    FutureScope,
    ForeignCatalog,
    Capacity,
    UnknownRequest,
    Pending,
    PruningBlocked,
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "identity creation: {self:?}")
    }
}
impl std::error::Error for CreateError {}
impl From<Error> for CreateError {
    fn from(error: Error) -> Self {
        Self::Storage(error)
    }
}
impl From<Fault> for CreateError {
    fn from(error: Fault) -> Self {
        Self::Authority(error)
    }
}
impl From<prns_identities_core::Error<Error>> for CreateError {
    fn from(error: prns_identities_core::Error<Error>) -> Self {
        match error {
            prns_identities_core::Error::Fault(error) => Self::Authority(error),
            prns_identities_core::Error::Storage(error) => Self::Storage(error),
        }
    }
}
pub type CreateResult<T> = std::result::Result<T, CreateError>;

#[derive(Clone, Copy)]
pub(crate) struct Record<const S: usize> {
    request: CreateRequestId,
    intent: CreateRequest<S>,
    token: OperationToken,
    result: Option<OperationResult>,
}

pub(crate) struct Evidence<const S: usize> {
    epoch: u64,
    records: Vec<Record<S>>,
}
impl<const S: usize> Evidence<S> {
    pub(crate) fn new() -> Self {
        Self {
            epoch: 1,
            records: Vec::new(),
        }
    }

    fn scope(&self, incarnation: [u8; 16]) -> CreateRequestScope {
        CreateRequestScope {
            incarnation,
            epoch: self.epoch,
        }
    }

    fn check_scope(&self, scope: CreateRequestScope, incarnation: [u8; 16]) -> CreateResult<()> {
        if scope.incarnation != incarnation {
            return Err(CreateError::ForeignCatalog);
        }
        if scope.epoch > self.epoch {
            return Err(CreateError::FutureScope);
        }
        if scope.epoch == 0 {
            return Err(CreateError::Expired);
        }
        Ok(())
    }

    fn validate<const I: usize, const O: usize>(&self, catalog: &Catalog<I, O, S>) -> Result<()> {
        if self.epoch == 0 || self.records.len() > O {
            return Err(Error::Corrupt);
        }
        for (index, record) in self.records.iter().enumerate() {
            if record.request.scope.incarnation != catalog.incarnation
                || record.request.scope.epoch == 0
                || record.request.scope.epoch > self.epoch
                || record.token.incarnation != catalog.incarnation
                || record.token.sequence == 0
                || record.token.sequence > catalog.issued_sequence
                || record.token.digest != request_digest(record.request)
                || validate_create_request(&record.intent).is_err()
                || self.records[..index].iter().any(|earlier| {
                    earlier.request == record.request
                        || earlier.token.sequence == record.token.sequence
                })
            {
                return Err(Error::Corrupt);
            }
            if let Some(operation) = catalog.operations().find(|op| op.token == record.token) {
                if operation.command != Command::Create(record.intent) {
                    return Err(Error::Corrupt);
                }
                let result = match operation.state {
                    OperationState::Completed(result) => Some(result),
                    _ => None,
                };
                if record.result != result {
                    return Err(Error::Corrupt);
                }
            } else if record.token.sequence > catalog.completed_floor || record.result.is_none() {
                return Err(Error::Corrupt);
            }
            match record.result {
                Some(
                    OperationResult::Created { identity } | OperationResult::Existing { identity },
                ) => {
                    let identity_record = catalog
                        .identities()
                        .find(|entry| entry.id == identity)
                        .ok_or(Error::Corrupt)?;
                    if matches!(record.result, Some(OperationResult::Created { .. }))
                        && (identity.sequence != record.token.sequence
                            || record.intent.services != identity_record.initial_services)
                    {
                        return Err(Error::Corrupt);
                    }
                    match record.intent.source {
                        KeySource::Generate
                            if matches!(record.result, Some(OperationResult::Existing { .. })) =>
                        {
                            return Err(Error::Corrupt)
                        }
                        KeySource::Import { expected_hash }
                            if identity_record.public_hash != expected_hash =>
                        {
                            return Err(Error::Corrupt)
                        }
                        _ => {}
                    }
                }
                Some(OperationResult::Cancelled) | None => {}
                _ => return Err(Error::Corrupt),
            }
        }
        Ok(())
    }

    fn capture<const I: usize, const O: usize>(
        &mut self,
        current: &Catalog<I, O, S>,
        next: &Catalog<I, O, S>,
    ) -> Result<bool> {
        let mut completed = false;
        for record in &mut self.records {
            let operation = next
                .operations()
                .find(|op| op.token == record.token)
                .or_else(|| current.operations().find(|op| op.token == record.token));
            if let Some(operation) = operation {
                if operation.command != Command::Create(record.intent) {
                    return Err(Error::Corrupt);
                }
                if let OperationState::Completed(result) = operation.state {
                    if record.result.is_some_and(|old| old != result) {
                        return Err(Error::Corrupt);
                    }
                    completed |= record.result.is_none();
                    record.result = Some(result);
                }
            }
        }
        Ok(completed)
    }
}

pub(crate) enum Change {
    Reserve { request: CreateRequestId },
    Retire(Vec<CreateRequestId>),
}

struct Update<'a, const I: usize, const O: usize, const S: usize> {
    store: &'a mut IdentityStore<I, O, S>,
    change: Option<Change>,
}
impl<const I: usize, const O: usize, const S: usize> Storage<I, O, S> for Update<'_, I, O, S> {
    type Error = Error;
    fn load(&mut self, into: &mut Catalog<I, O, S>) -> Result<()> {
        self.store.read_catalog(into)
    }
    fn compare_and_store(&mut self, revision: u64, next: &Catalog<I, O, S>) -> Result<()> {
        self.store
            .compare_and_store_creation(revision, next, self.change.take())
    }
}

impl<const I: usize, const O: usize, const S: usize> IdentityStore<I, O, S> {
    /// Obtain the current scope, then choose and persist a nonce before submitting.
    /// Reading a scope reserves no operation and authorizes no identity effect.
    pub fn creation_scope(&self) -> Result<CreateRequestScope> {
        let mut catalog = Box::new(Catalog::new([0; 16]));
        let evidence = self.read_envelope(&mut catalog)?;
        Ok(evidence.scope(catalog.incarnation))
    }

    /// Create or recover one exact canonical intent. No activation, host, default
    /// service, UI selection or messaging policy is applied. Import material is a
    /// native Rust capability and must not be exported through foreign bindings.
    ///
    /// Caller request evidence is admitted in the same commit as reservation.
    /// Key publication is ordered separately, then result evidence and core
    /// pruning share catalog-envelope commits. Storage errors require coordinator
    /// reload; retry with this exact ID, never an automatically refreshed scope.
    pub fn create_or_recover(
        &mut self,
        coordinator: &mut Coordinator<'_, I, O, S>,
        request: CreateRequestId,
        intent: CreateRequest<S>,
        import: Option<&IdentitySecretKey>,
    ) -> CreateResult<CreateOutcome> {
        self.create_or_recover_validated(coordinator, request, intent, import, |_| Ok(()))
    }

    /// Adapter policy validates only fresh admission. An exact retained request
    /// keeps its original recovery semantics when an optional adapter is added.
    pub(crate) fn create_or_recover_validated(
        &mut self,
        coordinator: &mut Coordinator<'_, I, O, S>,
        request: CreateRequestId,
        intent: CreateRequest<S>,
        import: Option<&IdentitySecretKey>,
        validate: impl FnOnce(prns_identities_core::Metadata) -> std::result::Result<(), Fault>,
    ) -> CreateResult<CreateOutcome> {
        self.validate_owner(coordinator)?;
        let mut catalog = Box::new(Catalog::new([0; 16]));
        let evidence = self.read_envelope(&mut catalog)?;
        evidence.check_scope(request.scope, catalog.incarnation)?;
        let existing = evidence
            .records
            .iter()
            .find(|record| record.request == request)
            .copied();
        if existing.is_some_and(|record| record.intent != intent) {
            return Err(CreateError::Conflict);
        }
        validate_create_request(&intent)?;
        if let Some(secret) = import {
            match intent.source {
                KeySource::Generate => return Err(Error::UnexpectedImport.into()),
                KeySource::Import { expected_hash } => {
                    if PrivateIdentityMaterial::from(secret.clone())
                        .identity_hash()
                        .as_bytes()
                        != &expected_hash.0
                    {
                        return Err(Error::KeyMismatch.into());
                    }
                }
            }
        }
        let token = if let Some(record) = existing {
            if let Some(result) = record.result {
                self.settle_retained_key(coordinator, record.token)?;
                self.prune_creation_operations(coordinator)?;
                return Ok(CreateOutcome {
                    operation: record.token,
                    result,
                });
            }
            record.token
        } else {
            if request.scope.epoch != evidence.epoch {
                return Err(CreateError::Expired);
            }
            if evidence.records.len() == O {
                return Err(CreateError::Capacity);
            }
            validate(intent.metadata)?;
            let mut update = Update {
                store: self,
                change: Some(Change::Reserve { request }),
            };
            coordinator.reserve(
                &mut update,
                Command::Create(intent),
                request_digest(request),
            )?
        };
        files::crash("create", "after_reserve");
        coordinator.begin(self, token, &Command::Create(intent))?;
        files::crash("create", "after_begin");
        if let Some(Action::PublishKey { ticket, .. }) = coordinator.action(token)? {
            let key = self.publish_key(coordinator, ticket, import)?;
            files::crash("create", "after_key");
            coordinator.ack_key(self, ticket, key)?;
        }
        files::crash("create", "after_ack");
        let operation = *coordinator.operation(token)?;
        self.settle_retained_key(coordinator, token)?;
        let OperationState::Completed(result) = operation.state else {
            return Err(CreateError::Pending);
        };
        self.prune_creation_operations(coordinator)?;
        files::crash("create", "after_prune");
        Ok(CreateOutcome {
            operation: token,
            result,
        })
    }

    /// Confirm receipt of completed results and forget only those exact requests.
    /// Removal and scope rotation are one catalog-envelope commit. Pending or
    /// unaccounted work cannot be retired. A completed operation behind another
    /// caller's journal entry returns `PruningBlocked` until that caller prunes
    /// its barrier. Capacity pressure never rotates scopes.
    /// Already-retired old IDs make this acknowledgment retryable after reply loss.
    pub fn retire_creations(
        &mut self,
        coordinator: &mut Coordinator<'_, I, O, S>,
        confirmed: &[CreateRequestId],
    ) -> CreateResult<CreateRequestScope> {
        self.validate_owner(coordinator)?;
        if confirmed.len() > O {
            return Err(CreateError::Capacity);
        }
        let mut catalog = Box::new(Catalog::new([0; 16]));
        let evidence = self.read_envelope(&mut catalog)?;
        let mut remove = Vec::new();
        remove
            .try_reserve_exact(confirmed.len())
            .map_err(|_| Error::Bounds)?;
        for request in confirmed {
            evidence.check_scope(request.scope, catalog.incarnation)?;
            match evidence
                .records
                .iter()
                .find(|record| record.request == *request)
            {
                Some(record) => {
                    if record.result.is_none()
                        || catalog.operations().any(|operation| {
                            operation.token == record.token && operation.unpublished_key.is_some()
                        })
                    {
                        return Err(CreateError::Pending);
                    }
                    if !remove.contains(request) {
                        remove.push(*request);
                    }
                }
                None if request.scope.epoch < evidence.epoch => {}
                None => return Err(CreateError::UnknownRequest),
            }
        }
        if remove.is_empty() {
            return Ok(evidence.scope(catalog.incarnation));
        }
        self.prune_creation_operations(coordinator)?;
        if evidence.records.iter().any(|record| {
            remove.contains(&record.request)
                && record.token.sequence > coordinator.snapshot().completed_floor
        }) {
            return Err(CreateError::PruningBlocked);
        }
        let epoch = evidence.epoch.checked_add(1).ok_or(Fault::Exhausted)?;
        let mut update = Update {
            store: self,
            change: Some(Change::Retire(remove)),
        };
        coordinator.checkpoint(&mut update)?;
        Ok(CreateRequestScope {
            incarnation: catalog.incarnation,
            epoch,
        })
    }

    fn settle_retained_key(
        &mut self,
        coordinator: &mut Coordinator<'_, I, O, S>,
        token: OperationToken,
    ) -> CreateResult<()> {
        let reference = coordinator
            .snapshot()
            .operations()
            .find(|operation| operation.token == token)
            .and_then(|operation| operation.unpublished_key);
        if let Some(reference) = reference {
            self.account_retained_key(coordinator, token, reference)?;
            coordinator.acknowledge_retained_key(self, token, reference)?;
        }
        Ok(())
    }

    fn prune_creation_operations(
        &mut self,
        coordinator: &mut Coordinator<'_, I, O, S>,
    ) -> CreateResult<()> {
        let mut catalog = Box::new(Catalog::new([0; 16]));
        let evidence = self.read_envelope(&mut catalog)?;
        let through = coordinator
            .snapshot()
            .operations()
            .filter(|operation| {
                operation.unpublished_key.is_some()
                    || !evidence.records.iter().any(|record| {
                        record.token == operation.token
                            && record.result.is_some_and(|result| {
                                operation.state == OperationState::Completed(result)
                            })
                    })
            })
            .map(|operation| operation.token.sequence - 1)
            .min()
            .unwrap_or(coordinator.snapshot().issued_sequence);
        coordinator.prune_completed(self, through)?;
        Ok(())
    }

    pub(crate) fn compare_and_store_creation(
        &mut self,
        expected_revision: u64,
        next: &Catalog<I, O, S>,
        change: Option<Change>,
    ) -> Result<()> {
        let mut current = Box::new(Catalog::new([0; 16]));
        let mut evidence = self.read_envelope(&mut current)?;
        if current.revision != expected_revision {
            return Err(Error::RevisionMismatch {
                expected: expected_revision,
                found: current.revision,
            });
        }
        if current.incarnation != next.incarnation {
            return Err(Error::ForeignCatalog);
        }
        if expected_revision.checked_add(1) != Some(next.revision) {
            return Err(Error::Corrupt);
        }
        let completed = evidence.capture(&current, next)?;
        let family = match change {
            Some(Change::Reserve { request }) => {
                if evidence.records.len() >= O
                    || request.scope != evidence.scope(next.incarnation)
                    || evidence
                        .records
                        .iter()
                        .any(|record| record.request == request)
                    || current.issued_sequence.checked_add(1) != Some(next.issued_sequence)
                {
                    return Err(Error::Corrupt);
                }
                let operation = next
                    .operations()
                    .find(|op| op.token.sequence == next.issued_sequence)
                    .ok_or(Error::Corrupt)?;
                let Command::Create(intent) = operation.command else {
                    return Err(Error::Corrupt);
                };
                if operation.state != OperationState::Reserved
                    || operation.token.digest != request_digest(request)
                {
                    return Err(Error::Corrupt);
                }
                evidence
                    .records
                    .try_reserve_exact(1)
                    .map_err(|_| Error::Bounds)?;
                evidence.records.push(Record {
                    request,
                    intent,
                    token: operation.token,
                    result: None,
                });
                "create-request"
            }
            Some(Change::Retire(requests)) => {
                for request in &requests {
                    let record = evidence
                        .records
                        .iter()
                        .find(|record| record.request == *request)
                        .ok_or(Error::Corrupt)?;
                    if record.result.is_none()
                        || next
                            .operations()
                            .any(|op| op.token == record.token && op.unpublished_key.is_some())
                    {
                        return Err(Error::Corrupt);
                    }
                }
                evidence
                    .records
                    .retain(|record| !requests.contains(&record.request));
                evidence.epoch = evidence.epoch.checked_add(1).ok_or(Error::Bounds)?;
                "create-retire"
            }
            None if completed => "create-result",
            None if next.completed_floor != current.completed_floor
                && !evidence.records.is_empty() =>
            {
                "create-prune"
            }
            None => "catalog",
        };
        evidence.validate(next)?;
        self.write_catalog(next, &evidence, family)
    }
}

fn request_digest(request: CreateRequestId) -> CommandDigest {
    let mut bytes = [0; 72];
    bytes[..16].copy_from_slice(b"PRNS-CREATE-ID01");
    bytes[16..32].copy_from_slice(&request.scope.incarnation);
    bytes[32..40].copy_from_slice(&request.scope.epoch.to_be_bytes());
    bytes[40..].copy_from_slice(&request.nonce);
    CommandDigest(sha256(&bytes))
}

pub(crate) fn envelope_bound<const I: usize, const O: usize, const S: usize>() -> Result<usize> {
    let catalog = prns_identities_codec::max_encoded_len::<I, O, S>().ok_or(Error::Bounds)?;
    let record = prns_identities_codec::max_create_request_encoded_len::<S>()
        .and_then(|n| n.checked_add(120))
        .ok_or(Error::Bounds)?;
    O.checked_mul(record)
        .and_then(|n| n.checked_add(catalog))
        .and_then(|n| n.checked_add(16))
        .ok_or(Error::Bounds)
}

pub(crate) fn encode_envelope<const I: usize, const O: usize, const S: usize>(
    catalog: &Catalog<I, O, S>,
    evidence: &Evidence<S>,
    maximum: usize,
) -> Result<Vec<u8>> {
    evidence.validate(catalog)?;
    let catalog_bound = prns_identities_codec::max_encoded_len::<I, O, S>().ok_or(Error::Bounds)?;
    let mut catalog_bytes = vec![0; catalog_bound];
    let catalog_length = prns_identities_codec::encode(catalog, &mut catalog_bytes)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(maximum)
        .map_err(|_| Error::Bounds)?;
    bytes.extend_from_slice(
        &u32::try_from(catalog_length)
            .map_err(|_| Error::Bounds)?
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&catalog_bytes[..catalog_length]);
    bytes.extend_from_slice(&evidence.epoch.to_be_bytes());
    bytes.extend_from_slice(
        &u32::try_from(evidence.records.len())
            .map_err(|_| Error::Bounds)?
            .to_be_bytes(),
    );
    let mut intent =
        vec![0; prns_identities_codec::max_create_request_encoded_len::<S>().ok_or(Error::Bounds)?];
    for record in &evidence.records {
        bytes.extend_from_slice(&record.request.scope.epoch.to_be_bytes());
        bytes.extend_from_slice(&record.request.nonce);
        bytes.extend_from_slice(&record.token.sequence.to_be_bytes());
        let used = prns_identities_codec::encode_create_request(&record.intent, &mut intent)?;
        bytes.extend_from_slice(
            &u32::try_from(used)
                .map_err(|_| Error::Bounds)?
                .to_be_bytes(),
        );
        bytes.extend_from_slice(&intent[..used]);
        let mut result = [0; 64];
        let used = match record.result {
            Some(value) => prns_identities_codec::encode_operation_result(&value, &mut result)?,
            None => 0,
        };
        bytes.push(u8::try_from(used).map_err(|_| Error::Bounds)?);
        bytes.extend_from_slice(&result[..used]);
    }
    if bytes.len() > maximum {
        return Err(Error::Bounds);
    }
    Ok(bytes)
}

pub(crate) fn decode_envelope<const I: usize, const O: usize, const S: usize>(
    bytes: &[u8],
    into: &mut Catalog<I, O, S>,
) -> Result<Evidence<S>> {
    let mut reader = Reader(bytes);
    let catalog_length = usize::try_from(reader.u32()?).map_err(|_| Error::Bounds)?;
    prns_identities_codec::decode(reader.take(catalog_length)?, into)?;
    let epoch = reader.u64()?;
    let count = usize::try_from(reader.u32()?).map_err(|_| Error::Bounds)?;
    if count > O {
        return Err(Error::Bounds);
    }
    let mut records = Vec::new();
    records
        .try_reserve_exact(count)
        .map_err(|_| Error::Bounds)?;
    for _ in 0..count {
        let request = CreateRequestId {
            scope: CreateRequestScope {
                incarnation: into.incarnation,
                epoch: reader.u64()?,
            },
            nonce: reader.take(32)?.try_into().map_err(|_| Error::Corrupt)?,
        };
        let token = OperationToken {
            incarnation: into.incarnation,
            sequence: reader.u64()?,
            digest: request_digest(request),
        };
        let length = usize::try_from(reader.u32()?).map_err(|_| Error::Bounds)?;
        let intent = prns_identities_codec::decode_create_request(reader.take(length)?)?;
        let length = usize::from(reader.take(1)?[0]);
        let result = if length == 0 {
            None
        } else {
            Some(prns_identities_codec::decode_operation_result(
                reader.take(length)?,
            )?)
        };
        records.push(Record {
            request,
            intent,
            token,
            result,
        });
    }
    if !reader.0.is_empty() {
        return Err(Error::Corrupt);
    }
    let evidence = Evidence { epoch, records };
    evidence.validate(into)?;
    Ok(evidence)
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.0.len() {
            return Err(Error::Corrupt);
        }
        let (value, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().map_err(|_| Error::Corrupt)?,
        ))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| Error::Corrupt)?,
        ))
    }
}

#[cfg(test)]
mod adapter_validation_tests {
    use super::*;
    #[test]
    #[allow(clippy::unwrap_used)]
    fn fresh_adapter_validation_cannot_invalidate_an_exact_retained_creation() {
        type Disk = IdentityStore<2, 4, 1>;
        let root = std::env::temp_dir().join(format!(
            "prns-creation-policy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut disk = Disk::create_new(&root).unwrap();
        {
            let mut working = Catalog::<2, 4, 1>::new([0; 16]);
            let mut staging = working.clone();
            let mut coordinator = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
            let request = CreateRequestId {
                scope: disk.creation_scope().unwrap(),
                nonce: [1; 32],
            };
            let intent = CreateRequest {
                source: KeySource::Generate,
                metadata: prns_identities_core::Metadata::new("local", Some("old\u{202e}name"))
                    .unwrap(),
                services: prns_identities_core::ServiceSet::empty(),
            };
            assert!(matches!(
                disk.create_or_recover_validated(&mut coordinator, request, intent, None, |_| Err(
                    Fault::InvalidMetadata
                )),
                Err(CreateError::Authority(Fault::InvalidMetadata))
            ));
            // The rejected admission left no substituted receipt or key behind.
            let created = disk
                .create_or_recover(&mut coordinator, request, intent, None)
                .unwrap();
            let recovered = disk
                .create_or_recover_validated(&mut coordinator, request, intent, None, |_| {
                    Err(Fault::InvalidMetadata)
                })
                .unwrap();
            assert_eq!(recovered, created);
        }
        drop(disk);
        std::fs::remove_dir_all(root).unwrap();
    }
}
