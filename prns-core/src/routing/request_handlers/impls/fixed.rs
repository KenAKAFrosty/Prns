use heapless::Vec as HeaplessVec;

use crate::identity::IdentityHash;
use crate::routing::request_handlers::{RequestHandlerTable, RequestPathHash, RequestPolicy};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

pub const MAX_ALLOWED_REQUESTERS: usize = 4;

#[derive(Debug, Default)]
pub struct FixedRequestHandlerTable<const MAX_REQUEST_HANDLERS: usize> {
    destinations: HeaplessVec<DestinationHash, MAX_REQUEST_HANDLERS>,
    path_hashes: HeaplessVec<RequestPathHash, MAX_REQUEST_HANDLERS>,
    policies: HeaplessVec<RequestPolicy, MAX_REQUEST_HANDLERS>,
    allowed: HeaplessVec<HeaplessVec<IdentityHash, MAX_ALLOWED_REQUESTERS>, MAX_REQUEST_HANDLERS>,
}

impl<const MAX_REQUEST_HANDLERS: usize> RequestHandlerTable
    for FixedRequestHandlerTable<MAX_REQUEST_HANDLERS>
{
    fn capacity(&self) -> usize {
        MAX_REQUEST_HANDLERS
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
    ) -> Result<(), TablePushError> {
        if self.destinations.is_full() {
            return Err(TablePushError::TableFull);
        }
        let _ = self.destinations.push(destination);
        let _ = self.path_hashes.push(path_hash);
        let _ = self.policies.push(policy);
        let _ = self.allowed.push(HeaplessVec::new());
        Ok(())
    }

    fn remove_at(&mut self, slot: usize) {
        if slot < self.destinations.len() {
            self.destinations.remove(slot);
            self.path_hashes.remove(slot);
            self.policies.remove(slot);
            self.allowed.remove(slot);
        }
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

    fn allow_at(&mut self, slot: usize, identity: IdentityHash) -> Result<(), TablePushError> {
        let Some(list) = self.allowed.get_mut(slot) else {
            return Err(TablePushError::TableFull);
        };
        list.push(identity).map_err(|_| TablePushError::TableFull)
    }

    fn disallow_at(&mut self, slot: usize, identity: &IdentityHash) {
        if let Some(list) = self.allowed.get_mut(slot) {
            list.retain(|candidate| candidate != identity);
        }
    }
}
