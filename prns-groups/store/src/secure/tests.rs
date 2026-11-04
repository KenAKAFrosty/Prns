extern crate std;

use super::*;
use alloc::{vec, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};
use embedded_storage::nor_flash::{
    ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
};
static ENTROPY_SEED: AtomicU64 = AtomicU64::new(1);
fn random() -> Random {
    Random(
        ENTROPY_SEED
            .fetch_add(0x9e3779b97f4a7c15, Ordering::Relaxed)
            .wrapping_add(0x12345678),
    )
}
use crate::{
    AtomicBackend, CommitOutcome, MaintenanceBackend, MaintenanceOutcome, Mutation, OperationId,
    OperationRejection, OperationTicket, RecoveryOutcome,
};

const SECTOR: usize = 4096;
const PART: Partition = Partition {
    offset: SECTOR as u32,
    length: (3 * SECTOR + 2 * 32768) as u32,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Io {
    Cut,
    Contract,
}
impl NorFlashError for Io {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}
#[derive(Clone, Copy, Debug)]
enum Call {
    Write(u32, usize),
    Erase(u32, usize),
}
#[derive(Clone)]
struct Flash {
    bytes: Vec<u8>,
    cut: Option<(usize, usize)>,
    corrupt_write: Option<usize>,
    failed: bool,
    calls: Vec<Call>,
}
impl Flash {
    fn fresh() -> Self {
        Self {
            bytes: vec![0xff; PART.offset as usize + PART.length as usize + SECTOR],
            cut: None,
            corrupt_write: None,
            failed: false,
            calls: Vec::new(),
        }
    }
    fn boot(mut self) -> Self {
        self.cut = None;
        self.corrupt_write = None;
        self.failed = false;
        self.calls.clear();
        self
    }
    fn count(&mut self, call: Call, length: usize) -> Result<(usize, bool), Io> {
        if self.failed {
            return Err(Io::Cut);
        }
        let index = self.calls.len();
        self.calls.push(call);
        Ok(match self.cut {
            Some((at, bytes)) if at == index => (bytes.min(length), true),
            _ => (length, false),
        })
    }
}
impl ErrorType for Flash {
    type Error = Io;
}
impl ReadNorFlash for Flash {
    const READ_SIZE: usize = 4;
    fn capacity(&self) -> usize {
        self.bytes.len()
    }
    fn read(&mut self, offset: u32, out: &mut [u8]) -> Result<(), Io> {
        if self.failed {
            return Err(Io::Cut);
        }
        let start = offset as usize;
        if start + out.len() > self.bytes.len()
            || !start.is_multiple_of(4)
            || !out.len().is_multiple_of(4)
        {
            return Err(Io::Contract);
        }
        out.copy_from_slice(&self.bytes[start..start + out.len()]);
        Ok(())
    }
}
impl NorFlash for Flash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = SECTOR;
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Io> {
        let start = offset as usize;
        if !start.is_multiple_of(4)
            || !bytes.len().is_multiple_of(4)
            || start + bytes.len() > self.bytes.len()
        {
            return Err(Io::Contract);
        }
        let (n, cut) = self.count(Call::Write(offset, bytes.len()), bytes.len())?;
        for (old, new) in self.bytes[start..start + n].iter_mut().zip(bytes) {
            if *old & *new != *new {
                return Err(Io::Contract);
            }
            *old &= *new;
        }
        if self.corrupt_write == Some(self.calls.len() - 1) {
            self.bytes[start] ^= 1;
        }
        if cut {
            self.failed = true;
            Err(Io::Cut)
        } else {
            Ok(())
        }
    }
    fn erase(&mut self, from: u32, to: u32) -> Result<(), Io> {
        if !(from as usize).is_multiple_of(SECTOR)
            || !(to as usize).is_multiple_of(SECTOR)
            || to < from
            || to as usize > self.bytes.len()
        {
            return Err(Io::Contract);
        }
        let (n, cut) = self.count(
            Call::Erase(from, (to - from) as usize),
            (to - from) as usize,
        )?;
        self.bytes[from as usize..from as usize + n].fill(0xff);
        if cut {
            self.failed = true;
            Err(Io::Cut)
        } else {
            Ok(())
        }
    }
}
struct Keys;
impl KeyProvider for Keys {
    type Error = ();
    fn wrapping_key(&mut self, out: &mut [u8; 32]) -> Result<(), ()> {
        *out = [0x72; 32];
        Ok(())
    }
}
struct Random(u64);
impl Entropy for Random {
    type Error = ();
    fn fill(&mut self, out: &mut [u8]) -> Result<(), ()> {
        for b in out {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            *b = self.0 as u8;
        }
        Ok(())
    }
}
type Store = SecureBackend<Flash, Keys, Random>;
fn incarnation() -> StoreIncarnation {
    StoreIncarnation::new([0x91; 24]).unwrap()
}
fn limits() -> SecureLimits {
    SecureLimits {
        capsule_sectors: 3,
        max_live_objects: 12,
        max_catalog_bytes: 4096,
        max_key_bytes: 64,
        max_value_bytes: 2048,
        max_changes: 12,
        max_receipts: 12,
        reserve_bytes: 8192,
    }
}
fn ticket(n: u64) -> OperationTicket {
    OperationTicket::new(incarnation(), n).unwrap()
}
fn op(n: u64) -> OperationId {
    ticket(n).operation_id()
}
fn fresh() -> Store {
    Store::format(
        Flash::fresh(),
        PART,
        limits(),
        incarnation(),
        Keys,
        random(),
    )
    .unwrap()
}
fn mount(flash: Flash) -> Result<Store, ErrorOf<Flash, Keys, Random>> {
    Store::mount(flash.boot(), PART, limits(), incarnation(), Keys, random())
}
fn put(n: u8) -> Mutation {
    Mutation::Put {
        key: b"state".to_vec(),
        value: vec![n; 513],
    }
}
fn check_one_capsule(store: &mut Store) {
    let current = store.root.unwrap().slot;
    for i in 0..3 {
        assert_eq!(
            store.erased(store.capsule_at(i), SECTOR).unwrap(),
            i != current
        );
    }
}

fn is_marker(call: &Call) -> bool {
    matches!(*call, Call::Write(at, BLOCK)
        if at >= PART.offset
        && at < PART.offset + (3 * SECTOR) as u32
        && at as usize % SECTOR == SECTOR - BLOCK)
}

/// Test-only forensic oracle: unwrap every physically surviving capsule and
/// catalog using the separately retained device key. Never logs key material.
fn audit_reachable_keys(store: &Store, retired_dek: &[u8], old_root: Root, old_catalog: &[u8]) {
    let mut surviving = 0;
    for slot in 0..3 {
        let at = store.capsule_at(slot) as usize;
        let sector = &store.flash.bytes[at..at + SECTOR];
        if sector.iter().all(|b| *b == 0xff) {
            continue;
        }
        surviving += 1;
        assert_eq!(&sector[SECTOR - BLOCK..], MARKER);
        let nonce: [u8; 24] = sector[40..64].try_into().unwrap();
        let descriptor = codec::open(
            &[0x72; 32],
            &nonce,
            &sector[..64],
            Zeroizing::new(sector[64..212].to_vec()),
        )
        .unwrap_or_else(|_| panic!("surviving capsule must authenticate"));
        let generation = u64::from_le_bytes(sector[32..40].try_into().unwrap());
        let location = u32::from_le_bytes(descriptor[36..40].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(descriptor[40..44].try_into().unwrap()) as usize;
        let catalog_nonce: [u8; 24] = descriptor[44..68].try_into().unwrap();
        let catalog_key = &descriptor[100..132];
        let plain = codec::open(
            catalog_key,
            &catalog_nonce,
            &codec::catalog_aad(incarnation(), generation, &store.config),
            Zeroizing::new(store.flash.bytes[location..location + length].to_vec()),
        )
        .unwrap_or_else(|_| panic!("reachable catalog must authenticate"));
        let reachable = codec::decode(&plain, limits(), incarnation())
            .unwrap_or_else(|_| panic!("reachable catalog must decode"));
        assert_eq!(reachable.objects.len(), store.catalog.objects.len());
        for object in &reachable.objects {
            assert_ne!(object.dek.as_slice(), retired_dek);
            assert!(store
                .catalog
                .objects
                .iter()
                .any(|live| live.key == object.key
                    && live.version == object.version
                    && live.dek == object.dek));
        }
        // The live capsule cannot open the retained prior catalog ciphertext.
        assert!(codec::open(
            catalog_key,
            &old_root.catalog_nonce,
            &codec::catalog_aad(incarnation(), old_root.generation, &store.config),
            Zeroizing::new(old_catalog.to_vec()),
        )
        .is_err());
    }
    assert_eq!(surviving, 1);
}

#[test]
fn fresh_versions_retire_capsules_and_gc_preserves_exact_ciphertext() {
    let mut store = fresh();
    store.commit(Revision(0), op(1), &[put(11)]).unwrap();
    let prior_key = store.catalog.objects[0].dek.to_vec();
    let prior_catalog = store.copy_catalog().unwrap();
    let prior_root = store.root.unwrap();
    let prior_catalog_cipher = store.flash.bytes
        [prior_root.catalog_at as usize..prior_root.catalog_at as usize + prior_root.catalog_bytes]
        .to_vec();
    let ciphertext = store
        .object_cipher(&prior_catalog.objects[0], prior_catalog.arena)
        .unwrap();
    store.commit(Revision(1), op(2), &[put(22)]).unwrap();
    assert_ne!(store.catalog.objects[0].dek.as_slice(), prior_key);
    check_one_capsule(&mut store);
    audit_reachable_keys(&store, &prior_key, prior_root, &prior_catalog_cipher);
    // Historical key capture is explicitly outside the erasure guarantee:
    // archived ciphertext still opens with its independently archived old DEK.
    let aad = codec::object_aad(incarnation(), &prior_catalog.objects[0])
        .unwrap_or_else(|_| panic!("old object AAD"));
    let archived_plain = codec::open(
        &prior_key,
        &prior_catalog.objects[0].nonce,
        &aad,
        Zeroizing::new(ciphertext.to_vec()),
    )
    .unwrap_or_else(|_| panic!("archived key oracle"));
    assert_eq!(archived_plain.as_slice(), [11; 513]);
    assert!(!store.flash.bytes.windows(513).any(|w| w == [22; 513]));
    let current = store.copy_catalog().unwrap();
    let exact = store
        .object_cipher(&current.objects[0], current.arena)
        .unwrap();
    store.maintain(SecureRequest::Compact).unwrap();
    let after = store.copy_catalog().unwrap();
    assert_eq!(
        exact,
        store.object_cipher(&after.objects[0], after.arena).unwrap()
    );
    assert_eq!(store.read(b"state", 513).unwrap(), Some(vec![22; 513]));
    assert_eq!(store.head().unwrap(), Revision(2));
    assert_ne!(ciphertext, exact);
    check_one_capsule(&mut store);
    audit_reachable_keys(&store, &prior_key, prior_root, &prior_catalog_cipher);
}

#[test]
fn each_program_and_erase_boundary_recovers_exact_committed_or_old_state() {
    let mut original = fresh();
    original.commit(Revision(0), op(1), &[put(11)]).unwrap();
    let baseline = original.into_parts().0.boot();
    let mut success = mount(baseline.clone()).unwrap();
    success.commit(Revision(1), op(2), &[put(22)]).unwrap();
    let trace = success.into_parts().0.calls;
    for (call, event) in trace.iter().enumerate() {
        let (size, marker) = match *event {
            Call::Write(offset, len) => (len, offset as usize % SECTOR == SECTOR - BLOCK),
            Call::Erase(offset, len) => {
                assert!(offset >= PART.offset);
                (len, false)
            }
        };
        for cut_bytes in [0, 1, 127, 255, size / 2, size - 1, size] {
            let mut backend = mount(baseline.clone()).unwrap();
            backend.flash.cut = Some((call, cut_bytes));
            assert!(backend.commit(Revision(1), op(2), &[put(22)]).is_err());
            assert!(
                backend.head().is_err(),
                "strict gate after cut {call}/{cut_bytes}"
            );
            let reopened = mount(backend.into_parts().0);
            if marker && cut_bytes > 0 && cut_bytes < size {
                assert!(reopened.is_err(), "partial marker must fail closed");
                continue;
            }
            let mut reopened =
                reopened.unwrap_or_else(|e| panic!("cut {call}/{cut_bytes} {event:?}: {e:?}"));
            let revision = reopened.head().unwrap();
            assert!(revision == Revision(1) || revision == Revision(2));
            let expected = if revision == Revision(2) { 22 } else { 11 };
            assert_eq!(
                reopened.read(b"state", 513).unwrap(),
                Some(vec![expected; 513])
            );
            match reopened.recover(op(2)).unwrap() {
                RecoveryOutcome::Committed(r) => assert_eq!(r.revision, Revision(2)),
                RecoveryOutcome::NotCommitted { revision } => assert_eq!(revision, Revision(1)),
                other => panic!("unexpected recovery {other:?}"),
            }
            check_one_capsule(&mut reopened);
        }
    }
}

#[test]
fn damaged_published_successor_never_falls_back_while_parent_coexists() {
    let mut initial = fresh();
    initial.commit(Revision(0), op(1), &[put(11)]).unwrap();
    let baseline = initial.into_parts().0.boot();
    let mut sample = mount(baseline.clone()).unwrap();
    sample.commit(Revision(1), op(2), &[put(22)]).unwrap();
    let trace = sample.into_parts().0.calls;
    let erase = trace
        .iter()
        .position(|e| matches!(e, Call::Erase(_, _)))
        .unwrap();
    for erased in [0, 1, 128, 2048, SECTOR - 1] {
        let mut store = mount(baseline.clone()).unwrap();
        store.flash.cut = Some((erase, erased));
        assert!(store.commit(Revision(1), op(2), &[put(22)]).is_err());
        let mut flash = store.into_parts().0.boot();
        let candidate = (PART.offset as usize) + 2 * SECTOR;
        flash.bytes[candidate + 80] ^= 1;
        assert!(
            mount(flash).is_err(),
            "must not recover old ratchet after erase prefix={erased}"
        );
    }
}

#[test]
fn receipt_floor_and_ring_reuse_preserve_bounds() {
    let mut store = fresh();
    for n in 1..=40 {
        assert!(matches!(
            store
                .commit(Revision(n - 1), op(n), &[put(n as u8)])
                .unwrap(),
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            store
                .maintain(SecureRequest::FinalizeThrough(ticket(n)))
                .unwrap(),
            MaintenanceOutcome::Complete(_)
        ));
        assert!(matches!(
            store.recover(op(n)).unwrap(),
            RecoveryOutcome::Retired { .. }
        ));
        assert!(matches!(
            store.commit(Revision(n), op(n), &[put(99)]).unwrap(),
            CommitOutcome::Rejected(OperationRejection::RetiredTicket)
        ));
        if n % 3 == 0 {
            store.maintain(SecureRequest::Compact).unwrap();
        }
        check_one_capsule(&mut store);
    }
    assert_eq!(store.head().unwrap(), Revision(40));
    assert_eq!(store.usage().receipts, 0);
}

#[test]
fn all_live_objects_are_authenticated_before_releasing_a_new_commit() {
    let mut store = fresh();
    store.commit(Revision(0), op(1), &[put(11)]).unwrap();
    let at = store.catalog.objects[0].location as usize;
    store.flash.bytes[at] ^= 1;
    assert!(store
        .commit(
            Revision(1),
            op(2),
            &[Mutation::Put {
                key: b"other".to_vec(),
                value: vec![4]
            }]
        )
        .is_err());
    assert!(store.head().is_err());
    assert!(mount(store.into_parts().0).is_err());
}

#[test]
fn compact_and_finalize_survive_program_and_partial_erase_cuts() {
    for request in [
        SecureRequest::Compact,
        SecureRequest::FinalizeThrough(ticket(1)),
    ] {
        let mut store = fresh();
        store.commit(Revision(0), op(1), &[put(11)]).unwrap();
        let baseline = store.into_parts().0.boot();
        let mut success = mount(baseline.clone()).unwrap();
        success.maintain(request).unwrap();
        let trace = success.into_parts().0.calls;
        let publication = trace.iter().position(is_marker).unwrap();
        for (call, event) in trace.iter().enumerate() {
            let (size, marker) = match *event {
                Call::Write(offset, len) => (len, offset as usize % SECTOR == SECTOR - BLOCK),
                Call::Erase(_, len) => (len, false),
            };
            for count in [0, 1, size / 2, size - 1, size] {
                let mut store = mount(baseline.clone()).unwrap();
                store.flash.cut = Some((call, count));
                assert!(store.maintain(request).is_err());
                assert!(store.head().is_err());
                let recovered = mount(store.into_parts().0);
                if marker && count > 0 && count < size {
                    assert!(recovered.is_err());
                    continue;
                }
                let mut recovered = recovered.unwrap_or_else(|e| {
                    panic!("{request:?} cut={call}/{count} event={event:?}: {e:?}")
                });
                recovered.recover_maintenance().unwrap();
                assert_eq!(recovered.head().unwrap(), Revision(1));
                assert_eq!(recovered.read(b"state", 513).unwrap(), Some(vec![11; 513]));
                let published = call > publication || (call == publication && count == size);
                let finalized = request == SecureRequest::FinalizeThrough(ticket(1)) && published;
                assert_eq!(recovered.usage().retired_through, u64::from(finalized));
                match recovered.recover(op(1)).unwrap() {
                    RecoveryOutcome::Committed(receipt) if !finalized => {
                        assert_eq!(receipt.revision, Revision(1));
                    }
                    RecoveryOutcome::Retired {
                        revision,
                        retired_through,
                    } if finalized => {
                        assert_eq!(revision, Revision(1));
                        assert_eq!(retired_through, ticket(1));
                    }
                    other => panic!("incorrect recovery after publication={published}: {other:?}"),
                }
                let unused = recovered.arena_start[1 - recovered.catalog.arena];
                assert!(recovered.erased(unused, recovered.arena_bytes).unwrap());
                check_one_capsule(&mut recovered);
            }
        }
    }
}

#[test]
fn a_silently_corrupt_new_blob_is_rejected_before_publication() {
    let mut store = fresh();
    store.commit(Revision(0), op(1), &[put(11)]).unwrap();
    store.flash.calls.clear();
    store.flash.corrupt_write = Some(0);
    assert!(store.commit(Revision(1), op(2), &[put(22)]).is_err());
    assert!(!store.flash.calls.iter().any(is_marker));
    let mut recovered = mount(store.into_parts().0).unwrap();
    assert_eq!(recovered.head().unwrap(), Revision(1));
    assert_eq!(recovered.read(b"state", 513).unwrap(), Some(vec![11; 513]));
    assert!(matches!(
        recovered.recover(op(2)).unwrap(),
        RecoveryOutcome::NotCommitted {
            revision: Revision(1)
        }
    ));
}

#[test]
fn a_retried_unpublished_revision_has_a_fresh_capsule_nonce() {
    let mut store = fresh();
    store.commit(Revision(0), op(1), &[put(11)]).unwrap();
    let baseline = store.into_parts().0.boot();
    let mut sample = mount(baseline.clone()).unwrap();
    sample.commit(Revision(1), op(2), &[put(22)]).unwrap();
    let trace = sample.into_parts().0.calls;
    let publish = trace
        .iter()
        .position(|call| matches!(*call,Call::Write(at,BLOCK) if at as usize%SECTOR==SECTOR-BLOCK))
        .unwrap();
    let mut cut = mount(baseline).unwrap();
    cut.flash.cut = Some((publish, 0));
    assert!(cut.commit(Revision(1), op(2), &[put(22)]).is_err());
    let flash = cut.into_parts().0;
    let at = PART.offset as usize + 2 * SECTOR;
    let before = flash.bytes[at + 40..at + 64].to_vec();
    let mut recovered = mount(flash).unwrap();
    recovered.commit(Revision(1), op(2), &[put(22)]).unwrap();
    assert_ne!(recovered.flash.bytes[at + 40..at + 64], before);
}
