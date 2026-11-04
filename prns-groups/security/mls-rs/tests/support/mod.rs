pub mod durable;

use durable::{Directory, Faults, FileBackend};
use prns_groups_mls::{Adapter, Limits, LocalIdentity, Policy};
use prns_groups_store::{Coordinator, GroupId, Limits as StoreLimits, OperationId};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
pub const GROUP: GroupId = GroupId([7; 32]);
pub const NOW: u64 = 1_792_000_000;

pub fn limits() -> Limits {
    Limits {
        max_members: 16,
        max_groups: 2,
        max_wire_bytes: 65_536,
        max_plaintext_bytes: 4_096,
        max_snapshot_bytes: 65_536,
        max_key_packages: 8,
        max_retained_epochs: 2,
        max_pending_proposals: 16,
        max_pending_outputs: 64,
    }
}

pub fn store_limits() -> StoreLimits {
    StoreLimits {
        max_changes: 128,
        max_key_bytes: 256,
        max_value_bytes: 1_048_576,
        max_staged_bytes: 4_194_304,
    }
}

pub struct Endpoint {
    pub directory: Directory,
    pub identity: LocalIdentity,
    pub policy: Policy,
    pub limits: Limits,
    pub faults: Faults,
    adapter: Option<Adapter<FileBackend>>,
}

impl Endpoint {
    pub fn new(identity: LocalIdentity, policy: Policy, limits: Limits) -> TestResult<Self> {
        let directory = Directory::new()?;
        let backend = FileBackend::open(directory.path())?;
        let faults = backend.faults();
        let coordinator = Coordinator::new(backend, store_limits())?;
        let adapter = Adapter::new(coordinator, identity.clone(), policy.clone(), limits)?;
        Ok(Self {
            directory,
            identity,
            policy,
            limits,
            faults,
            adapter: Some(adapter),
        })
    }

    pub fn adapter(&self) -> &Adapter<FileBackend> {
        self.adapter.as_ref().unwrap()
    }

    pub fn reopen(&mut self) -> TestResult {
        drop(self.adapter.take());
        let backend = FileBackend::open(self.directory.path())?;
        self.faults = backend.faults();
        let coordinator = Coordinator::new(backend, store_limits())?;
        self.adapter = Some(Adapter::new(
            coordinator,
            self.identity.clone(),
            self.policy.clone(),
            self.limits,
        )?);
        Ok(())
    }

    pub fn bytes(&self) -> TestResult<Vec<u8>> {
        Ok(FileBackend::open(self.directory.path())?.published_bytes()?)
    }
}

pub struct Enrollment {
    pub owner: Endpoint,
    pub member: Endpoint,
    pub welcome: Vec<u8>,
}

impl Enrollment {
    pub fn new(member_limits: Limits) -> TestResult<Self> {
        let owner_identity = LocalIdentity::generate([1; 32], 1)?;
        let member_identity = LocalIdentity::generate([2; 32], 1)?;
        let policy = Policy::new(
            owner_identity.binding(),
            vec![owner_identity.binding(), member_identity.binding()],
        )?;
        let owner = Endpoint::new(owner_identity, policy.clone(), limits())?;
        let member = Endpoint::new(member_identity, policy, member_limits)?;
        owner
            .adapter()
            .create(OperationId([1; 32]), GROUP)
            .map_err(|error| format!("fixture owner create: {error}"))?;
        let package = member
            .adapter()
            .key_package(OperationId([1; 32]), NOW)
            .map_err(|error| format!("fixture member key package: {error}"))?;
        let added = owner
            .adapter()
            .add(
                OperationId([2; 32]),
                GROUP,
                &[&package.output.messages[0]],
                NOW,
            )
            .map_err(|error| format!("fixture owner add: {error}"))?;
        Ok(Self {
            owner,
            member,
            welcome: added.output.messages[1].clone(),
        })
    }

    pub fn joined(member_limits: Limits) -> TestResult<Self> {
        let peers = Self::new(member_limits)?;
        peers
            .member
            .adapter()
            .join(OperationId([2; 32]), GROUP, &peers.welcome, NOW)
            .map_err(|error| format!("fixture member join: {error}"))?;
        Ok(peers)
    }
}

pub struct Trio {
    pub owner: Endpoint,
    pub member: Endpoint,
    pub other: Endpoint,
}

impl Trio {
    pub fn joined(member_limits: Limits) -> TestResult<Self> {
        let owner_identity = LocalIdentity::generate([1; 32], 1)?;
        let member_identity = LocalIdentity::generate([2; 32], 1)?;
        let other_identity = LocalIdentity::generate([3; 32], 1)?;
        let policy = Policy::new(
            owner_identity.binding(),
            vec![
                owner_identity.binding(),
                member_identity.binding(),
                other_identity.binding(),
            ],
        )?;
        let owner = Endpoint::new(owner_identity, policy.clone(), limits())?;
        let other = Endpoint::new(other_identity, policy.clone(), limits())?;
        let member = Endpoint::new(member_identity, policy, member_limits)?;
        owner
            .adapter()
            .create(OperationId([1; 32]), GROUP)
            .map_err(|error| format!("fixture owner create: {error}"))?;
        let member_package = member.adapter().key_package(OperationId([1; 32]), NOW)?;
        let other_package = other.adapter().key_package(OperationId([1; 32]), NOW)?;
        let added = owner.adapter().add(
            OperationId([2; 32]),
            GROUP,
            &[
                &member_package.output.messages[0],
                &other_package.output.messages[0],
            ],
            NOW,
        )?;
        member
            .adapter()
            .join(OperationId([2; 32]), GROUP, &added.output.messages[1], NOW)?;
        other
            .adapter()
            .join(OperationId([2; 32]), GROUP, &added.output.messages[1], NOW)?;
        Ok(Self {
            owner,
            member,
            other,
        })
    }
}
