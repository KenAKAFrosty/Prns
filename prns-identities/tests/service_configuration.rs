#![allow(clippy::unwrap_used, clippy::panic, clippy::drop_non_drop)]
#![cfg(unix)]

use prns_core::identity::PrivateIdentityMaterial;
use prns_identities::creation::CreateRequestId;
use prns_identities::{Error as IdentityError, IdentityStore};
use prns_identities_core::*;
use prns_identities_runtime::configuration::{initialize_service, ServiceInitializer};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

type Disk = IdentityStore<2, 16, 2>;
type State = Catalog<2, 16, 2>;
const TELEMETRY: ServiceId = ServiceId(0x7710);
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "prns-service-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!root.exists());
        drop(Disk::create_new(&root).unwrap());
        Self(root)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn intent() -> CreateRequest<2> {
    CreateRequest {
        source: KeySource::Generate,
        metadata: Metadata::new("Sensor node", None).unwrap(),
        services: ServiceSet::empty(),
    }
}
fn report() -> DependencyReport<2> {
    DependencyReport::new(&[ServiceDependency::clear(TELEMETRY, 1)], 0).unwrap()
}

#[derive(Debug, PartialEq, Eq)]
enum DomainError {
    Unavailable,
    UnsupportedState,
    MissingState,
    Storage,
    LostReply,
}
struct Telemetry {
    path: PathBuf,
    available: bool,
    lose_reply: bool,
    writes: usize,
}
impl Telemetry {
    fn validate_existing(&self) -> Result<(), DomainError> {
        let bytes = fs::read(&self.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                DomainError::MissingState
            } else {
                DomainError::Storage
            }
        })?;
        if !bytes.starts_with(b"SENSOR01") {
            return Err(DomainError::UnsupportedState);
        }
        Ok(())
    }
}
impl ServiceInitializer for Telemetry {
    type Error = DomainError;
    fn initialize_service(
        &mut self,
        ticket: EffectTicket,
        identity: IdentityGuard,
        key: KeyReference,
        service: ServiceId,
    ) -> Result<InitializedService, DomainError> {
        if !self.available || service != TELEMETRY {
            return Err(DomainError::Unavailable);
        }
        let operation = ticket.operation.ok_or(DomainError::UnsupportedState)?;
        let mut expected = b"SENSOR01".to_vec();
        expected.extend_from_slice(&identity.id.incarnation);
        expected.extend_from_slice(&identity.id.sequence.to_be_bytes());
        expected.extend_from_slice(&identity.public_hash.0);
        expected.extend_from_slice(&key.0);
        expected.extend_from_slice(&service.0.to_be_bytes());
        expected.extend_from_slice(&operation.sequence.to_be_bytes());
        expected.extend_from_slice(&operation.digest.0);
        if self.path.exists() {
            if fs::read(&self.path).map_err(|_| DomainError::Storage)? != expected {
                return Err(DomainError::UnsupportedState);
            }
        } else {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&self.path)
                .map_err(|_| DomainError::Storage)?;
            file.write_all(&expected)
                .map_err(|_| DomainError::Storage)?;
            file.sync_all().map_err(|_| DomainError::Storage)?;
            fs::File::open(self.path.parent().unwrap())
                .and_then(|parent| parent.sync_all())
                .map_err(|_| DomainError::Storage)?;
            self.writes += 1;
        }
        if self.lose_reply {
            return Err(DomainError::LostReply);
        }
        Ok(InitializedService { ticket, service })
    }
}

#[test]
fn physical_domain_commit_recovers_without_adapter_defaults_and_keeps_creation_result() {
    let root = Root::new();
    let mut disk = Disk::open(root.path()).unwrap();
    let request = CreateRequestId {
        scope: disk.creation_scope().unwrap(),
        nonce: [12; 32],
    };
    let mut work = State::new([0; 16]);
    let mut stage = work.clone();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    let creation = disk
        .create_or_recover(&mut core, request, intent(), None)
        .unwrap();
    let OperationResult::Created { identity } = creation.result else {
        panic!()
    };
    let prepared = core
        .prepare(
            identity,
            TransitionTarget::AddService(TELEMETRY),
            DependencyReport::empty(),
        )
        .unwrap();
    let command = Command::Transition(prepared);
    let token = core
        .reserve(&mut disk, command, CommandDigest([13; 32]))
        .unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let quiesce = core.action(token).unwrap().unwrap().ticket();
    assert!(matches!(
        disk.initialization_key(&core, quiesce),
        Err(IdentityError::WrongAction)
    ));
    core.ack_configuration_quiesced(&mut disk, quiesce, DependencyReport::empty())
        .unwrap();
    let action = core.action(token).unwrap().unwrap();
    let mut domain = Telemetry {
        path: root.path().join("telemetry.state"),
        available: true,
        lose_reply: true,
        writes: 0,
    };
    assert_eq!(
        initialize_service(&disk.runtime_authority(&core), &mut domain, action),
        Err(prns_identities_runtime::Error::Driver(
            DomainError::LostReply
        ))
    );
    let persisted = fs::read(&domain.path).unwrap();
    assert_eq!(domain.writes, 1);
    drop(core);
    drop(disk);

    let mut disk = Disk::open(root.path()).unwrap();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    let recovered = core.recovery_action(identity).unwrap().unwrap();
    assert_ne!(recovered.ticket().session, action.ticket().session);
    domain.available = false;
    domain.lose_reply = false;
    assert_eq!(
        initialize_service(&disk.runtime_authority(&core), &mut domain, recovered),
        Err(prns_identities_runtime::Error::Driver(
            DomainError::Unavailable
        ))
    );
    assert_eq!(
        core.identity(identity).unwrap().services,
        ServiceSet::empty()
    );
    assert!(core.identity(identity).unwrap().pending.is_some());
    assert_eq!(
        core.prepare(
            identity,
            TransitionTarget::Retire,
            DependencyReport::empty()
        ),
        Err(Fault::Busy)
    );
    assert!(fs::read(&domain.path).unwrap() == persisted);

    domain.available = true;
    let proof = initialize_service(&disk.runtime_authority(&core), &mut domain, recovered).unwrap();
    let secret = disk.initialization_key(&core, recovered.ticket()).unwrap();
    assert_eq!(
        PrivateIdentityMaterial::from(secret)
            .identity_hash()
            .as_bytes(),
        &core.identity(identity).unwrap().public_hash.0
    );
    core.ack_service_initialized(&mut disk, proof).unwrap();
    assert_eq!(domain.writes, 1);
    assert_eq!(
        core.identity(identity).unwrap().desired,
        DesiredState::Paused
    );
    assert_eq!(
        disk.create_or_recover(&mut core, request, intent(), None)
            .unwrap(),
        creation
    );
    assert!(disk.initialization_key(&core, recovered.ticket()).is_err());
    drop(core);
    drop(disk);

    let mut disk = Disk::open(root.path()).unwrap();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    assert_eq!(
        core.identity(identity).unwrap().services,
        ServiceSet::new(&[TELEMETRY]).unwrap()
    );
    assert_eq!(
        core.prepare(
            identity,
            TransitionTarget::Retire,
            DependencyReport::empty()
        ),
        Err(Fault::InvalidReport)
    );
    let prepared = core
        .prepare(
            identity,
            TransitionTarget::RemoveService(TELEMETRY),
            report(),
        )
        .unwrap();
    let command = Command::Transition(prepared);
    let remove = core
        .reserve(&mut disk, command, CommandDigest([14; 32]))
        .unwrap();
    core.begin(&mut disk, remove, &command).unwrap();
    let ticket = core.action(remove).unwrap().unwrap().ticket();
    core.ack_configuration_quiesced(&mut disk, ticket, report())
        .unwrap();
    assert_eq!(
        core.identity(identity).unwrap().services,
        ServiceSet::empty()
    );
    assert_eq!(
        disk.create_or_recover(&mut core, request, intent(), None)
            .unwrap(),
        creation
    );
    // Configuration removal does not delete domain history or its stored binding.
    assert!(fs::read(&domain.path).unwrap() == persisted);
}

#[test]
fn explicit_preparation_never_replaces_unsupported_state_and_reopen_is_validation_only() {
    let root = Root::new();
    let mut disk = Disk::open(root.path()).unwrap();
    let request = CreateRequestId {
        scope: disk.creation_scope().unwrap(),
        nonce: [22; 32],
    };
    let mut work = State::new([0; 16]);
    let mut stage = work.clone();
    let mut core = Coordinator::open(&mut disk, &mut work, &mut stage).unwrap();
    let result = disk
        .create_or_recover(&mut core, request, intent(), None)
        .unwrap();
    let OperationResult::Created { identity } = result.result else {
        panic!()
    };
    let prepared = core
        .prepare(
            identity,
            TransitionTarget::AddService(TELEMETRY),
            DependencyReport::empty(),
        )
        .unwrap();
    let command = Command::Transition(prepared);
    let token = core
        .reserve(&mut disk, command, CommandDigest([23; 32]))
        .unwrap();
    core.begin(&mut disk, token, &command).unwrap();
    let ticket = core.action(token).unwrap().unwrap().ticket();
    core.ack_configuration_quiesced(&mut disk, ticket, DependencyReport::empty())
        .unwrap();
    let action = core.action(token).unwrap().unwrap();
    let mut domain = Telemetry {
        path: root.path().join("future.state"),
        available: true,
        lose_reply: false,
        writes: 0,
    };
    assert_eq!(domain.validate_existing(), Err(DomainError::MissingState));
    assert!(!domain.path.exists());
    fs::write(&domain.path, b"SENSOR99future-obligations").unwrap();
    assert_eq!(
        initialize_service(&disk.runtime_authority(&core), &mut domain, action),
        Err(prns_identities_runtime::Error::Driver(
            DomainError::UnsupportedState
        ))
    );
    assert_eq!(
        fs::read(&domain.path).unwrap(),
        b"SENSOR99future-obligations"
    );
    assert_eq!(domain.writes, 0);
    assert!(core.identity(identity).unwrap().pending.is_some());
}
