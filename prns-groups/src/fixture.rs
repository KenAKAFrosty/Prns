//! Explicit, private development enrollment artifacts. No shared baked-in keys.
//! Each artifact belongs to one Reticulum identity and one device incarnation.
use crate::Provisioned;
use prns_groups_core::{
    AuthorizationEra, ControlId, ControlReference, Endpoint, EndpointId, EndpointIncarnation,
    GroupId, OperationId, OperationTicket, StoreIncarnation, Timestamp,
};
use prns_groups_mls::{Adapter, LocalIdentity, MemberBinding, Policy};
use prns_groups_reticulum::{destination_for, Peer};
use prns_groups_store::{AtomicBackend, CommitOutcome, Coordinator, Mutation};
use prns_groups_store_redb::{
    EnrollmentSnapshot, RedbBackend, RedbError, RedbStoreOwner, StoreNamespace,
};
use prns_host::IdentityHash;
use redb::Database;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_FIXTURE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MEMBERS: usize = 8;
const FORMAT: u32 = 1;
const SIGNING_KEYPAIR_BYTES: usize = 64;
const CONFIG_KEY: &[u8] = b"development/enrollment/v1";

#[derive(Debug)]
pub enum FixtureError {
    Invalid,
    TooLarge,
    IdentityMismatch,
    ConfigurationChanged,
    Json { line: usize, column: usize },
    Storage(RedbError),
    Enrollment,
    Entropy,
}
impl core::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "development enrollment: {self:?}")
    }
}
impl std::error::Error for FixtureError {}
impl From<RedbError> for FixtureError {
    fn from(error: RedbError) -> Self {
        Self::Storage(error)
    }
}
impl From<serde_json::Error> for FixtureError {
    fn from(error: serde_json::Error) -> Self {
        // Serde diagnostics can echo malformed input values, including keys.
        Self::Json {
            line: error.line(),
            column: error.column(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    endpoint: [u8; 32],
    incarnation: u64,
    signing_key: [u8; 32],
    reticulum_identity: [u8; 16],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format: u32,
    title: String,
    group: [u8; 32],
    control: [u8; 32],
    era: [u8; 32],
    namespace: [u8; 16],
    store_incarnation: [u8; 24],
    local_member: usize,
    members: Vec<Member>,
    signing_secret: Vec<u8>,
}
impl Drop for Configuration {
    fn drop(&mut self) {
        self.signing_secret.zeroize();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    metadata: Vec<u8>,
    values: Vec<(Vec<u8>, Vec<u8>)>,
    receipts: Vec<(u64, u64)>,
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        for (_, value) in &mut self.values {
            value.zeroize();
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    configuration: Configuration,
    snapshot: Snapshot,
}

/// Install once or open existing state. Never reimport the initial ratchets on
/// restart. The caller trusts this local artifact as explicit enrollment input.
pub fn load_and_install(
    database: &Database,
    bytes: &[u8],
    local_identity: IdentityHash,
) -> Result<Provisioned, FixtureError> {
    if bytes.len() > MAX_FIXTURE_BYTES {
        return Err(FixtureError::TooLarge);
    }
    let fixture: Fixture = serde_json::from_slice(bytes)?;
    let config = &fixture.configuration;
    validate(config, local_identity)?;
    let expected_hash = configuration_hash(config)?;
    let markers: Vec<_> = fixture
        .snapshot
        .values
        .iter()
        .filter(|(key, _)| key == CONFIG_KEY)
        .collect();
    if markers.len() != 1 || markers[0].1.as_slice() != expected_hash {
        return Err(FixtureError::ConfigurationChanged);
    }
    let incarnation =
        StoreIncarnation::new(config.store_incarnation).map_err(|_| FixtureError::Invalid)?;
    let namespace = StoreNamespace(config.namespace);
    let mut owner = RedbStoreOwner::new(database);
    match owner.open(namespace, incarnation, store_limits()) {
        Ok(mut backend) => {
            if backend.read(CONFIG_KEY, 32)?.as_deref() != Some(expected_hash.as_slice()) {
                return Err(FixtureError::ConfigurationChanged);
            }
        }
        Err(RedbError::MissingNamespace) => {
            let mut snapshot = EnrollmentSnapshot {
                metadata: fixture.snapshot.metadata.clone(),
                values: fixture.snapshot.values.clone(),
                receipts: fixture.snapshot.receipts.clone(),
            };
            let result = owner.import_enrollment(namespace, incarnation, store_limits(), &snapshot);
            for (_, value) in &mut snapshot.values {
                value.zeroize();
            }
            result?;
        }
        Err(error) => return Err(error.into()),
    }
    provisioned(config)
}

fn validate(config: &Configuration, local_identity: IdentityHash) -> Result<(), FixtureError> {
    if config.format != FORMAT
        || config.members.len() < 2
        || config.members.len() > MAX_MEMBERS
        || config.title.is_empty()
        || config.title.len() > crate::profile::MAX_TITLE_BYTES
        || config.signing_secret.len() != SIGNING_KEYPAIR_BYTES
    {
        return Err(FixtureError::Invalid);
    }
    let local = config
        .members
        .get(config.local_member)
        .ok_or(FixtureError::Invalid)?;
    if local.reticulum_identity != local_identity.into_bytes() {
        return Err(FixtureError::IdentityMismatch);
    }
    for (index, member) in config.members.iter().enumerate() {
        if member.incarnation == 0
            || config.members[..index].iter().any(|previous| {
                previous.endpoint == member.endpoint
                    || previous.reticulum_identity == member.reticulum_identity
                    || previous.signing_key == member.signing_key
            })
        {
            return Err(FixtureError::Invalid);
        }
    }
    // Validate the private/public key relationship before any database mutation.
    LocalIdentity::from_parts(binding(local), config.signing_secret.clone().into())
        .map_err(|_| FixtureError::Invalid)?;
    Ok(())
}
fn binding(member: &Member) -> MemberBinding {
    MemberBinding {
        endpoint: member.endpoint,
        incarnation: member.incarnation,
        signing_key: member.signing_key,
    }
}
fn endpoint(member: &Member) -> Result<Endpoint, FixtureError> {
    Ok(Endpoint {
        id: EndpointId(member.endpoint),
        incarnation: EndpointIncarnation::new(member.incarnation)
            .map_err(|_| FixtureError::Invalid)?,
    })
}
fn provisioned(config: &Configuration) -> Result<Provisioned, FixtureError> {
    let local = config
        .members
        .get(config.local_member)
        .ok_or(FixtureError::Invalid)?;
    let bindings: Vec<_> = config.members.iter().map(binding).collect();
    let mut readers = config
        .members
        .iter()
        .map(endpoint)
        .collect::<Result<Vec<_>, _>>()?;
    readers.sort();
    let peers = config
        .members
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != config.local_member)
        .map(|(_, member)| {
            let identity = IdentityHash::new(member.reticulum_identity);
            Ok(Peer {
                endpoint: endpoint(member)?,
                identity,
                destination: destination_for(identity).map_err(|_| FixtureError::Invalid)?,
            })
        })
        .collect::<Result<Vec<_>, FixtureError>>()?;
    Ok(Provisioned {
        lifecycle: crate::GroupLifecycle::Active,
        history_generation: 0,
        history_peers: peers.clone(),
        owner_identity: None,
        cleanup_reserve: None,
        group: GroupId(config.group),
        control: ControlReference {
            head: ControlId(config.control),
            era: AuthorizationEra(ControlId(config.era)),
        },
        readers,
        identity: LocalIdentity::from_parts(binding(local), config.signing_secret.clone().into())
            .map_err(|_| FixtureError::Invalid)?,
        policy: Policy::new(bindings[0].clone(), bindings).map_err(|_| FixtureError::Invalid)?,
        namespace: StoreNamespace(config.namespace),
        incarnation: StoreIncarnation::new(config.store_incarnation)
            .map_err(|_| FixtureError::Invalid)?,
        store_limits: store_limits(),
        mls_limits: mls_limits(),
        service_limits: service_limits(),
        title: config.title.clone(),
        peers,
        local_reticulum_identity: IdentityHash::new(local.reticulum_identity),
    })
}
fn configuration_hash(config: &Configuration) -> Result<[u8; 32], FixtureError> {
    let bytes = Zeroizing::new(serde_json::to_vec(config)?);
    Ok(Sha256::digest(&bytes).into())
}
fn random<const N: usize>() -> Result<[u8; N], FixtureError> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|_| FixtureError::Entropy)?;
    Ok(bytes)
}

/// Pre-enroll independent identities, returning one private fixture per input
/// identity in input order. The first member is the MLS owner. Transfer fixtures
/// privately and install each on exactly one device; cloning them forks ratchets.
pub fn generate(
    title: &str,
    identities: &[IdentityHash],
    now: Timestamp,
) -> Result<Vec<Vec<u8>>, FixtureError> {
    if !(2..=MAX_MEMBERS).contains(&identities.len())
        || title.is_empty()
        || title.len() > crate::profile::MAX_TITLE_BYTES
        || identities
            .iter()
            .enumerate()
            .any(|(i, id)| identities[..i].contains(id))
    {
        return Err(FixtureError::Invalid);
    }
    let group = random()?;
    let control = random()?;
    let era = random()?;
    let namespace = random()?;
    let locals = identities
        .iter()
        .map(|_| LocalIdentity::generate(random()?, 1).map_err(|_| FixtureError::Enrollment))
        .collect::<Result<Vec<_>, FixtureError>>()?;
    let members: Vec<_> = locals
        .iter()
        .zip(identities)
        .map(|(local, identity)| {
            let binding = local.binding();
            Member {
                endpoint: binding.endpoint,
                incarnation: binding.incarnation,
                signing_key: binding.signing_key,
                reticulum_identity: identity.into_bytes(),
            }
        })
        .collect();
    let mut configs = Vec::new();
    let mut databases = Vec::new();
    for (index, identity) in locals.into_iter().enumerate() {
        let (_, secret) = identity.into_parts();
        let configuration = Configuration {
            format: FORMAT,
            title: title.into(),
            group,
            control,
            era,
            namespace,
            store_incarnation: random()?,
            local_member: index,
            members: members
                .iter()
                .map(|m| Member {
                    endpoint: m.endpoint,
                    incarnation: m.incarnation,
                    signing_key: m.signing_key,
                    reticulum_identity: m.reticulum_identity,
                })
                .collect(),
            signing_secret: secret.as_ref().to_vec(),
        };
        let database = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(|error| FixtureError::Storage(RedbError::Database(error.into())))?;
        let mut owner = RedbStoreOwner::new(&database);
        owner.initialize(
            StoreNamespace(namespace),
            StoreIncarnation::new(configuration.store_incarnation)
                .map_err(|_| FixtureError::Invalid)?,
            store_limits(),
        )?;
        configs.push(configuration);
        databases.push(database);
    }
    with_adapter(&databases[0], &configs[0], |adapter| {
        adapter
            .create(next(adapter, &configs[0])?, GroupId(group))
            .map_err(|_| FixtureError::Enrollment)?;
        Ok(())
    })?;
    let mut packages = Vec::new();
    for index in 1..configs.len() {
        packages.push(with_adapter(
            &databases[index],
            &configs[index],
            |adapter| {
                let output = adapter
                    .key_package(next(adapter, &configs[index])?, now.0)
                    .map_err(|_| FixtureError::Enrollment)?;
                output
                    .output
                    .messages
                    .first()
                    .cloned()
                    .ok_or(FixtureError::Enrollment)
            },
        )?);
    }
    let welcome = with_adapter(&databases[0], &configs[0], |adapter| {
        let packages: Vec<&[u8]> = packages.iter().map(Vec::as_slice).collect();
        let output = adapter
            .add(
                next(adapter, &configs[0])?,
                GroupId(group),
                &packages,
                now.0,
            )
            .map_err(|_| FixtureError::Enrollment)?;
        output
            .output
            .messages
            .get(1)
            .cloned()
            .ok_or(FixtureError::Enrollment)
    })?;
    for index in 1..configs.len() {
        with_adapter(&databases[index], &configs[index], |adapter| {
            adapter
                .join(
                    next(adapter, &configs[index])?,
                    GroupId(group),
                    &welcome,
                    now.0,
                )
                .map_err(|_| FixtureError::Enrollment)?;
            Ok(())
        })?;
    }
    let mut results = Vec::new();
    for (database, configuration) in databases.iter().zip(configs) {
        let mut owner = RedbStoreOwner::new(database);
        let incarnation = StoreIncarnation::new(configuration.store_incarnation)
            .map_err(|_| FixtureError::Invalid)?;
        let mut backend = owner.open(StoreNamespace(namespace), incarnation, store_limits())?;
        let head = backend.head()?;
        let operation = OperationTicket::new(
            incarnation,
            head.0.checked_add(1).ok_or(FixtureError::Enrollment)?,
        )
        .map_err(|_| FixtureError::Enrollment)?;
        let hash = configuration_hash(&configuration)?;
        match backend.commit(
            head,
            operation.operation_id(),
            &[Mutation::Put {
                key: CONFIG_KEY.to_vec(),
                value: hash.to_vec(),
            }],
        )? {
            CommitOutcome::Committed(_) => {}
            _ => return Err(FixtureError::Enrollment),
        }
        let snapshot = backend.export_enrollment()?;
        let fixture = Fixture {
            configuration,
            snapshot: Snapshot {
                metadata: snapshot.metadata,
                values: snapshot.values,
                receipts: snapshot.receipts,
            },
        };
        let bytes = serde_json::to_vec(&fixture)?;
        if bytes.len() > MAX_FIXTURE_BYTES {
            return Err(FixtureError::TooLarge);
        }
        results.push(bytes);
    }
    Ok(results)
}
fn with_adapter<T>(
    database: &Database,
    config: &Configuration,
    action: impl FnOnce(&Adapter<RedbBackend<'_>>) -> Result<T, FixtureError>,
) -> Result<T, FixtureError> {
    let provisioned = provisioned(config)?;
    let mut owner = RedbStoreOwner::new(database);
    let backend = owner.open(
        provisioned.namespace,
        provisioned.incarnation,
        provisioned.store_limits,
    )?;
    let limits = store_limits();
    let coordinator = Coordinator::new(backend, limits.transaction_limits()?)
        .map_err(|_| FixtureError::Enrollment)?;
    let adapter = Adapter::new(
        coordinator,
        provisioned.identity,
        provisioned.policy,
        provisioned.mls_limits,
    )
    .map_err(|_| FixtureError::Enrollment)?;
    action(&adapter)
}
fn next(
    adapter: &Adapter<RedbBackend<'_>>,
    config: &Configuration,
) -> Result<OperationId, FixtureError> {
    let head = adapter
        .coordinator()
        .head()
        .map_err(|_| FixtureError::Enrollment)?;
    Ok(OperationTicket::new(
        StoreIncarnation::new(config.store_incarnation).map_err(|_| FixtureError::Invalid)?,
        head.0.checked_add(1).ok_or(FixtureError::Enrollment)?,
    )
    .map_err(|_| FixtureError::Enrollment)?
    .operation_id())
}
pub use crate::profile::{mls_limits, service_limits, store_limits};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn independent_enrollments_import_once_and_reopen_without_ratchet_rollback() {
        let identities = [IdentityHash::new([1; 16]), IdentityHash::new([2; 16])];
        let fixtures =
            generate("Development group", &identities, Timestamp(1_792_000_000)).unwrap();
        let left: Fixture = serde_json::from_slice(&fixtures[0]).unwrap();
        let right: Fixture = serde_json::from_slice(&fixtures[1]).unwrap();
        assert_ne!(
            left.configuration.signing_secret,
            right.configuration.signing_secret
        );
        assert_ne!(
            left.configuration.store_incarnation,
            right.configuration.store_incarnation
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("application.redb");
        let database = Database::create(&path).unwrap();
        let initial = load_and_install(&database, &fixtures[0], identities[0]).unwrap();
        {
            let mut owner = RedbStoreOwner::new(&database);
            let mut backend = owner
                .open(initial.namespace, initial.incarnation, initial.store_limits)
                .unwrap();
            let head = backend.head().unwrap();
            let ticket = OperationTicket::new(initial.incarnation, head.0 + 1).unwrap();
            assert!(matches!(
                backend
                    .commit(
                        head,
                        ticket.operation_id(),
                        &[Mutation::Put {
                            key: b"after-import".to_vec(),
                            value: b"retained".to_vec()
                        }]
                    )
                    .unwrap(),
                CommitOutcome::Committed(_)
            ));
        }
        drop(database);
        let database = Database::open(&path).unwrap();
        let resumed = load_and_install(&database, &fixtures[0], identities[0]).unwrap();
        {
            let mut owner = RedbStoreOwner::new(&database);
            let mut backend = owner
                .open(resumed.namespace, resumed.incarnation, resumed.store_limits)
                .unwrap();
            assert_eq!(
                backend.read(b"after-import", 100).unwrap().unwrap(),
                b"retained"
            );
        }
        assert!(matches!(
            load_and_install(&database, &fixtures[0], identities[1]),
            Err(FixtureError::IdentityMismatch)
        ));
        let mut changed: Fixture = serde_json::from_slice(&fixtures[0]).unwrap();
        changed.configuration.title = "Changed".into();
        assert!(matches!(
            load_and_install(
                &database,
                &serde_json::to_vec(&changed).unwrap(),
                identities[0]
            ),
            Err(FixtureError::ConfigurationChanged)
        ));
    }

    #[test]
    fn malformed_fixture_fails_before_installing_tables() {
        let identities = [IdentityHash::new([3; 16]), IdentityHash::new([4; 16])];
        let fixtures =
            generate("Development group", &identities, Timestamp(1_792_000_000)).unwrap();
        let mut fixture: Fixture = serde_json::from_slice(&fixtures[0]).unwrap();
        fixture.configuration.members[1].reticulum_identity = identities[0].into_bytes();
        let database = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .unwrap();
        assert!(matches!(
            load_and_install(
                &database,
                &serde_json::to_vec(&fixture).unwrap(),
                identities[0]
            ),
            Err(FixtureError::Invalid)
        ));
        // The original fixture can still perform first enrollment.
        let provisioned = load_and_install(&database, &fixtures[0], identities[0]).unwrap();
        crate::StoreSession::open(&database, provisioned).unwrap();
    }
}
