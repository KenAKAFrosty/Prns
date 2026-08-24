use crate::identity::IdentityHash;
use crate::storage::TablePushError;
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

/// RNS 1.4.2 `Identity.truncated_hash(path.encode("utf-8"))`: the wire never carries the path string; both ends know it by contract and meet at this hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPathHash([u8; TRUNCATED_HASH_BYTE_LEN]);

impl RequestPathHash {
    #[must_use]
    pub fn of(path: &str) -> Self {
        let full = crate::crypto::sha256(path.as_bytes());
        let mut truncated = [0u8; TRUNCATED_HASH_BYTE_LEN];
        truncated.copy_from_slice(&full[..TRUNCATED_HASH_BYTE_LEN]);
        Self(truncated)
    }

    #[must_use]
    pub const fn new(bytes: [u8; TRUNCATED_HASH_BYTE_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; TRUNCATED_HASH_BYTE_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPolicy {
    AllowNone,
    AllowAll,
    RequireIdentified,
    AllowList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestHandlerError {
    NoSuchHandler,
    NoAllowList,
    AllowListFull,
}

pub trait RequestHandlerTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn path_hashes(&self) -> &[RequestPathHash];
    fn policies(&self) -> &[RequestPolicy];

    /// Insert a fresh handler row with an empty allow list. The wrapper owns upsert semantics and never pushes a duplicate key.
    fn push(
        &mut self,
        destination: DestinationHash,
        path_hash: RequestPathHash,
        policy: RequestPolicy,
    ) -> Result<(), TablePushError>;

    fn remove_at(&mut self, slot: usize);
    fn set_policy_at(&mut self, slot: usize, policy: RequestPolicy);
    fn clear_allowed_at(&mut self, slot: usize);
    fn allowed_contains_at(&self, slot: usize, identity: &IdentityHash) -> bool;
    fn allow_at(&mut self, slot: usize, identity: IdentityHash) -> Result<(), TablePushError>;
    fn disallow_at(&mut self, slot: usize, identity: &IdentityHash);
}

#[derive(Debug, Default)]
pub struct RequestHandlers<C: RequestHandlerTable> {
    table: C,
}

impl<C: RequestHandlerTable> RequestHandlers<C> {
    fn slot_for(
        &self,
        destination: &DestinationHash,
        path_hash: &RequestPathHash,
    ) -> Option<usize> {
        self.table
            .destinations()
            .iter()
            .zip(self.table.path_hashes())
            .position(|(candidate, hash)| candidate == destination && hash == path_hash)
    }

    /// Register (or re-register) a handler: last write wins, and a policy change starts from an empty allow list, the same upsert posture destination registration takes.
    pub fn register(
        &mut self,
        destination: DestinationHash,
        path_hash: RequestPathHash,
        policy: RequestPolicy,
    ) -> Result<(), TablePushError> {
        match self.slot_for(&destination, &path_hash) {
            Some(slot) => {
                self.table.set_policy_at(slot, policy);
                self.table.clear_allowed_at(slot);
                Ok(())
            }
            None => self.table.push(destination, path_hash, policy),
        }
    }

    /// Remove a handler row if it exists. An already absent route is an
    /// idempotent no-op, which lets runtime reconcilers converge safely.
    pub fn unregister(
        &mut self,
        destination: &DestinationHash,
        path_hash: &RequestPathHash,
    ) -> bool {
        let Some(slot) = self.slot_for(destination, path_hash) else {
            return false;
        };
        self.table.remove_at(slot);
        true
    }

    pub fn allow(
        &mut self,
        destination: &DestinationHash,
        path_hash: &RequestPathHash,
        identity: IdentityHash,
    ) -> Result<(), RequestHandlerError> {
        let slot = self
            .slot_for(destination, path_hash)
            .ok_or(RequestHandlerError::NoSuchHandler)?;
        if self.table.policies()[slot] != RequestPolicy::AllowList {
            return Err(RequestHandlerError::NoAllowList);
        }
        if self.table.allowed_contains_at(slot, &identity) {
            return Ok(());
        }
        self.table
            .allow_at(slot, identity)
            .map_err(|TablePushError::TableFull| RequestHandlerError::AllowListFull)
    }

    pub fn disallow(
        &mut self,
        destination: &DestinationHash,
        path_hash: &RequestPathHash,
        identity: &IdentityHash,
    ) -> Result<(), RequestHandlerError> {
        let slot = self
            .slot_for(destination, path_hash)
            .ok_or(RequestHandlerError::NoSuchHandler)?;
        if self.table.policies()[slot] != RequestPolicy::AllowList {
            return Err(RequestHandlerError::NoAllowList);
        }
        self.table.disallow_at(slot, identity);
        Ok(())
    }

    pub fn permits(
        &self,
        destination: &DestinationHash,
        path_hash: &RequestPathHash,
        remote_identity: Option<&IdentityHash>,
    ) -> bool {
        let Some(slot) = self.slot_for(destination, path_hash) else {
            return false;
        };
        match self.table.policies()[slot] {
            RequestPolicy::AllowNone => false,
            RequestPolicy::AllowAll => true,
            RequestPolicy::RequireIdentified => remote_identity.is_some(),
            RequestPolicy::AllowList => remote_identity
                .is_some_and(|identity| self.table.allowed_contains_at(slot, identity)),
        }
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    type TestHandlers = RequestHandlers<FixedRequestHandlerTable<4>>;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn identity(byte: u8) -> IdentityHash {
        IdentityHash::new([byte; 16])
    }

    #[test]
    fn the_gate_follows_the_reference_policies() {
        let mut handlers = TestHandlers::default();
        let path = RequestPathHash::of("/page/index.mu");
        handlers
            .register(dest(1), path, RequestPolicy::AllowNone)
            .unwrap();
        assert!(
            !handlers.permits(&dest(1), &path, None),
            "AllowNone refuses"
        );

        handlers
            .register(dest(1), path, RequestPolicy::AllowAll)
            .unwrap();
        assert!(handlers.permits(&dest(1), &path, None), "AllowAll permits");
        assert!(
            !handlers.permits(&dest(2), &path, None),
            "another destination's handler does not answer for this one",
        );
        assert!(
            !handlers.permits(&dest(1), &RequestPathHash::of("/other"), None),
            "an unregistered path stays silent",
        );

        handlers
            .register(dest(1), path, RequestPolicy::RequireIdentified)
            .unwrap();
        assert!(!handlers.permits(&dest(1), &path, None));
        assert!(handlers.permits(&dest(1), &path, Some(&identity(0xAA))));

        handlers
            .register(dest(1), path, RequestPolicy::AllowList)
            .unwrap();
        assert!(
            !handlers.permits(&dest(1), &path, Some(&identity(0xAA))),
            "an empty list permits no one",
        );
        handlers.allow(&dest(1), &path, identity(0xAA)).unwrap();
        assert!(handlers.permits(&dest(1), &path, Some(&identity(0xAA))));
        assert!(
            !handlers.permits(&dest(1), &path, None),
            "an unidentified link never passes a list",
        );
        handlers.disallow(&dest(1), &path, &identity(0xAA)).unwrap();
        assert!(!handlers.permits(&dest(1), &path, Some(&identity(0xAA))));
    }

    #[test]
    fn re_registration_resets_the_allow_list() {
        let mut handlers = TestHandlers::default();
        let path = RequestPathHash::of("/admin");
        handlers
            .register(dest(1), path, RequestPolicy::AllowList)
            .unwrap();
        handlers.allow(&dest(1), &path, identity(0xAA)).unwrap();
        handlers
            .register(dest(1), path, RequestPolicy::AllowList)
            .unwrap();
        assert!(
            !handlers.permits(&dest(1), &path, Some(&identity(0xAA))),
            "a fresh registration starts from an empty list",
        );
        assert_eq!(handlers.len(), 1, "re-registration keeps one row");
    }

    #[test]
    fn unregister_removes_exactly_one_handler_and_is_idempotent() {
        let mut handlers = TestHandlers::default();
        let first = RequestPathHash::of("/page/first.mu");
        let second = RequestPathHash::of("/page/second.mu");
        handlers
            .register(dest(1), first, RequestPolicy::AllowAll)
            .unwrap();
        handlers
            .register(dest(1), second, RequestPolicy::AllowAll)
            .unwrap();

        assert!(handlers.unregister(&dest(1), &first));
        assert!(!handlers.unregister(&dest(1), &first));
        assert!(!handlers.permits(&dest(1), &first, None));
        assert!(handlers.permits(&dest(1), &second, None));
        assert_eq!(handlers.len(), 1);
    }

    #[test]
    fn list_management_on_a_handler_that_keeps_no_list_is_refused() {
        let mut handlers = TestHandlers::default();
        let path = RequestPathHash::of("/open");
        handlers
            .register(dest(1), path, RequestPolicy::AllowAll)
            .unwrap();
        assert_eq!(
            handlers.allow(&dest(1), &path, identity(1)),
            Err(RequestHandlerError::NoAllowList),
        );
        assert_eq!(
            handlers.disallow(&dest(1), &path, &identity(1)),
            Err(RequestHandlerError::NoAllowList),
        );

        handlers
            .register(dest(1), path, RequestPolicy::AllowNone)
            .unwrap();
        assert_eq!(
            handlers.allow(&dest(1), &path, identity(1)),
            Err(RequestHandlerError::NoAllowList),
        );
    }

    #[test]
    fn missing_handlers_report_on_list_management() {
        let mut handlers = TestHandlers::default();
        let path = RequestPathHash::of("/nope");
        assert_eq!(
            handlers.allow(&dest(1), &path, identity(1)),
            Err(RequestHandlerError::NoSuchHandler),
        );
        assert_eq!(
            handlers.disallow(&dest(1), &path, &identity(1)),
            Err(RequestHandlerError::NoSuchHandler),
        );
    }
}
