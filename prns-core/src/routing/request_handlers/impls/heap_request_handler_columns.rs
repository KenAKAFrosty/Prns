use alloc::vec::Vec;

use crate::identity::IdentityHash;
use crate::routing::request_handlers::{RequestHandlerColumns, RequestPathHash, RequestPolicy};
use crate::storage::ColumnsFull;
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapRequestHandlerColumns {
    destinations: Vec<DestinationHash>,
    path_hashes: Vec<RequestPathHash>,
    policies: Vec<RequestPolicy>,
    allowed: Vec<Vec<IdentityHash>>,
}

impl RequestHandlerColumns for HeapRequestHandlerColumns {
    fn capacity(&self) -> usize {
        usize::MAX
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn path_hashes(&self) -> &[RequestPathHash] {
        &self.path_hashes
    }
    fn policies(&self) -> &[RequestPolicy] {
        &self.policies
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        path_hash: RequestPathHash,
        policy: RequestPolicy,
    ) -> Result<(), ColumnsFull> {
        self.destinations.push(destination);
        self.path_hashes.push(path_hash);
        self.policies.push(policy);
        self.allowed.push(Vec::new());
        Ok(())
    }

    fn set_policy_at(&mut self, slot: usize, policy: RequestPolicy) {
        if let Some(existing) = self.policies.get_mut(slot) {
            *existing = policy;
        }
    }

    fn clear_allowed_at(&mut self, slot: usize) {
        if let Some(list) = self.allowed.get_mut(slot) {
            list.clear();
        }
    }

    fn allowed_contains_at(&self, slot: usize, identity: &IdentityHash) -> bool {
        self.allowed
            .get(slot)
            .is_some_and(|list| list.contains(identity))
    }

    fn allow_at(&mut self, slot: usize, identity: IdentityHash) -> Result<(), ColumnsFull> {
        let Some(list) = self.allowed.get_mut(slot) else {
            return Err(ColumnsFull);
        };
        list.push(identity);
        Ok(())
    }

    fn disallow_at(&mut self, slot: usize, identity: &IdentityHash) {
        if let Some(list) = self.allowed.get_mut(slot) {
            list.retain(|candidate| candidate != identity);
        }
    }
}
