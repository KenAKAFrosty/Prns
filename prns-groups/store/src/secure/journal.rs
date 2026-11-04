use alloc::vec::Vec;
use embedded_storage::nor_flash::NorFlash;
use zeroize::Zeroizing;

use super::{
    codec, Catalog, Entropy, ErrorOf, KeyProvider, Object, Root, SecureBackend, SecureError,
    SecureLimits, SecureRequest, SecureUsage, BLOCK, MAGIC, MARKER, NO_PARENT,
};
use crate::{
    flash::Partition, AtomicBackend, CommitOutcome, CommitReceipt, MaintenanceBackend,
    MaintenanceOutcome, Mutation, OperationId, OperationRejection, OperationTicket,
    RecoveryOutcome, Revision, StoreIncarnation,
};

fn mapped<F, K, R>(error: codec::Error) -> SecureError<F, K, R> {
    match error {
        codec::Error::Corrupt => SecureError::Corrupt,
        codec::Error::Capacity => SecureError::Capacity,
        codec::Error::Allocation => SecureError::Allocation,
        codec::Error::Authentication => SecureError::Authentication,
    }
}
fn aligned(bytes: usize) -> Option<usize> {
    bytes.checked_add(BLOCK - 1).map(|n| n / BLOCK * BLOCK)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapsuleState {
    Erased,
    Unpublished,
    Ambiguous,
    Published { generation: u64 },
}

impl<F: NorFlash, K: KeyProvider, R: Entropy> SecureBackend<F, K, R> {
    fn construct(
        flash: F,
        partition: Partition,
        limits: SecureLimits,
        incarnation: StoreIncarnation,
        mut key_provider: K,
        entropy: R,
    ) -> Result<Self, ErrorOf<F, K, R>> {
        let start = partition.offset as usize;
        let length = partition.length as usize;
        let capsule_bytes = limits
            .capsule_sectors
            .checked_mul(F::ERASE_SIZE)
            .ok_or(SecureError::Geometry)?;
        if F::READ_SIZE == 0
            || F::WRITE_SIZE == 0
            || F::ERASE_SIZE < 2 * BLOCK
            || !BLOCK.is_multiple_of(F::READ_SIZE)
            || !BLOCK.is_multiple_of(F::WRITE_SIZE)
            || !F::ERASE_SIZE.is_multiple_of(BLOCK)
            || !start.is_multiple_of(F::ERASE_SIZE)
            || !length.is_multiple_of(F::ERASE_SIZE)
            || limits.capsule_sectors < 3
            || start
                .checked_add(length)
                .is_none_or(|end| end > flash.capacity() || end > u32::MAX as usize)
            || capsule_bytes >= length
            || !(length - capsule_bytes).is_multiple_of(2 * F::ERASE_SIZE)
            || limits.max_catalog_bytes < 36
            || limits.max_catalog_bytes > u32::MAX as usize - 16
            || limits.max_key_bytes > u32::MAX as usize
            || limits.max_value_bytes > u32::MAX as usize - 16
            || limits.max_live_objects > u32::MAX as usize
            || limits.max_receipts > u32::MAX as usize
            || limits.max_changes > u32::MAX as usize
        {
            return Err(SecureError::Geometry);
        }
        let arena_bytes = (length - capsule_bytes) / 2;
        if limits.reserve_bytes
            < aligned(limits.max_catalog_bytes + 16).ok_or(SecureError::Geometry)?
            || aligned(limits.max_catalog_bytes + 16)
                .and_then(|n| n.checked_add(limits.reserve_bytes))
                .is_none_or(|n| n > arena_bytes)
        {
            return Err(SecureError::Geometry);
        }
        let mut config_bytes = codec::buffer(112).map_err(mapped)?;
        for value in [
            start,
            length,
            F::ERASE_SIZE,
            F::READ_SIZE,
            F::WRITE_SIZE,
            limits.capsule_sectors,
            limits.max_live_objects,
            limits.max_catalog_bytes,
            limits.max_key_bytes,
            limits.max_value_bytes,
            limits.max_changes,
            limits.max_receipts,
            limits.reserve_bytes,
        ] {
            config_bytes.extend_from_slice(&(value as u64).to_le_bytes());
        }
        let config = codec::hash(&config_bytes);
        let mut wrapping = Zeroizing::new([0; 32]);
        key_provider
            .wrapping_key(&mut wrapping)
            .map_err(SecureError::Key)?;
        Ok(Self {
            flash,
            key_provider,
            entropy,
            partition,
            limits,
            incarnation,
            config,
            wrapping,
            root: None,
            catalog: Catalog {
                revision: Revision(0),
                floor: 0,
                arena: 0,
                objects: Vec::new(),
                receipts: Vec::new(),
            },
            arena_start: [
                (start + capsule_bytes) as u32,
                (start + capsule_bytes + arena_bytes) as u32,
            ],
            arena_bytes,
            cursor: 0,
            dirty: true,
        })
    }

    /// Initialize only an entirely erased, exclusively owned partition. Never
    /// substitutes for mount and never erases existing evidence on failure.
    pub fn format(
        flash: F,
        partition: Partition,
        limits: SecureLimits,
        incarnation: StoreIncarnation,
        key_provider: K,
        entropy: R,
    ) -> Result<Self, ErrorOf<F, K, R>> {
        let mut backend =
            Self::construct(flash, partition, limits, incarnation, key_provider, entropy)?;
        if !backend.erased(partition.offset, partition.length as usize)? {
            return Err(SecureError::NotErased);
        }
        let catalog = backend.copy_catalog()?;
        backend.publish(catalog)?;
        Ok(backend)
    }

    pub fn mount(
        flash: F,
        partition: Partition,
        limits: SecureLimits,
        incarnation: StoreIncarnation,
        key_provider: K,
        entropy: R,
    ) -> Result<Self, ErrorOf<F, K, R>> {
        let mut backend =
            Self::construct(flash, partition, limits, incarnation, key_provider, entropy)?;
        backend.reopen()?;
        Ok(backend)
    }

    pub(super) fn capsule_at(&self, slot: usize) -> u32 {
        self.partition.offset + (slot * F::ERASE_SIZE) as u32
    }
    fn random(&mut self, bytes: &mut [u8]) -> Result<(), ErrorOf<F, K, R>> {
        self.entropy.fill(bytes).map_err(SecureError::Entropy)
    }
    fn refresh(&mut self) -> Result<(), ErrorOf<F, K, R>> {
        if self.dirty {
            self.reopen()?;
        }
        Ok(())
    }

    fn read_bytes(
        &mut self,
        start: u32,
        length: usize,
    ) -> Result<Zeroizing<Vec<u8>>, ErrorOf<F, K, R>> {
        let mut bytes = codec::buffer(length).map_err(mapped)?;
        let mut block = Zeroizing::new([0; BLOCK]);
        for offset in (0..length).step_by(BLOCK) {
            self.flash
                .read(start + offset as u32, &mut *block)
                .map_err(SecureError::Flash)?;
            bytes.extend_from_slice(&block[..BLOCK.min(length - offset)]);
        }
        Ok(bytes)
    }
    fn write_bytes(&mut self, start: u32, bytes: &[u8]) -> Result<(), ErrorOf<F, K, R>> {
        let mut block = Zeroizing::new([0xff; BLOCK]);
        for (index, chunk) in bytes.chunks(BLOCK).enumerate() {
            block.fill(0xff);
            block[..chunk.len()].copy_from_slice(chunk);
            self.flash
                .write(start + (index * BLOCK) as u32, &*block)
                .map_err(SecureError::Flash)?;
        }
        Ok(())
    }
    pub(super) fn erased(&mut self, start: u32, length: usize) -> Result<bool, ErrorOf<F, K, R>> {
        let mut block = Zeroizing::new([0; BLOCK]);
        for offset in (0..length).step_by(BLOCK) {
            self.flash
                .read(start + offset as u32, &mut *block)
                .map_err(SecureError::Flash)?;
            if block.iter().any(|b| *b != 0xff) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn erase_verified(&mut self, start: u32, length: usize) -> Result<(), ErrorOf<F, K, R>> {
        for offset in (0..length).step_by(F::ERASE_SIZE) {
            let at = start + offset as u32;
            if !self.erased(at, F::ERASE_SIZE)? {
                self.flash
                    .erase(at, at + F::ERASE_SIZE as u32)
                    .map_err(SecureError::Flash)?;
                if !self.erased(at, F::ERASE_SIZE)? {
                    return Err(SecureError::Corrupt);
                }
            }
        }
        Ok(())
    }
    pub(super) fn copy_catalog(&self) -> Result<Catalog, ErrorOf<F, K, R>> {
        let bytes = codec::encode(&self.catalog, self.limits).map_err(mapped)?;
        codec::decode(&bytes, self.limits, self.incarnation).map_err(mapped)
    }

    fn capsule(&self, bytes: &[u8], slot: usize) -> Result<super::OpenedCapsule, ErrorOf<F, K, R>> {
        if bytes.len() != BLOCK
            || &bytes[..8] != MAGIC
            || &bytes[8..32] != self.incarnation.as_bytes()
        {
            return Err(SecureError::Corrupt);
        }
        let generation =
            u64::from_le_bytes(bytes[32..40].try_into().map_err(|_| SecureError::Corrupt)?);
        if generation == 0 || (generation - 1) % self.limits.capsule_sectors as u64 != slot as u64 {
            return Err(SecureError::Corrupt);
        }
        let nonce: [u8; 24] = bytes[40..64].try_into().map_err(|_| SecureError::Corrupt)?;
        let mut encrypted = codec::buffer(148).map_err(mapped)?;
        encrypted.extend_from_slice(&bytes[64..212]);
        let plain =
            codec::open(self.wrapping.as_ref(), &nonce, &bytes[..64], encrypted).map_err(mapped)?;
        if plain[..32] != self.config {
            return Err(SecureError::Configuration);
        }
        let u32_at =
            |at| u32::from_le_bytes([plain[at], plain[at + 1], plain[at + 2], plain[at + 3]]);
        let parent = u32_at(32);
        let expected_parent = if generation == 1 {
            NO_PARENT
        } else {
            ((slot + self.limits.capsule_sectors - 1) % self.limits.capsule_sectors) as u32
        };
        if parent != expected_parent {
            return Err(SecureError::Corrupt);
        }
        let root = Root {
            generation,
            slot,
            parent,
            catalog_at: u32_at(36),
            catalog_bytes: u32_at(40) as usize,
            catalog_nonce: plain[44..68].try_into().map_err(|_| SecureError::Corrupt)?,
            digest: plain[68..100]
                .try_into()
                .map_err(|_| SecureError::Corrupt)?,
        };
        Ok((
            root,
            Zeroizing::new(
                plain[100..132]
                    .try_into()
                    .map_err(|_| SecureError::Corrupt)?,
            ),
        ))
    }

    pub(super) fn object_cipher(
        &mut self,
        object: &Object,
        arena: usize,
    ) -> Result<Zeroizing<Vec<u8>>, ErrorOf<F, K, R>> {
        let length = object.length.checked_add(16).ok_or(SecureError::Corrupt)?;
        self.check_range(object.location, length, arena)?;
        let bytes = self.read_bytes(object.location, length)?;
        if codec::hash(&bytes) != object.digest {
            return Err(SecureError::Corrupt);
        }
        Ok(bytes)
    }
    fn object_plain(
        &mut self,
        object: &Object,
        arena: usize,
    ) -> Result<Zeroizing<Vec<u8>>, ErrorOf<F, K, R>> {
        let bytes = self.object_cipher(object, arena)?;
        let aad = codec::object_aad(self.incarnation, object).map_err(mapped)?;
        codec::open(&object.dek, &object.nonce, &aad, bytes).map_err(mapped)
    }
    fn check_range(&self, at: u32, bytes: usize, arena: usize) -> Result<(), ErrorOf<F, K, R>> {
        if arena > 1
            || !(at as usize).is_multiple_of(BLOCK)
            || at < self.arena_start[arena]
            || aligned(bytes)
                .and_then(|n| (at as usize).checked_add(n))
                .is_none_or(|end| end > self.arena_start[arena] as usize + self.arena_bytes)
        {
            return Err(SecureError::Corrupt);
        }
        Ok(())
    }

    fn reopen(&mut self) -> Result<(), ErrorOf<F, K, R>> {
        self.dirty = true;
        let mut states = Vec::new();
        states
            .try_reserve_exact(self.limits.capsule_sectors)
            .map_err(|_| SecureError::Allocation)?;
        let mut selected: Option<(Root, Zeroizing<[u8; 32]>)> = None;
        for slot in 0..self.limits.capsule_sectors {
            let at = self.capsule_at(slot);
            let marker = self.read_bytes(at + F::ERASE_SIZE as u32 - BLOCK as u32, BLOCK)?;
            let state = if marker.as_slice() == MARKER {
                let body = self.read_bytes(at, BLOCK)?;
                match self.capsule(&body, slot) {
                    Ok(candidate) => {
                        let generation = candidate.0.generation;
                        if selected
                            .as_ref()
                            .is_none_or(|current| generation > current.0.generation)
                        {
                            selected = Some(candidate);
                        }
                        CapsuleState::Published { generation }
                    }
                    Err(_) => CapsuleState::Ambiguous,
                }
            } else if marker.iter().all(|b| *b == 0xff) {
                if self.erased(at, F::ERASE_SIZE)? {
                    CapsuleState::Erased
                } else {
                    CapsuleState::Unpublished
                }
            } else {
                CapsuleState::Ambiguous
            };
            states.push(state);
        }
        let Some((root, catalog_key)) = selected else {
            return Err(if states.iter().all(|s| *s == CapsuleState::Erased) {
                SecureError::Empty
            } else {
                SecureError::Corrupt
            });
        };
        let next = (root.slot + 1) % self.limits.capsule_sectors;
        for (slot, state) in states.iter().enumerate() {
            if slot == root.slot || *state == CapsuleState::Erased {
                continue;
            }
            if slot as u32 == root.parent {
                if let CapsuleState::Published { generation } = state {
                    if generation.checked_add(1) != Some(root.generation) {
                        return Err(SecureError::Corrupt);
                    }
                }
            } else if slot != next || *state != CapsuleState::Unpublished {
                return Err(SecureError::Corrupt);
            }
        }
        if root.catalog_bytes < 16 || root.catalog_bytes > self.limits.max_catalog_bytes + 16 {
            return Err(SecureError::Corrupt);
        }
        let arena = self
            .arena_start
            .iter()
            .position(|start| {
                root.catalog_at >= *start
                    && (root.catalog_at as usize) < *start as usize + self.arena_bytes
            })
            .ok_or(SecureError::Corrupt)?;
        self.check_range(root.catalog_at, root.catalog_bytes, arena)?;
        let ciphertext = self.read_bytes(root.catalog_at, root.catalog_bytes)?;
        if codec::hash(&ciphertext) != root.digest {
            return Err(SecureError::Corrupt);
        }
        let aad = codec::catalog_aad(self.incarnation, root.generation, &self.config);
        let plain = codec::open(catalog_key.as_ref(), &root.catalog_nonce, &aad, ciphertext)
            .map_err(mapped)?;
        let catalog = codec::decode(&plain, self.limits, self.incarnation).map_err(mapped)?;
        if catalog.arena != arena {
            return Err(SecureError::Corrupt);
        }
        for object in &catalog.objects {
            self.object_plain(object, arena)?;
        }
        // A minimum three-slot ring keeps the next candidate distinct from the
        // authenticated predecessor-cleanup target even after slot reuse.
        for (slot, state) in states.iter().enumerate() {
            if slot != root.slot && *state != CapsuleState::Erased {
                self.erase_verified(self.capsule_at(slot), F::ERASE_SIZE)?;
            }
        }
        let mut cursor = 0;
        for offset in (0..self.arena_bytes).step_by(BLOCK) {
            if !self.erased(self.arena_start[arena] + offset as u32, BLOCK)? {
                cursor = offset + BLOCK;
            }
        }
        self.root = Some(root);
        self.catalog = catalog;
        self.cursor = cursor;
        self.dirty = false;
        Ok(())
    }

    fn publish(&mut self, catalog: Catalog) -> Result<(), ErrorOf<F, K, R>> {
        self.dirty = true;
        let generation = self
            .root
            .map_or(Some(1), |r| r.generation.checked_add(1))
            .ok_or(SecureError::CounterExhausted)?;
        let slot = ((generation - 1) % self.limits.capsule_sectors as u64) as usize;
        if !self.erased(self.capsule_at(slot), F::ERASE_SIZE)? {
            return Err(SecureError::Corrupt);
        }
        let mut key = Zeroizing::new([0; 32]);
        let mut nonce = [0; 24];
        self.random(&mut *key)?;
        self.random(&mut nonce)?;
        let encoded = codec::encode(&catalog, self.limits).map_err(mapped)?;
        let aad = codec::catalog_aad(self.incarnation, generation, &self.config);
        let encrypted = codec::seal(key.as_ref(), &nonce, &aad, &encoded).map_err(mapped)?;
        if self
            .cursor
            .checked_add(aligned(encrypted.len()).ok_or(SecureError::Capacity)?)
            .is_none_or(|end| end > self.arena_bytes)
        {
            return Err(SecureError::Capacity);
        }
        let at = self.arena_start[catalog.arena] + self.cursor as u32;
        self.write_bytes(at, &encrypted)?;
        let readback = self.read_bytes(at, encrypted.len())?;
        if readback != encrypted {
            return Err(SecureError::Corrupt);
        }
        for object in &catalog.objects {
            self.object_plain(object, catalog.arena)?;
        }
        let mut body = Zeroizing::new([0xff; BLOCK]);
        body[..8].copy_from_slice(MAGIC);
        body[8..32].copy_from_slice(self.incarnation.as_bytes());
        body[32..40].copy_from_slice(&generation.to_le_bytes());
        self.random(&mut body[40..64])?;
        let mut descriptor = Zeroizing::new([0; 132]);
        descriptor[..32].copy_from_slice(&self.config);
        descriptor[32..36]
            .copy_from_slice(&self.root.map_or(NO_PARENT, |r| r.slot as u32).to_le_bytes());
        descriptor[36..40].copy_from_slice(&at.to_le_bytes());
        descriptor[40..44].copy_from_slice(&(encrypted.len() as u32).to_le_bytes());
        descriptor[44..68].copy_from_slice(&nonce);
        descriptor[68..100].copy_from_slice(&codec::hash(&encrypted));
        descriptor[100..132].copy_from_slice(key.as_ref());
        let wrap_nonce: [u8; 24] = body[40..64].try_into().map_err(|_| SecureError::Corrupt)?;
        let wrapped = codec::seal(
            self.wrapping.as_ref(),
            &wrap_nonce,
            &body[..64],
            &*descriptor,
        )
        .map_err(mapped)?;
        body[64..212].copy_from_slice(&wrapped);
        let capsule_at = self.capsule_at(slot);
        self.write_bytes(capsule_at, &*body)?;
        if self.read_bytes(capsule_at, BLOCK)?.as_slice() != body.as_slice() {
            return Err(SecureError::Corrupt);
        }
        self.flash
            .write(capsule_at + F::ERASE_SIZE as u32 - BLOCK as u32, &MARKER)
            .map_err(SecureError::Flash)?;
        self.reopen()
    }

    fn compact(&mut self) -> Result<(), ErrorOf<F, K, R>> {
        self.refresh()?;
        let mut catalog = self.copy_catalog()?;
        let old = catalog.arena;
        let new = 1 - old;
        self.dirty = true;
        self.erase_verified(self.arena_start[new], self.arena_bytes)?;
        self.cursor = 0;
        for object in &mut catalog.objects {
            self.object_plain(object, old)?;
            let encrypted = self.object_cipher(object, old)?;
            let at = self.arena_start[new] + self.cursor as u32;
            self.write_bytes(at, &encrypted)?;
            object.location = at;
            self.cursor += aligned(encrypted.len()).ok_or(SecureError::Capacity)?;
        }
        catalog.arena = new;
        self.publish(catalog)?;
        self.dirty = true;
        self.erase_verified(self.arena_start[old], self.arena_bytes)?;
        self.dirty = false;
        Ok(())
    }

    fn clean_inactive(&mut self) -> Result<(), ErrorOf<F, K, R>> {
        self.dirty = true;
        self.erase_verified(self.arena_start[1 - self.catalog.arena], self.arena_bytes)?;
        self.dirty = false;
        Ok(())
    }

    fn ticket(&self, operation: OperationId) -> Result<OperationTicket, OperationRejection> {
        let ticket = OperationTicket::from_operation_id(operation)
            .map_err(|_| OperationRejection::InvalidTicket)?;
        if ticket.incarnation() != self.incarnation {
            return Err(OperationRejection::ForeignIncarnation);
        }
        if ticket.sequence() > self.catalog.revision.0.saturating_add(1) {
            return Err(OperationRejection::OutOfOrder {
                next: self
                    .catalog
                    .revision
                    .0
                    .checked_add(1)
                    .and_then(|seq| OperationTicket::new(self.incarnation, seq).ok()),
            });
        }
        Ok(ticket)
    }
}

impl<F: NorFlash, K: KeyProvider, R: Entropy> AtomicBackend for SecureBackend<F, K, R> {
    type Error = ErrorOf<F, K, R>;
    fn head(&mut self) -> Result<Revision, Self::Error> {
        self.refresh()?;
        Ok(self.catalog.revision)
    }
    fn read(&mut self, key: &[u8], max_bytes: usize) -> Result<Option<Vec<u8>>, Self::Error> {
        self.refresh()?;
        if key.len() > self.limits.max_key_bytes {
            return Err(SecureError::Capacity);
        }
        let catalog = self.copy_catalog()?;
        let Some(object) = catalog.objects.iter().find(|o| o.key.as_slice() == key) else {
            return Ok(None);
        };
        if object.length > max_bytes {
            return Err(SecureError::Capacity);
        }
        let result = self.object_plain(object, catalog.arena);
        if result.is_err() {
            self.dirty = true;
        }
        result.map(|mut bytes| Some(core::mem::take(&mut *bytes)))
    }
    fn recover(&mut self, operation: OperationId) -> Result<RecoveryOutcome, Self::Error> {
        self.reopen()?;
        let ticket = match self.ticket(operation) {
            Ok(ticket) => ticket,
            Err(reason) => return Ok(RecoveryOutcome::Rejected(reason)),
        };
        if ticket.sequence() <= self.catalog.floor {
            return Ok(RecoveryOutcome::Retired {
                revision: self.catalog.revision,
                retired_through: OperationTicket::new(self.incarnation, self.catalog.floor)
                    .map_err(|_| SecureError::Corrupt)?,
            });
        }
        Ok(
            match self
                .catalog
                .receipts
                .iter()
                .find(|r| r.operation_id == operation)
            {
                Some(receipt) => RecoveryOutcome::Committed(*receipt),
                None if self.catalog.receipts.len() == self.limits.max_receipts => {
                    RecoveryOutcome::Rejected(OperationRejection::RecoveryWindowFull)
                }
                None => RecoveryOutcome::NotCommitted {
                    revision: self.catalog.revision,
                },
            },
        )
    }
    fn commit(
        &mut self,
        expected: Revision,
        operation: OperationId,
        changes: &[Mutation],
    ) -> Result<CommitOutcome, Self::Error> {
        self.refresh()?;
        let ticket = match self.ticket(operation) {
            Ok(ticket) => ticket,
            Err(reason) => return Ok(CommitOutcome::Rejected(reason)),
        };
        if ticket.sequence() <= self.catalog.floor {
            return Ok(CommitOutcome::Rejected(OperationRejection::RetiredTicket));
        }
        if let Some(receipt) = self
            .catalog
            .receipts
            .iter()
            .find(|r| r.operation_id == operation)
        {
            return Ok(CommitOutcome::Committed(*receipt));
        }
        if expected != self.catalog.revision {
            return Ok(CommitOutcome::NotCommitted {
                revision: self.catalog.revision,
            });
        }
        if self.catalog.receipts.len() == self.limits.max_receipts {
            return Ok(CommitOutcome::Rejected(
                OperationRejection::RecoveryWindowFull,
            ));
        }
        if changes.len() > self.limits.max_changes {
            return Err(SecureError::Capacity);
        }
        let mut needed =
            aligned(self.limits.max_catalog_bytes + 16).ok_or(SecureError::Capacity)?;
        for change in changes {
            if change.key().len() > self.limits.max_key_bytes
                || change
                    .value()
                    .is_some_and(|v| v.len() > self.limits.max_value_bytes)
            {
                return Err(SecureError::Capacity);
            }
            if let Some(value) = change.value() {
                needed = needed
                    .checked_add(aligned(value.len() + 16).ok_or(SecureError::Capacity)?)
                    .ok_or(SecureError::Capacity)?;
            }
        }
        if self
            .cursor
            .checked_add(needed)
            .is_none_or(|end| end > self.arena_bytes)
        {
            self.compact()?;
        }
        if self
            .cursor
            .checked_add(needed)
            .is_none_or(|end| end > self.arena_bytes)
        {
            return Err(SecureError::Capacity);
        }
        let mut catalog = self.copy_catalog()?;
        for change in changes {
            let existing = catalog
                .objects
                .iter()
                .position(|o| o.key.as_slice() == change.key());
            if let Some(index) = existing {
                catalog.objects.remove(index);
            }
            if let Some(value) = change.value() {
                if catalog.objects.len() == self.limits.max_live_objects {
                    return Err(SecureError::Capacity);
                }
                catalog
                    .objects
                    .try_reserve_exact(1)
                    .map_err(|_| SecureError::Allocation)?;
                let mut key = codec::buffer(change.key().len()).map_err(mapped)?;
                key.extend_from_slice(change.key());
                let mut dek = codec::buffer(32).map_err(mapped)?;
                dek.resize(32, 0);
                let mut object = Object {
                    key,
                    version: [0; 16],
                    dek,
                    nonce: [0; 24],
                    location: 0,
                    length: value.len(),
                    digest: [0; 32],
                };
                self.random(&mut object.version)?;
                self.random(&mut object.dek)?;
                self.random(&mut object.nonce)?;
                catalog.objects.push(object);
            }
        }
        let revision = expected.next().ok_or(SecureError::CounterExhausted)?;
        let receipt = CommitReceipt {
            operation_id: operation,
            revision,
        };
        catalog.revision = revision;
        catalog
            .receipts
            .try_reserve_exact(1)
            .map_err(|_| SecureError::Allocation)?;
        catalog.receipts.push(receipt);
        let mut live = aligned(
            codec::encoded_size(&catalog)
                .map_err(mapped)?
                .checked_add(16)
                .ok_or(SecureError::Capacity)?,
        )
        .ok_or(SecureError::Capacity)?;
        for object in &catalog.objects {
            live = live
                .checked_add(aligned(object.length + 16).ok_or(SecureError::Capacity)?)
                .ok_or(SecureError::Capacity)?;
        }
        if live
            .checked_add(self.limits.reserve_bytes)
            .is_none_or(|n| n > self.arena_bytes)
        {
            return Err(SecureError::Capacity);
        }
        codec::encode(&catalog, self.limits).map_err(mapped)?;
        self.dirty = true;
        for object in &mut catalog.objects {
            if object.location != 0 {
                continue;
            }
            let value = changes
                .iter()
                .rev()
                .find(|c| c.key() == object.key.as_slice())
                .and_then(Mutation::value)
                .ok_or(SecureError::Corrupt)?;
            let aad = codec::object_aad(self.incarnation, object).map_err(mapped)?;
            let encrypted = codec::seal(&object.dek, &object.nonce, &aad, value).map_err(mapped)?;
            object.location = self.arena_start[catalog.arena] + self.cursor as u32;
            object.digest = codec::hash(&encrypted);
            self.write_bytes(object.location, &encrypted)?;
            self.cursor += aligned(encrypted.len()).ok_or(SecureError::Capacity)?;
        }
        self.publish(catalog)?;
        Ok(CommitOutcome::Committed(receipt))
    }
}

impl<F: NorFlash, K: KeyProvider, R: Entropy> MaintenanceBackend for SecureBackend<F, K, R> {
    type Request = SecureRequest;
    type Report = SecureUsage;
    fn maintain(
        &mut self,
        request: Self::Request,
    ) -> Result<MaintenanceOutcome<Self::Report>, Self::Error> {
        self.refresh()?;
        match request {
            SecureRequest::Compact => self.compact()?,
            SecureRequest::EraseRetired => {
                self.reopen()?;
                self.clean_inactive()?;
            }
            SecureRequest::FinalizeThrough(ticket) => {
                if ticket.incarnation() != self.incarnation
                    || ticket.sequence() > self.catalog.revision.0
                    || ticket.sequence() < self.catalog.floor
                {
                    return Ok(MaintenanceOutcome::Rejected(self.usage()));
                }
                if ticket.sequence() != self.catalog.floor {
                    if self
                        .cursor
                        .checked_add(
                            aligned(self.limits.max_catalog_bytes + 16)
                                .ok_or(SecureError::Capacity)?,
                        )
                        .is_none_or(|end| end > self.arena_bytes)
                    {
                        self.compact()?;
                    }
                    let mut catalog = self.copy_catalog()?;
                    catalog.floor = ticket.sequence();
                    catalog
                        .receipts
                        .retain(|r| r.revision.0 > ticket.sequence());
                    self.publish(catalog)?;
                }
            }
        }
        Ok(MaintenanceOutcome::Complete(self.usage()))
    }
    fn recover_maintenance(&mut self) -> Result<MaintenanceOutcome<Self::Report>, Self::Error> {
        self.reopen()?;
        self.clean_inactive()?;
        Ok(MaintenanceOutcome::Complete(self.usage()))
    }
}
