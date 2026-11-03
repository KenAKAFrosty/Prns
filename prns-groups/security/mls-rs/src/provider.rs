use crate::{Limit, Limits};
use alloc::{sync::Arc, vec::Vec};
use mls_rs_codec::{MlsDecode, MlsEncode};
use mls_rs_core::{
    error::IntoAnyError,
    group::{EpochRecord, GroupState, GroupStateStorage},
    key_package::{KeyPackageData, KeyPackageStorage},
};
use prns_groups_store::{AtomicBackend, StoreError, Transaction};
use spin::Mutex;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError {
    Capacity(Limit),
    StoreCapacity(prns_groups_core::CapacityError),
    CorruptState,
    Transaction,
}
impl IntoAnyError for ProviderError {}

pub(crate) fn key(prefix: u8, id: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(id.len() + 1);
    key.push(prefix);
    key.extend_from_slice(id);
    key
}
fn epoch_key(group: &[u8], epoch: u64) -> Vec<u8> {
    let mut key = key(b'E', group);
    key.extend_from_slice(&epoch.to_be_bytes());
    key
}

pub(crate) struct Providers<B> {
    pub tx: Transaction<B>,
    pub limits: Limits,
    fault: Arc<Mutex<Option<ProviderError>>>,
}
impl<B> Clone for Providers<B> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            limits: self.limits,
            fault: self.fault.clone(),
        }
    }
}
impl<B: AtomicBackend> Providers<B> {
    pub fn new(tx: Transaction<B>, limits: Limits) -> Self {
        Self {
            tx,
            limits,
            fault: Arc::new(Mutex::new(None)),
        }
    }
    pub fn fault(&self) -> Option<ProviderError> {
        *self.fault.lock()
    }
    fn checked<T>(&self, result: Result<T, ProviderError>) -> Result<T, ProviderError> {
        if let Err(error) = &result {
            *self.fault.lock() = Some(*error);
        }
        result
    }
    fn storage<T>(&self, result: Result<T, StoreError<B::Error>>) -> Result<T, ProviderError> {
        self.checked(result.map_err(|error| match error {
            StoreError::Capacity(error) => ProviderError::StoreCapacity(error),
            StoreError::AllocationFailed => ProviderError::Capacity(Limit::Allocation),
            _ => ProviderError::Transaction,
        }))
    }
    pub fn read(&self, key: &[u8]) -> Result<Option<Zeroizing<Vec<u8>>>, ProviderError> {
        self.storage(self.tx.read(key))
            .map(|value| value.map(Zeroizing::new))
    }
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<(), ProviderError> {
        self.storage(self.tx.put(key, value))
    }
    pub fn delete(&self, key: &[u8]) -> Result<(), ProviderError> {
        self.storage(self.tx.delete(key))
    }
    pub fn index(&self, name: &[u8], width: usize, max: usize) -> Result<Vec<u8>, ProviderError> {
        let value = self.storage(self.tx.read(name))?.unwrap_or_default();
        if value.len() % width != 0 || value.len() / width > max {
            return self.checked(Err(ProviderError::CorruptState));
        }
        // An index is canonical and duplicate-free; corrupt local metadata fails closed.
        if value.chunks_exact(width).enumerate().any(|(i, id)| {
            value[..i * width]
                .chunks_exact(width)
                .any(|prior| prior == id)
        }) {
            return self.checked(Err(ProviderError::CorruptState));
        }
        Ok(value)
    }
    pub fn add_index(
        &self,
        name: &[u8],
        id: &[u8],
        max: usize,
        limit: Limit,
    ) -> Result<(), ProviderError> {
        let mut index = self.index(name, id.len(), max)?;
        if index.chunks_exact(id.len()).any(|item| item == id) {
            return Ok(());
        }
        if index.len() / id.len() >= max {
            return self.checked(Err(ProviderError::Capacity(limit)));
        }
        index
            .try_reserve_exact(id.len())
            .map_err(|_| ProviderError::Capacity(Limit::Allocation))?;
        index.extend_from_slice(id);
        self.put(name, &index)
    }
    pub fn remove_index(&self, name: &[u8], id: &[u8], max: usize) -> Result<(), ProviderError> {
        let mut index = self.index(name, id.len(), max)?;
        if let Some(position) = index.chunks_exact(id.len()).position(|item| item == id) {
            index.drain(position * id.len()..(position + 1) * id.len());
        }
        self.put(name, &index)
    }
    pub fn purge_epochs(&self, group: &[u8]) -> Result<(), ProviderError> {
        let name = key(b'H', group);
        let index = self.index(&name, 8, self.limits.max_retained_epochs)?;
        for bytes in index.as_chunks::<8>().0.iter() {
            let epoch = u64::from_be_bytes(*bytes);
            self.delete(&epoch_key(group, epoch))?;
        }
        self.put(&name, &[])
    }
}
impl<B: AtomicBackend + Send> GroupStateStorage for Providers<B> {
    type Error = ProviderError;
    fn state(&self, group: &[u8]) -> Result<Option<Zeroizing<Vec<u8>>>, Self::Error> {
        if group.len() != 32 {
            return self.checked(Err(ProviderError::CorruptState));
        }
        let value = self.read(&key(b'G', group))?;
        if value
            .as_ref()
            .is_some_and(|value| value.len() > self.limits.max_snapshot_bytes)
        {
            return self.checked(Err(ProviderError::Capacity(Limit::SnapshotBytes)));
        }
        Ok(value)
    }
    fn epoch(&self, group: &[u8], epoch: u64) -> Result<Option<Zeroizing<Vec<u8>>>, Self::Error> {
        let index = self.index(&key(b'H', group), 8, self.limits.max_retained_epochs)?;
        if !index
            .as_chunks::<8>()
            .0
            .iter()
            .any(|value| *value == epoch.to_be_bytes())
        {
            return Ok(None);
        }
        let value = self.read(&epoch_key(group, epoch))?;
        if value
            .as_ref()
            .is_some_and(|value| value.len() > self.limits.max_snapshot_bytes)
        {
            return self.checked(Err(ProviderError::Capacity(Limit::SnapshotBytes)));
        }
        Ok(value)
    }
    fn write(
        &mut self,
        state: GroupState,
        inserts: Vec<EpochRecord>,
        updates: Vec<EpochRecord>,
    ) -> Result<(), Self::Error> {
        if state.id.len() != 32 {
            return self.checked(Err(ProviderError::CorruptState));
        }
        if state.data.len() > self.limits.max_snapshot_bytes {
            return self.checked(Err(ProviderError::Capacity(Limit::SnapshotBytes)));
        }
        self.put(&key(b'G', &state.id), &state.data)?;
        let name = key(b'H', &state.id);
        let index = self.index(&name, 8, self.limits.max_retained_epochs)?;
        let mut epochs: Vec<u64> = index
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bytes| {
                let mut value = [0; 8];
                value.copy_from_slice(bytes);
                u64::from_be_bytes(value)
            })
            .collect();
        for record in inserts.into_iter().chain(updates) {
            if record.data.len() > self.limits.max_snapshot_bytes {
                return self.checked(Err(ProviderError::Capacity(Limit::SnapshotBytes)));
            }
            // Capacity is reclaimed only from the oldest epoch. Removal barriers
            // separately purge every previous epoch in the enclosing transaction.
            if !epochs.contains(&record.id) {
                epochs.push(record.id);
                epochs.sort_unstable();
            }
            while epochs.len() > self.limits.max_retained_epochs {
                let old = epochs.remove(0);
                // `self` is mutable here: an unqualified call would resolve to
                // KeyPackageStorage::delete and interpret this epoch key as a
                // key-package identifier instead of deleting the stored value.
                Providers::delete(self, &epoch_key(&state.id, old))?;
            }
            if epochs.contains(&record.id) {
                self.put(&epoch_key(&state.id, record.id), &record.data)?;
            }
        }
        let mut encoded = Vec::with_capacity(epochs.len() * 8);
        for epoch in epochs {
            encoded.extend_from_slice(&epoch.to_be_bytes());
        }
        self.put(&name, &encoded)
    }
    fn max_epoch_id(&self, group: &[u8]) -> Result<Option<u64>, Self::Error> {
        let index = self.index(&key(b'H', group), 8, self.limits.max_retained_epochs)?;
        index
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bytes| Ok::<u64, ProviderError>(u64::from_be_bytes(*bytes)))
            .collect::<Result<Vec<_>, _>>()
            .map(|epochs| epochs.into_iter().max())
    }
}
impl<B: AtomicBackend + Send> KeyPackageStorage for Providers<B> {
    type Error = ProviderError;
    fn delete(&mut self, id: &[u8]) -> Result<(), Self::Error> {
        if id.len() != 32 {
            return self.checked(Err(ProviderError::CorruptState));
        }
        Providers::delete(self, &key(b'K', id))?;
        self.remove_index(b"key-packages", id, self.limits.max_key_packages)
    }
    fn insert(&mut self, id: Vec<u8>, package: KeyPackageData) -> Result<(), Self::Error> {
        if id.len() != 32 {
            return self.checked(Err(ProviderError::CorruptState));
        }
        let data = Zeroizing::new(
            package
                .mls_encode_to_vec()
                .map_err(|_| ProviderError::CorruptState)?,
        );
        if data.len() > self.limits.max_snapshot_bytes {
            return self.checked(Err(ProviderError::Capacity(Limit::SnapshotBytes)));
        }
        self.add_index(
            b"key-packages",
            &id,
            self.limits.max_key_packages,
            Limit::KeyPackages,
        )?;
        self.put(&key(b'K', &id), &data)
    }
    fn get(&self, id: &[u8]) -> Result<Option<KeyPackageData>, Self::Error> {
        let Some(data) = self.read(&key(b'K', id))? else {
            return Ok(None);
        };
        if data.len() > self.limits.max_snapshot_bytes {
            return self.checked(Err(ProviderError::Capacity(Limit::SnapshotBytes)));
        }
        let mut cursor = data.as_slice();
        let package =
            KeyPackageData::mls_decode(&mut cursor).map_err(|_| ProviderError::CorruptState)?;
        if !cursor.is_empty() {
            return self.checked(Err(ProviderError::CorruptState));
        }
        Ok(Some(package))
    }
}
