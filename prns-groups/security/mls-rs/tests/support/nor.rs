//! Byte-addressed NOR model for real MLS operations; deliberately not a filesystem
//! or a transactional map. Only flash bytes survive each backend/adapter reopen.

use embedded_storage::nor_flash::{
    ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
};
use prns_groups_mls::{Adapter, Limits, LocalIdentity, Policy};
use prns_groups_store::flash::{
    FlashBackend, FlashLimits, FlashUsage, Partition, COMMIT_MARKER_BYTES,
};
use prns_groups_store::{Coordinator, GroupId, Limits as StoreLimits, OperationId};
use std::sync::{Arc, Mutex};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
pub type Backend = FlashBackend<Flash>;
pub const GROUP: GroupId = GroupId([0x29; 32]);
pub const NOW: u64 = 1_791_763_200;
const SLOT: u32 = 64 * 1024;
const PARTITION: Partition = Partition {
    offset: 4096,
    length: 16 * SLOT,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    TornBody,
    BeforeMarker,
    AfterMarker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerLoss;
impl NorFlashError for PowerLoss {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}

struct Image {
    bytes: Vec<u8>,
    cut: Option<Cut>,
    fired: bool,
}

#[derive(Clone)]
pub struct Controller(Arc<Mutex<Image>>);
impl Controller {
    pub fn arm(&self, cut: Cut) {
        let mut image = self.0.lock().unwrap();
        image.cut = Some(cut);
        image.fired = false;
    }
    pub fn fired(&self) -> bool {
        self.0.lock().unwrap().fired
    }
    /// A host-only evidence copy, never used by the production backend to commit.
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().bytes.clone()
    }
}

pub struct Flash(Controller);
impl ErrorType for Flash {
    type Error = PowerLoss;
}
impl ReadNorFlash for Flash {
    const READ_SIZE: usize = 4;
    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        assert!(offset.is_multiple_of(Self::READ_SIZE as u32));
        assert!(bytes.len().is_multiple_of(Self::READ_SIZE));
        let image = self.0 .0.lock().unwrap();
        bytes.copy_from_slice(&image.bytes[offset as usize..offset as usize + bytes.len()]);
        Ok(())
    }
    fn capacity(&self) -> usize {
        (PARTITION.offset + PARTITION.length) as usize
    }
}
impl NorFlash for Flash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = 4096;
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        assert!(offset.is_multiple_of(Self::WRITE_SIZE as u32));
        assert!(bytes.len().is_multiple_of(Self::WRITE_SIZE));
        assert!(offset >= PARTITION.offset);
        assert!(offset as usize + bytes.len() <= (PARTITION.offset + PARTITION.length) as usize);
        let mut image = self.0 .0.lock().unwrap();
        let within_slot = (offset - PARTITION.offset) % SLOT;
        let marker = within_slot == SLOT - COMMIT_MARKER_BYTES as u32;
        let fault = match image.cut {
            Some(Cut::TornBody) if within_slot == 0 => Some(Cut::TornBody),
            Some(cut @ (Cut::BeforeMarker | Cut::AfterMarker)) if marker => Some(cut),
            _ => None,
        };
        let written = match fault {
            Some(Cut::TornBody) => bytes.len() / 2,
            Some(Cut::BeforeMarker) => 0,
            _ => bytes.len(),
        };
        for (destination, source) in image.bytes[offset as usize..offset as usize + written]
            .iter_mut()
            .zip(bytes)
        {
            assert_eq!(*destination, 0xff, "each programmed position must be fresh");
            *destination &= source;
        }
        if fault.is_some() {
            image.cut = None;
            image.fired = true;
            return Err(PowerLoss);
        }
        Ok(())
    }
    fn erase(&mut self, _from: u32, _to: u32) -> Result<(), Self::Error> {
        panic!("the append-only backend must not erase receipts")
    }
}

pub struct Endpoint {
    pub controller: Controller,
    identity: LocalIdentity,
    policy: Policy,
    adapter: Option<Adapter<Backend>>,
    pub last_mount: FlashUsage,
}

impl Endpoint {
    fn new(identity: LocalIdentity, policy: Policy) -> TestResult<Self> {
        let mut peer = Self {
            controller: Controller(Arc::new(Mutex::new(Image {
                bytes: vec![0xff; (PARTITION.offset + PARTITION.length) as usize],
                cut: None,
                fired: false,
            }))),
            identity,
            policy,
            adapter: None,
            last_mount: FlashUsage {
                committed_slots: 0,
                retired_slots: 0,
                total_slots: 0,
                next_slot: 0,
            },
        };
        peer.reopen()?;
        Ok(peer)
    }

    pub fn adapter(&self) -> &Adapter<Backend> {
        self.adapter.as_ref().unwrap()
    }

    pub fn reopen(&mut self) -> TestResult {
        // Recreate every security/storage object; only the physical byte image
        // and the fixture's stable provisioned identity/configuration survive.
        drop(self.adapter.take());
        let backend = FlashBackend::mount(
            Flash(self.controller.clone()),
            PARTITION,
            FlashLimits {
                slot_bytes: SLOT,
                max_changes: 48,
                max_key_bytes: 64,
                max_value_bytes: 32 * 1024,
            },
        )?;
        self.last_mount = backend.usage();
        let store = Coordinator::new(
            backend,
            StoreLimits {
                max_changes: 48,
                max_key_bytes: 64,
                max_value_bytes: 32 * 1024,
                max_staged_bytes: 60 * 1024,
            },
        )?;
        // Match the first E290 qualification profile; these are explicit test
        // inputs, not defaults promised by the reusable library.
        let limits = Limits {
            max_members: 4,
            max_groups: 1,
            max_wire_bytes: 16 * 1024,
            max_plaintext_bytes: 1024,
            max_snapshot_bytes: 32 * 1024,
            max_key_packages: 2,
            max_retained_epochs: 1,
            max_pending_proposals: 4,
            max_pending_outputs: 24,
        };
        self.adapter = Some(Adapter::new(
            store,
            self.identity.clone(),
            self.policy.clone(),
            limits,
        )?);
        Ok(())
    }
}

pub struct Peers {
    pub owner: Endpoint,
    pub member: Endpoint,
}
impl Peers {
    pub fn joined() -> TestResult<Self> {
        let owner = LocalIdentity::generate([1; 32], 1)?;
        let member = LocalIdentity::generate([2; 32], 1)?;
        let policy = Policy::new(owner.binding(), vec![owner.binding(), member.binding()])?;
        let owner = Endpoint::new(owner, policy.clone())?;
        let member = Endpoint::new(member, policy)?;
        owner.adapter().create(OperationId([1; 32]), GROUP)?;
        let package = member.adapter().key_package(OperationId([1; 32]), NOW)?;
        let add = owner.adapter().add(
            OperationId([2; 32]),
            GROUP,
            &[&package.output.messages[0]],
            NOW,
        )?;
        member
            .adapter()
            .join(OperationId([2; 32]), GROUP, &add.output.messages[1], NOW)?;
        Ok(Self { owner, member })
    }
}
