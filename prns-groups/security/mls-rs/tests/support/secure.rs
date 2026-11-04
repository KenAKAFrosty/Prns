//! Real MLS over authenticated NOR storage. Reopen retains only the NOR image,
//! hardware-KEK analogue and public provisioning; MLS and identity state reload
//! from encrypted objects. Host OS entropy is never reset or replayed on reboot.

#[path = "secure_nor.rs"]
pub mod nor;

use core::convert::Infallible;
use prns_groups_mls::{Adapter, Limits, LocalIdentity, MemberBinding, Policy};
use prns_groups_store::flash::Partition;
use prns_groups_store::secure::{
    Entropy, KeyProvider, SecureBackend, SecureLimits, SecureRequest, SecureUsage,
};
use prns_groups_store::{
    AtomicBackend, CommitOutcome, Coordinator, GroupId, Limits as StoreLimits, MaintenanceOutcome,
    Mutation, OperationId, OperationTicket, Revision, StoreIncarnation,
};
use zeroize::Zeroizing;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
pub type Backend = SecureBackend<nor::Flash, HardwareKey, HostEntropy>;
pub const GROUP: GroupId = GroupId([0x72; 32]);
pub const NOW: u64 = 1_791_763_200;
pub const CAPSULES: usize = 3;
pub const PARTITION: Partition = Partition {
    offset: nor::ERASE as u32,
    length: ((CAPSULES + 64) * nor::ERASE) as u32,
};
const IDENTITY: &[u8] = b"fixture/identity";

pub fn incarnation() -> StoreIncarnation {
    StoreIncarnation::new([0x59; 24]).unwrap()
}
pub fn ticket(sequence: u64) -> OperationTicket {
    OperationTicket::new(incarnation(), sequence).unwrap()
}
pub fn operation(sequence: u64) -> OperationId {
    ticket(sequence).into()
}

pub struct HardwareKey(Zeroizing<[u8; 32]>);
impl KeyProvider for HardwareKey {
    type Error = Infallible;
    fn wrapping_key(&mut self, out: &mut [u8; 32]) -> Result<(), Self::Error> {
        out.copy_from_slice(self.0.as_ref());
        Ok(())
    }
}
pub struct HostEntropy;
impl Entropy for HostEntropy {
    type Error = std::io::Error;
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::fill(out).map_err(|error| std::io::Error::other(error.to_string()))
    }
}

fn storage_limits() -> SecureLimits {
    SecureLimits {
        capsule_sectors: CAPSULES,
        max_live_objects: 128,
        max_catalog_bytes: 32 * 1024,
        max_key_bytes: 64,
        max_value_bytes: 32 * 1024,
        max_changes: 48,
        max_receipts: 32,
        reserve_bytes: 36 * 1024,
    }
}
fn coordinator_limits() -> StoreLimits {
    StoreLimits {
        max_changes: 48,
        max_key_bytes: 64,
        max_value_bytes: 32 * 1024,
        max_staged_bytes: 60 * 1024,
    }
}
fn mls_limits() -> Limits {
    Limits {
        max_members: 4,
        max_groups: 1,
        max_wire_bytes: 16 * 1024,
        max_plaintext_bytes: 1024,
        max_snapshot_bytes: 32 * 1024,
        max_key_packages: 2,
        max_retained_epochs: 1,
        max_pending_proposals: 4,
        max_pending_outputs: 24,
    }
}

pub struct Endpoint {
    pub controller: nor::Controller,
    wrapping_key: Zeroizing<[u8; 32]>,
    policy: Policy,
    adapter: Option<Adapter<Backend>>,
    pub last_mount: SecureUsage,
}
impl Endpoint {
    pub fn new(identity: LocalIdentity, policy: Policy) -> TestResult<Self> {
        let controller = nor::Controller::erased((PARTITION.offset + PARTITION.length) as usize);
        let mut wrapping_key = Zeroizing::new([0; 32]);
        getrandom::fill(wrapping_key.as_mut())
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let mut backend = SecureBackend::format(
            controller.flash(),
            PARTITION,
            storage_limits(),
            incarnation(),
            HardwareKey(wrapping_key.clone()),
            HostEntropy,
        )?;
        let (binding, secret) = identity.into_parts();
        let mut encoded = Zeroizing::new(Vec::new());
        encoded.extend_from_slice(&binding.endpoint);
        encoded.extend_from_slice(&binding.incarnation.to_be_bytes());
        encoded.extend_from_slice(&binding.signing_key);
        encoded.extend_from_slice(secret.as_ref());
        assert_eq!(encoded.len(), 136);
        assert!(matches!(
            backend.commit(
                Revision(0),
                operation(1),
                &[Mutation::Put {
                    key: IDENTITY.to_vec(),
                    value: encoded.to_vec()
                }],
            )?,
            CommitOutcome::Committed(_)
        ));
        let last_mount = backend.usage();
        drop(backend);
        let mut endpoint = Self {
            controller,
            wrapping_key,
            policy,
            adapter: None,
            last_mount,
        };
        endpoint.reopen()?;
        Ok(endpoint)
    }

    pub fn adapter(&self) -> &Adapter<Backend> {
        self.adapter.as_ref().unwrap()
    }

    fn mount(&self) -> TestResult<Backend> {
        Ok(SecureBackend::mount(
            self.controller.flash(),
            PARTITION,
            storage_limits(),
            incarnation(),
            HardwareKey(self.wrapping_key.clone()),
            HostEntropy,
        )?)
    }

    pub fn reopen(&mut self) -> TestResult {
        let bytes = self.controller.bytes();
        self.restart_from(bytes)
    }

    /// Used for a fault trial beginning at a known pre-operation image. This is
    /// explicitly a test snapshot, not a supported production rollback feature.
    pub fn restart_from(&mut self, bytes: Vec<u8>) -> TestResult {
        drop(self.adapter.take());
        self.controller = nor::Controller::from_bytes(bytes);
        let mut backend = self.mount()?;
        self.last_mount = backend.usage();
        let physical = self.controller.bytes();
        assert!(
            physical[..PARTITION.offset as usize]
                .iter()
                .all(|byte| *byte == 0xff),
            "storage must preserve bytes outside its partition"
        );
        let capsules =
            &physical[PARTITION.offset as usize..PARTITION.offset as usize + CAPSULES * nor::ERASE];
        assert_eq!(
            capsules
                .as_chunks::<{ nor::ERASE }>()
                .0
                .iter()
                .filter(|sector| sector.iter().any(|byte| *byte != 0xff))
                .count(),
            1,
            "strict mount must erase every obsolete or abandoned capsule"
        );
        let encoded = Zeroizing::new(backend.read(IDENTITY, 136)?.ok_or("identity missing")?);
        assert_eq!(encoded.len(), 136);
        let identity = LocalIdentity::from_parts(
            MemberBinding {
                endpoint: encoded[..32].try_into()?,
                incarnation: u64::from_be_bytes(encoded[32..40].try_into()?),
                signing_key: encoded[40..72].try_into()?,
            },
            encoded[72..].to_vec().into(),
        )?;
        self.adapter = Some(Adapter::new(
            Coordinator::new(backend, coordinator_limits())?,
            identity,
            self.policy.clone(),
            mls_limits(),
        )?);
        Ok(())
    }

    pub fn maintenance(&mut self, request: SecureRequest) -> TestResult<SecureUsage> {
        let report = match self.adapter().coordinator().maintain(request)? {
            MaintenanceOutcome::Complete(report) => report,
            other => return Err(format!("maintenance did not complete: {other:?}").into()),
        };
        self.reopen()?;
        Ok(report)
    }
}

pub struct Enrollment {
    pub owner: Endpoint,
    pub member: Endpoint,
    pub welcome: Vec<u8>,
}
impl Enrollment {
    pub fn new() -> TestResult<Self> {
        let owner = LocalIdentity::generate([1; 32], 1)?;
        let member = LocalIdentity::generate([2; 32], 1)?;
        let policy = Policy::new(owner.binding(), vec![owner.binding(), member.binding()])?;
        let owner = Endpoint::new(owner, policy.clone())?;
        let member = Endpoint::new(member, policy)?;
        owner.adapter().create(operation(2), GROUP)?;
        let package = member.adapter().key_package(operation(2), NOW)?;
        let added =
            owner
                .adapter()
                .add(operation(3), GROUP, &[&package.output.messages[0]], NOW)?;
        Ok(Self {
            owner,
            member,
            welcome: added.output.messages[1].clone(),
        })
    }
    pub fn joined() -> TestResult<Self> {
        let peers = Self::new()?;
        peers
            .member
            .adapter()
            .join(operation(3), GROUP, &peers.welcome, NOW)?;
        Ok(peers)
    }
}

/// Every marker write in a normal strict transaction precedes the final capsule
/// retirement erase. Discover its position from actual I/O, not a fake backend
/// hook or a hard-coded number of encrypted object writes.
pub fn publication_and_retirement(actions: &[nor::Action]) -> (usize, usize) {
    let capsules_end = PARTITION.offset + (CAPSULES * nor::ERASE) as u32;
    let publication = actions
        .iter()
        .rposition(|action| {
            matches!(action,
                nor::Action::Write { offset, length: 256 }
                if *offset >= PARTITION.offset && *offset < capsules_end
                    && (*offset - PARTITION.offset) % nor::ERASE as u32 == nor::ERASE as u32 - 256
            )
        })
        .expect("capsule publication write");
    let retirement = actions
        .iter()
        .enumerate()
        .skip(publication + 1)
        .find_map(|(i, action)| {
            matches!(action, nor::Action::Erase { offset, .. }
            if *offset >= PARTITION.offset && *offset < capsules_end)
            .then_some(i)
        })
        .expect("old capsule retirement erase follows publication");
    (publication, retirement)
}
