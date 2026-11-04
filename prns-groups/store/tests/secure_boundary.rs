//! Public-API checks independent of the backend's private format/fault fixtures.
#![cfg(feature = "secure")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use embedded_storage::nor_flash::{
    ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
};
use prns_groups_core::{OperationTicket, StoreIncarnation};
use prns_groups_store::{
    flash::Partition,
    secure::{Entropy, KeyProvider, SecureBackend, SecureLimits, SecureRequest},
    AtomicBackend, CommitOutcome, MaintenanceBackend, MaintenanceOutcome, Mutation,
    OperationRejection, RecoveryOutcome, Revision,
};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

const SECTOR: usize = 4096;
const PARTITION: Partition = Partition {
    offset: SECTOR as u32,
    length: 20 * SECTOR as u32,
};
const SENTINEL: u8 = 0x96;

#[derive(Debug)]
struct Io;
impl NorFlashError for Io {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}

#[derive(Clone)]
struct Nor(Vec<u8>);
impl Nor {
    fn new() -> Self {
        let mut bytes = vec![SENTINEL; PARTITION.length as usize + 2 * SECTOR];
        bytes[SECTOR..SECTOR + PARTITION.length as usize].fill(0xff);
        Self(bytes)
    }
    fn assert_partition_isolation(&self) {
        assert!(self.0[..SECTOR].iter().all(|byte| *byte == SENTINEL));
        assert!(self.0[SECTOR + PARTITION.length as usize..]
            .iter()
            .all(|byte| *byte == SENTINEL));
    }
}
impl ErrorType for Nor {
    type Error = Io;
}
impl ReadNorFlash for Nor {
    const READ_SIZE: usize = 4;
    fn capacity(&self) -> usize {
        self.0.len()
    }
    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Io> {
        assert_eq!(offset as usize % Self::READ_SIZE, 0);
        assert_eq!(bytes.len() % Self::READ_SIZE, 0);
        bytes.copy_from_slice(&self.0[offset as usize..offset as usize + bytes.len()]);
        Ok(())
    }
}
impl NorFlash for Nor {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = SECTOR;
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Io> {
        assert_eq!(offset as usize % Self::WRITE_SIZE, 0);
        assert_eq!(bytes.len() % Self::WRITE_SIZE, 0);
        assert!(offset >= PARTITION.offset);
        assert!(offset as usize + bytes.len() <= (PARTITION.offset + PARTITION.length) as usize);
        for (stored, incoming) in self.0[offset as usize..offset as usize + bytes.len()]
            .iter_mut()
            .zip(bytes)
        {
            assert_eq!(*stored & incoming, *incoming);
            *stored &= incoming;
        }
        Ok(())
    }
    fn erase(&mut self, from: u32, to: u32) -> Result<(), Io> {
        assert_eq!(from as usize % SECTOR, 0);
        assert_eq!(to as usize % SECTOR, 0);
        assert!(from >= PARTITION.offset && to <= PARTITION.offset + PARTITION.length);
        self.0[from as usize..to as usize].fill(0xff);
        Ok(())
    }
}

struct TestKey(u8);
impl KeyProvider for TestKey {
    type Error = Io;
    fn wrapping_key(&mut self, out: &mut [u8; 32]) -> Result<(), Io> {
        out.fill(self.0);
        Ok(())
    }
}

// Test-only entropy independent of simulated persistent flash and never rewound
// on reboot. Deterministic streams restart neither keys nor wrapping nonces.
struct TestEntropy;
impl Entropy for TestEntropy {
    type Error = Io;
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Io> {
        static ATTEMPT: AtomicU64 = AtomicU64::new(1);
        for chunk in out.chunks_mut(32) {
            let counter = ATTEMPT.fetch_add(1, Ordering::Relaxed);
            let bytes = Sha256::digest(counter.to_be_bytes());
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        Ok(())
    }
}

type Backend = SecureBackend<Nor, TestKey, TestEntropy>;
fn incarnation() -> StoreIncarnation {
    StoreIncarnation::new([0xa7; 24]).unwrap()
}
fn ticket(sequence: u64) -> OperationTicket {
    OperationTicket::new(incarnation(), sequence).unwrap()
}
fn limits() -> SecureLimits {
    SecureLimits {
        capsule_sectors: 4,
        max_live_objects: 16,
        max_catalog_bytes: 8192,
        max_key_bytes: 64,
        max_value_bytes: 4096,
        max_changes: 8,
        max_receipts: 4,
        reserve_bytes: 12 * 1024,
    }
}
fn fresh() -> Backend {
    Backend::format(
        Nor::new(),
        PARTITION,
        limits(),
        incarnation(),
        TestKey(0x73),
        TestEntropy,
    )
    .unwrap()
}
fn reopen(nor: Nor) -> Backend {
    Backend::mount(
        nor,
        PARTITION,
        limits(),
        incarnation(),
        TestKey(0x73),
        TestEntropy,
    )
    .unwrap()
}
fn put(value: &[u8]) -> Mutation {
    Mutation::Put {
        key: b"ratchet".to_vec(),
        value: value.to_vec(),
    }
}

#[test]
fn opening_never_reinitializes_missing_or_mismatched_authenticated_state() {
    assert!(Backend::mount(
        Nor::new(),
        PARTITION,
        limits(),
        incarnation(),
        TestKey(0x73),
        TestEntropy
    )
    .is_err());
    let mut backend = fresh();
    backend
        .commit(Revision(0), ticket(1).into(), &[put(b"private state")])
        .unwrap();
    let (image, _, _) = backend.into_parts();
    image.assert_partition_isolation();
    assert!(Backend::mount(
        image.clone(),
        PARTITION,
        limits(),
        incarnation(),
        TestKey(0x72),
        TestEntropy
    )
    .is_err());
    let foreign = StoreIncarnation::new([0xb8; 24]).unwrap();
    assert!(Backend::mount(
        image.clone(),
        PARTITION,
        limits(),
        foreign,
        TestKey(0x73),
        TestEntropy
    )
    .is_err());
    let mut changed = limits();
    changed.max_receipts += 1;
    assert!(Backend::mount(
        image.clone(),
        PARTITION,
        changed,
        incarnation(),
        TestKey(0x73),
        TestEntropy
    )
    .is_err());
    assert!(Backend::format(
        image.clone(),
        PARTITION,
        limits(),
        incarnation(),
        TestKey(0x73),
        TestEntropy
    )
    .is_err());
    assert_eq!(
        reopen(image).read(b"ratchet", 32).unwrap(),
        Some(b"private state".to_vec())
    );
}

#[test]
fn receipt_floor_survives_repeated_compaction_and_full_recovery_windows() {
    let mut backend = fresh();
    for sequence in 1..=4 {
        assert!(matches!(
            backend
                .commit(
                    Revision(sequence - 1),
                    ticket(sequence).into(),
                    &[put(&sequence.to_be_bytes())]
                )
                .unwrap(),
            CommitOutcome::Committed(_)
        ));
    }
    assert_eq!(
        backend.recover(ticket(5).into()).unwrap(),
        RecoveryOutcome::Rejected(OperationRejection::RecoveryWindowFull)
    );
    assert!(matches!(
        backend.maintain(SecureRequest::Compact).unwrap(),
        MaintenanceOutcome::Complete(_)
    ));
    assert!(matches!(
        backend.recover(ticket(1).into()).unwrap(),
        RecoveryOutcome::Committed(_)
    ));
    backend
        .maintain(SecureRequest::FinalizeThrough(ticket(2)))
        .unwrap();
    for _ in 0..6 {
        let previous = backend.usage();
        assert!(matches!(
            backend.maintain(SecureRequest::Compact).unwrap(),
            MaintenanceOutcome::Complete(_)
        ));
        let current = backend.usage();
        assert_eq!(current.active_arena, 1 - previous.active_arena);
        assert_eq!(current.generation, previous.generation + 1);
        assert_eq!(current.revision, previous.revision);
        let (image, _, _) = backend.into_parts();
        image.assert_partition_isolation();
        let old_arena_start = PARTITION.offset as usize
            + limits().capsule_sectors * SECTOR
            + previous.active_arena * previous.arena_bytes;
        assert!(
            image.0[old_arena_start..old_arena_start + previous.arena_bytes]
                .iter()
                .all(|byte| *byte == 0xff)
        );
        backend = reopen(image);
        assert_eq!(
            backend.recover(ticket(1).into()).unwrap(),
            RecoveryOutcome::Retired {
                revision: Revision(4),
                retired_through: ticket(2)
            }
        );
        assert_eq!(
            backend
                .commit(Revision(4), ticket(1).into(), &[put(b"replay")])
                .unwrap(),
            CommitOutcome::Rejected(OperationRejection::RetiredTicket)
        );
        assert!(matches!(
            backend.recover(ticket(3).into()).unwrap(),
            RecoveryOutcome::Committed(_)
        ));
    }
    assert!(matches!(
        backend
            .commit(Revision(4), ticket(5).into(), &[put(b"next")])
            .unwrap(),
        CommitOutcome::Committed(_)
    ));
    assert_eq!(
        backend.read(b"ratchet", 32).unwrap(),
        Some(b"next".to_vec())
    );
}

#[test]
fn corrupted_authoritative_capsule_does_not_fall_back_to_old_ratchets() {
    let mut backend = fresh();
    backend
        .commit(Revision(0), ticket(1).into(), &[put(b"old")])
        .unwrap();
    backend
        .commit(Revision(1), ticket(2).into(), &[put(b"new")])
        .unwrap();
    let (mut image, _, _) = backend.into_parts();
    let active: Vec<_> = (0..limits().capsule_sectors)
        .filter(|slot| {
            let start = PARTITION.offset as usize + slot * SECTOR;
            image.0[start..start + SECTOR].iter().any(|b| *b != 0xff)
        })
        .collect();
    assert_eq!(active.len(), 1, "strict retirement leaves one capsule only");
    let start = PARTITION.offset as usize + active[0] * SECTOR;
    image.0[start + 64] ^= 1;
    assert!(Backend::mount(
        image,
        PARTITION,
        limits(),
        incarnation(),
        TestKey(0x73),
        TestEntropy
    )
    .is_err());
}

#[test]
fn archived_whole_flash_image_remains_a_deliberate_rollback_limit() {
    let mut backend = fresh();
    backend
        .commit(Revision(0), ticket(1).into(), &[put(b"first")])
        .unwrap();
    let (old, _, _) = backend.into_parts();
    let archived = old.clone();
    let mut backend = reopen(old);
    backend
        .commit(Revision(1), ticket(2).into(), &[put(b"second")])
        .unwrap();
    assert_eq!(
        backend.read(b"ratchet", 32).unwrap(),
        Some(b"second".to_vec())
    );
    // No anchor outside flash is claimed. Retiring the current device's wrapper
    // cannot revoke a capsule an attacker previously copied with its ciphertext.
    let mut replayed = reopen(archived);
    assert_eq!(replayed.head().unwrap(), Revision(1));
    assert_eq!(
        replayed.read(b"ratchet", 32).unwrap(),
        Some(b"first".to_vec())
    );
}
