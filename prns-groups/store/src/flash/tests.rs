#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{Coordinator, Limits, StoreError};
use alloc::vec;
use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash};

const SLOT: u32 = 4096;
const PARTITION: Partition = Partition {
    offset: 4096,
    length: 4 * SLOT,
};
const LIMITS: FlashLimits = FlashLimits {
    slot_bytes: SLOT,
    max_changes: 8,
    max_key_bytes: 64,
    max_value_bytes: 3000,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PowerLoss;
impl NorFlashError for PowerLoss {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}

struct Nor {
    bytes: Vec<u8>,
    program_budget: Option<usize>,
    read_failure: bool,
    writes: usize,
}

impl Nor {
    fn erased() -> Self {
        let mut bytes = vec![0x96; (PARTITION.offset + PARTITION.length + SLOT) as usize];
        bytes[PARTITION.offset as usize..(PARTITION.offset + PARTITION.length) as usize].fill(0xff);
        Self {
            bytes,
            program_budget: None,
            read_failure: false,
            writes: 0,
        }
    }
    fn reboot(mut self) -> Self {
        self.program_budget = None;
        self.read_failure = false;
        self
    }
}
impl ErrorType for Nor {
    type Error = PowerLoss;
}
impl ReadNorFlash for Nor {
    const READ_SIZE: usize = 4;
    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        assert_eq!(offset as usize % Self::READ_SIZE, 0);
        assert_eq!(bytes.len() % Self::READ_SIZE, 0);
        if self.read_failure {
            return Err(PowerLoss);
        }
        bytes.copy_from_slice(&self.bytes[offset as usize..offset as usize + bytes.len()]);
        Ok(())
    }
    fn capacity(&self) -> usize {
        self.bytes.len()
    }
}
impl NorFlash for Nor {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = 4096;
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        assert_eq!(offset as usize % Self::WRITE_SIZE, 0);
        assert_eq!(bytes.len() % Self::WRITE_SIZE, 0);
        assert!(offset >= PARTITION.offset);
        assert!(offset as usize + bytes.len() <= (PARTITION.offset + PARTITION.length) as usize);
        self.writes += 1;
        for (stored, incoming) in self.bytes[offset as usize..][..bytes.len()]
            .iter_mut()
            .zip(bytes)
        {
            if self.program_budget == Some(0) {
                return Err(PowerLoss);
            }
            // This implementation deliberately does not support programming a
            // location twice, even when the bitwise AND would be legal.
            assert_eq!(*stored, 0xff);
            *stored &= incoming;
            if let Some(remaining) = &mut self.program_budget {
                *remaining -= 1;
            }
        }
        if self.program_budget == Some(0) {
            return Err(PowerLoss);
        }
        Ok(())
    }
    fn erase(&mut self, _from: u32, _to: u32) -> Result<(), Self::Error> {
        panic!("the journal must never erase behind retained receipts")
    }
}

fn mount(nor: Nor) -> FlashBackend<Nor> {
    FlashBackend::mount(nor, PARTITION, LIMITS).unwrap()
}
fn op(id: u8) -> OperationId {
    OperationId([id; 32])
}
fn put(key: &[u8], value: &[u8]) -> Mutation {
    Mutation::Put {
        key: key.to_vec(),
        value: value.to_vec(),
    }
}
fn delete(key: &[u8]) -> Mutation {
    Mutation::Delete { key: key.to_vec() }
}

#[test]
fn erased_partition_is_empty_and_only_owned_range_is_written() {
    let mut backend = mount(Nor::erased());
    assert_eq!(backend.head().unwrap(), Revision(0));
    assert_eq!(backend.read(b"missing", 1).unwrap(), None);
    assert_eq!(
        backend.recover(op(1)).unwrap(),
        RecoveryOutcome::NotCommitted {
            revision: Revision(0)
        }
    );
    assert_eq!(
        backend.usage(),
        FlashUsage {
            committed_slots: 0,
            retired_slots: 0,
            total_slots: 4,
            next_slot: 0
        }
    );
    backend
        .commit(Revision(0), op(1), &[put(b"secret", b"state")])
        .unwrap();
    let nor = backend.into_inner();
    assert!(nor.bytes[..PARTITION.offset as usize]
        .iter()
        .all(|byte| *byte == 0x96));
    assert!(nor.bytes[(PARTITION.offset + PARTITION.length) as usize..]
        .iter()
        .all(|byte| *byte == 0x96));
    let mut backend = mount(nor.reboot());
    assert_eq!(backend.read(b"secret", 5).unwrap(), Some(b"state".to_vec()));
    assert_eq!(backend.head().unwrap(), Revision(1));
    assert!(matches!(
        backend.recover(op(1)).unwrap(),
        RecoveryOutcome::Committed(_)
    ));
}

#[test]
fn mutation_set_and_receipt_publish_together_and_replay_never_reapplies() {
    let mut backend = mount(Nor::erased());
    let first = backend
        .commit(Revision(0), op(1), &[put(b"a", b"old"), put(b"b", b"old")])
        .unwrap();
    backend
        .commit(
            Revision(1),
            op(2),
            &[put(b"a", b"new"), delete(b"b"), put(b"empty", b"")],
        )
        .unwrap();
    let writes = backend.flash.writes;
    assert_eq!(
        backend
            .commit(Revision(2), op(1), &[put(b"a", b"reused")])
            .unwrap(),
        first
    );
    assert_eq!(backend.flash.writes, writes);
    assert_eq!(
        backend
            .commit(Revision(0), op(3), &[put(b"a", b"wrong")])
            .unwrap(),
        CommitOutcome::NotCommitted {
            revision: Revision(2)
        }
    );
    let mut backend = mount(backend.into_inner().reboot());
    assert_eq!(backend.read(b"a", 3).unwrap(), Some(b"new".to_vec()));
    assert_eq!(backend.read(b"b", 3).unwrap(), None);
    assert_eq!(backend.read(b"empty", 0).unwrap(), Some(vec![]));
    assert_eq!(backend.read(b"a", 2), Err(FlashError::ValueTooLarge));
}

#[test]
fn direct_duplicate_mutations_have_last_mutation_semantics() {
    let mut backend = mount(Nor::erased());
    backend
        .commit(
            Revision(0),
            op(1),
            &[put(b"a", b"a"), delete(b"a"), put(b"a", b"last")],
        )
        .unwrap();
    assert_eq!(backend.read(b"a", 4).unwrap(), Some(b"last".to_vec()));
}

#[test]
fn every_program_byte_cut_preserves_previous_commit_or_fails_closed() {
    let mut seed = mount(Nor::erased());
    seed.commit(Revision(0), op(1), &[put(b"a", b"old"), put(b"b", b"old")])
        .unwrap();
    let image = seed.into_inner().bytes;
    let changes = [
        put(b"a", &[0x51; 300]),
        delete(b"b"),
        put(b"inbox", &[0x62; 11]),
    ];
    // The complete body occupies two 256-byte writes; the marker is a third.
    for cut in 0..=3 * BLOCK {
        let nor = Nor {
            bytes: image.clone(),
            program_budget: Some(cut),
            read_failure: false,
            writes: 0,
        };
        let mut backend = mount(nor);
        assert_eq!(
            backend.commit(Revision(1), op(2), &changes),
            Err(FlashError::Io(PowerLoss)),
            "cut={cut}"
        );
        let reopened = FlashBackend::mount(backend.into_inner().reboot(), PARTITION, LIMITS);
        if cut > 2 * BLOCK && cut < 3 * BLOCK {
            assert!(
                matches!(reopened, Err(FlashError::Corrupt { slot: 1 })),
                "cut={cut}"
            );
            continue;
        }
        let mut reopened = reopened.unwrap();
        let outcome = reopened.recover(op(2)).unwrap();
        if cut == 3 * BLOCK {
            assert_eq!(
                outcome,
                RecoveryOutcome::Committed(CommitReceipt {
                    operation_id: op(2),
                    revision: Revision(2)
                })
            );
            assert_eq!(reopened.read(b"a", 300).unwrap(), Some(vec![0x51; 300]));
            assert_eq!(reopened.read(b"b", 3).unwrap(), None);
            assert_eq!(reopened.read(b"inbox", 11).unwrap(), Some(vec![0x62; 11]));
        } else {
            assert_eq!(
                outcome,
                RecoveryOutcome::NotCommitted {
                    revision: Revision(1)
                }
            );
            assert_eq!(reopened.read(b"a", 3).unwrap(), Some(b"old".to_vec()));
            assert_eq!(reopened.read(b"b", 3).unwrap(), Some(b"old".to_vec()));
            assert_eq!(reopened.read(b"inbox", 11).unwrap(), None);
            assert_eq!(reopened.usage().retired_slots, usize::from(cut != 0));
            // A new transaction can proceed in the next clean slot. It cannot
            // overwrite remnants of the interrupted record.
            reopened.commit(Revision(1), op(2), &changes).unwrap();
            assert_eq!(reopened.head().unwrap(), Revision(2));
        }
    }
}

#[test]
fn uncommitted_torn_slots_retire_and_capacity_exhaustion_never_erases() {
    let mut backend = mount(Nor::erased());
    for id in 1..=4 {
        backend.flash.program_budget = Some(1);
        assert!(backend
            .commit(Revision(0), op(id), &[put(b"a", b"new")])
            .is_err());
        backend.flash.program_budget = None;
        assert_eq!(
            backend.recover(op(id)).unwrap(),
            RecoveryOutcome::NotCommitted {
                revision: Revision(0)
            }
        );
    }
    assert_eq!(backend.usage().retired_slots, 4);
    assert_eq!(
        backend.commit(Revision(0), op(5), &[]),
        Err(FlashError::Full)
    );
    assert_eq!(backend.head().unwrap(), Revision(0));
}

#[test]
fn full_receipt_journal_still_supports_reads_and_recovery() {
    let mut backend = mount(Nor::erased());
    for id in 1..=4 {
        backend
            .commit(Revision(u64::from(id - 1)), op(id), &[put(b"a", &[id])])
            .unwrap();
    }
    assert_eq!(
        backend.commit(Revision(4), op(5), &[]),
        Err(FlashError::Full)
    );
    let mut backend = mount(backend.into_inner().reboot());
    assert_eq!(backend.read(b"a", 1).unwrap(), Some(vec![4]));
    for id in 1..=4 {
        assert_eq!(
            backend.recover(op(id)).unwrap(),
            RecoveryOutcome::Committed(CommitReceipt {
                operation_id: op(id),
                revision: Revision(u64::from(id))
            })
        );
    }
}

#[test]
fn corrupted_published_header_body_crc_or_marker_never_looks_empty() {
    let mut seed = mount(Nor::erased());
    seed.commit(Revision(0), op(1), &[put(b"a", b"secret")])
        .unwrap();
    let image = seed.into_inner().bytes;
    for offset in [
        0,
        12,
        16,
        24,
        60,
        HEADER,
        HEADER + ENTRY + 1,
        HEADER + ENTRY + 1 + 6,
        SLOT as usize - BLOCK,
    ] {
        let mut nor = Nor::erased();
        nor.bytes = image.clone();
        nor.bytes[PARTITION.offset as usize + offset] ^= 1;
        assert!(
            matches!(
                FlashBackend::mount(nor, PARTITION, LIMITS),
                Err(FlashError::Corrupt { slot: 0 })
            ),
            "offset={offset}"
        );
    }
}

#[test]
fn malformed_entry_rejected_even_when_checksum_is_valid() {
    let mut backend = mount(Nor::erased());
    backend
        .commit(Revision(0), op(1), &[put(b"a", b"secret")])
        .unwrap();
    let mut nor = backend.into_inner();
    let start = PARTITION.offset as usize;
    nor.bytes[start + HEADER] = 4;
    let crc_offset = start + HEADER + ENTRY + 1 + 6;
    let crc = checksum(&nor.bytes[start..crc_offset]);
    nor.bytes[crc_offset..crc_offset + 4].copy_from_slice(&crc.to_le_bytes());
    assert!(matches!(
        FlashBackend::mount(nor, PARTITION, LIMITS),
        Err(FlashError::Corrupt { slot: 0 })
    ));
}

#[test]
fn detected_corruption_fences_later_writes_and_recovery() {
    let mut backend = mount(Nor::erased());
    backend
        .commit(Revision(0), op(1), &[put(b"a", b"secret")])
        .unwrap();
    backend.flash.bytes[PARTITION.offset as usize + HEADER + ENTRY + 1] ^= 1;
    let writes = backend.flash.writes;
    assert_eq!(backend.read(b"a", 6), Err(FlashError::Corrupt { slot: 0 }));
    assert_eq!(
        backend.commit(Revision(1), op(2), &[put(b"a", b"new")]),
        Err(FlashError::Corrupt { slot: 0 })
    );
    assert_eq!(backend.recover(op(1)), Err(FlashError::Corrupt { slot: 0 }));
    assert_eq!(backend.head(), Err(FlashError::Corrupt { slot: 0 }));
    assert_eq!(backend.flash.writes, writes);
}

#[test]
fn duplicate_receipts_and_revision_gaps_fail_closed() {
    let mut seed = mount(Nor::erased());
    seed.commit(Revision(0), op(1), &[]).unwrap();
    seed.commit(Revision(1), op(2), &[]).unwrap();
    let image = seed.into_inner().bytes;
    for duplicate in [false, true] {
        let mut nor = Nor::erased();
        nor.bytes = image.clone();
        let start = (PARTITION.offset + SLOT) as usize;
        if duplicate {
            nor.bytes[start + 24..start + 56].copy_from_slice(&op(1).0);
        } else {
            nor.bytes[start + 16..start + 24].copy_from_slice(&3u64.to_le_bytes());
        }
        let crc = checksum(&nor.bytes[start..start + 60]);
        nor.bytes[start + 60..start + HEADER].copy_from_slice(&crc.to_le_bytes());
        let crc = checksum(&nor.bytes[start..start + HEADER]);
        nor.bytes[start + HEADER..start + HEADER + 4].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            FlashBackend::mount(nor, PARTITION, LIMITS),
            Err(FlashError::Corrupt { slot: 1 })
        ));
    }
}

#[test]
fn limits_and_geometry_reject_before_programming() {
    let mut backend = mount(Nor::erased());
    assert_eq!(
        backend.commit(Revision(0), op(1), &[put(&[1; 65], b"")]),
        Err(FlashError::KeyTooLarge)
    );
    assert_eq!(
        backend.commit(Revision(0), op(1), &[put(b"a", &[1; 3001])]),
        Err(FlashError::ValueTooLarge)
    );
    assert_eq!(
        backend.commit(
            Revision(0),
            op(1),
            &(0..9).map(|_| delete(b"a")).collect::<Vec<_>>()
        ),
        Err(FlashError::TooManyChanges)
    );
    assert_eq!(
        backend.commit(
            Revision(0),
            op(1),
            &[put(b"a", &[1; 2000]), put(b"b", &[2; 2000])]
        ),
        Err(FlashError::TransactionTooLarge)
    );
    assert_eq!(backend.flash.writes, 0);
    for partition in [
        Partition {
            offset: 1,
            ..PARTITION
        },
        Partition {
            length: 0,
            ..PARTITION
        },
        Partition {
            length: SLOT + 1,
            ..PARTITION
        },
        Partition {
            offset: 0xffff_f000,
            ..PARTITION
        },
    ] {
        assert!(matches!(
            FlashBackend::mount(Nor::erased(), partition, LIMITS),
            Err(FlashError::InvalidGeometry)
        ));
    }
    assert!(matches!(
        FlashBackend::mount(
            Nor::erased(),
            PARTITION,
            FlashLimits {
                slot_bytes: 0,
                ..LIMITS
            }
        ),
        Err(FlashError::InvalidGeometry)
    ));
}

#[test]
fn read_failure_is_not_absence_and_poisons_coordinator_until_recovery() {
    let mut backend = mount(Nor::erased());
    backend.flash.program_budget = Some(BLOCK);
    let coordinator = Coordinator::new(
        backend,
        Limits {
            max_changes: 8,
            max_key_bytes: 64,
            max_value_bytes: 3000,
            max_staged_bytes: 3500,
        },
    )
    .unwrap();
    let tx = coordinator.begin(op(1), Revision(0)).unwrap();
    tx.put(b"secret", b"value").unwrap();
    assert!(matches!(
        tx.commit(),
        Err(StoreError::Backend(FlashError::Io(PowerLoss)))
    ));
    assert!(matches!(
        coordinator.head(),
        Err(StoreError::RecoveryRequired(_))
    ));
    // A write-only failure does not prevent read-back recovery of an unpublished
    // body. The coordinator clears its fence only after that definite result.
    assert_eq!(
        coordinator.recover(op(1)).unwrap(),
        RecoveryOutcome::NotCommitted {
            revision: Revision(0)
        }
    );
    assert_eq!(coordinator.head().unwrap(), Revision(0));
    let mut backend = mount(Nor::erased());
    backend
        .commit(Revision(0), op(1), &[put(b"a", b"value")])
        .unwrap();
    backend.flash.read_failure = true;
    assert_eq!(backend.read(b"a", 5), Err(FlashError::Io(PowerLoss)));
    assert_eq!(backend.recover(op(1)), Err(FlashError::Io(PowerLoss)));
    assert_eq!(backend.head(), Err(FlashError::Io(PowerLoss)));
}
