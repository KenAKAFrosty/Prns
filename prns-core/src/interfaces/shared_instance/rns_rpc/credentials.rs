use alloc::vec::Vec;
use core::fmt;

use zeroize::Zeroize;

use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentityHash, IdentitySigner, IDENTITY_SECRET_KEY_LEN};

#[derive(Clone, PartialEq, Eq)]
pub struct RpcAuthenticationKey(Vec<u8>);

impl RpcAuthenticationKey {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn from_rns_transport_identity_secret(secret: &[u8; IDENTITY_SECRET_KEY_LEN]) -> Self {
        Self(crate::crypto::sha256(secret).to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for RpcAuthenticationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RpcAuthenticationKey")
            .field(&"[REDACTED]")
            .finish()
    }
}

impl Drop for RpcAuthenticationKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedInstanceCredentials {
    rpc_key: RpcAuthenticationKey,
    transport_identity_hash: IdentityHash,
}

impl SharedInstanceCredentials {
    pub fn new(rpc_key: RpcAuthenticationKey, transport_identity_hash: IdentityHash) -> Self {
        Self {
            rpc_key,
            transport_identity_hash,
        }
    }

    pub fn from_identity_secret(secret: &[u8; IDENTITY_SECRET_KEY_LEN]) -> Self {
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(secret);
        Self::new(
            RpcAuthenticationKey::from_rns_transport_identity_secret(secret),
            identity.identity_hash(),
        )
    }

    pub fn with_rpc_authentication_key(mut self, rpc_key: RpcAuthenticationKey) -> Self {
        self.rpc_key = rpc_key;
        self
    }

    pub fn with_rpc_key(mut self, rpc_key: Vec<u8>) -> Self {
        self.rpc_key = RpcAuthenticationKey::new(rpc_key);
        self
    }

    pub fn rpc_key(&self) -> &RpcAuthenticationKey {
        &self.rpc_key
    }

    pub const fn transport_identity_hash(&self) -> IdentityHash {
        self.transport_identity_hash
    }
}
