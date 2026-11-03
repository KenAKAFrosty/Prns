use super::{core, groups, runtime, Error};
use std::sync::{Arc, Mutex};

struct Entry {
    identity: core::IdentityReference,
    owner: runtime::OwnerToken,
    source: groups::SyncProgressSource,
}

pub(super) struct Registry {
    entries: Mutex<Vec<Entry>>,
    capacity: usize,
}
impl Registry {
    pub fn new(capacity: usize) -> Result<Self, Error> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_| Error::Allocation)?;
        Ok(Self {
            entries: Mutex::new(entries),
            capacity,
        })
    }
    pub fn snapshot(
        &self,
        identity: core::IdentityReference,
        group: prns_groups_store::GroupId,
    ) -> groups::GroupSyncProgress {
        let source = self.entries.lock().ok().and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.identity == identity)
                .map(|entry| entry.source.clone())
        });
        source
            .map(|source| source.snapshot(group))
            .unwrap_or(groups::GroupSyncProgress {
                running: false,
                observed: false,
                control: None,
                peers: Vec::new(),
            })
    }
    pub fn register(
        self: &Arc<Self>,
        identity: core::IdentityReference,
        owner: runtime::OwnerToken,
        source: groups::SyncProgressSource,
    ) -> Result<Registration, groups::Error> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| groups::Error::Unavailable)?;
        if entries.len() == self.capacity
            || entries
                .iter()
                .any(|entry| entry.identity == identity || entry.owner == owner)
        {
            return Err(groups::Error::Configuration);
        }
        entries.push(Entry {
            identity,
            owner,
            source,
        });
        Ok(Registration {
            registry: self.clone(),
            identity,
            owner,
        })
    }
}

/// The retained runner owns this guard, including during startup and unwind.
/// Public client construction and unknown-reference reads never create entries.
pub(super) struct Registration {
    registry: Arc<Registry>,
    identity: core::IdentityReference,
    owner: runtime::OwnerToken,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.registry.entries.lock() {
            entries.retain(|entry| entry.identity != self.identity || entry.owner != self.owner);
        }
    }
}
