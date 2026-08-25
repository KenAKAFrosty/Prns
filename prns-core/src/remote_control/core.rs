use crate::identity::{IdentityHash, IdentityPublicKeys};
use crate::storage::TablePushError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlIdentity {
    public_keys: IdentityPublicKeys,
}

impl RemoteControlIdentity {
    #[must_use]
    pub const fn new(public_keys: IdentityPublicKeys) -> Self {
        Self { public_keys }
    }

    #[must_use]
    pub const fn public_keys(&self) -> &IdentityPublicKeys {
        &self.public_keys
    }

    #[must_use]
    pub fn identity_hash(&self) -> IdentityHash {
        self.public_keys.identity_hash()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveRemoteControlAccessOutcome {
    Removed,
    NotFound,
}

pub trait RemoteControlAccessTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
    fn identities(&self) -> &[RemoteControlIdentity];
    fn upsert(&mut self, identity: RemoteControlIdentity) -> Result<(), TablePushError>;
    fn swap_remove(&mut self, index: usize);

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn index_of(&self, identity: &IdentityHash) -> Option<usize> {
        self.identities()
            .iter()
            .position(|candidate| candidate.identity_hash() == *identity)
    }

    fn get(&self, identity: &IdentityHash) -> Option<&RemoteControlIdentity> {
        //It's expected this will be a very short list (single digit members allowed in most cases, etc.)
        //Because the domain fundamentally wants to keep this tight, quick scans like this are not only okay but probably faster
        //If we start getting to like ~32 members or more to support real use cases or something, we'll want to improve this with a proper index.
        self.identities().get(self.index_of(identity)?)
    }

    fn contains(&self, identity: &IdentityHash) -> bool {
        self.index_of(identity).is_some()
    }

    fn remove(&mut self, identity: &IdentityHash) -> RemoveRemoteControlAccessOutcome {
        let Some(index) = self.index_of(identity) else {
            return RemoveRemoteControlAccessOutcome::NotFound;
        };
        self.swap_remove(index);
        RemoveRemoteControlAccessOutcome::Removed
    }
}
