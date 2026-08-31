use personal_rns::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey, X25519SecretKey};
use personal_rns::engine::AnnounceVerifyOwed;
use personal_rns::routing::announce::Announce;
use personal_rns::routing::links::handshake::LinkProofVerifyOwed;
use personal_rns::wire::{BROADCAST_MTU, TRUNCATED_HASH_BYTE_LEN};
use zeroize::Zeroizing;

const MAXIMUM_PENDING_PROTOCOL_CRYPTO: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtocolCryptoKind {
    AnnounceVerify,
    LinkProofVerify,
}

enum ProtocolCryptoState {
    Queued,
    Running,
}

pub(crate) enum ProtocolCryptoOperation {
    AnnounceVerify {
        owed: AnnounceVerifyOwed,
        public_key: [u8; Ed25519PublicKey::LEN],
        message: Vec<u8>,
        signature: [u8; Ed25519Signature::LEN],
    },
    LinkProofVerify(LinkProofVerifyOwed),
}

impl ProtocolCryptoOperation {
    pub(crate) fn announce(owed: AnnounceVerifyOwed) -> Result<Self, AnnounceVerifyOwed> {
        let prepared = {
            let announce = match Announce::from_wire_unverified(&owed.header, &owed.payload) {
                Ok(announce) => announce,
                Err(_) => return Err(owed),
            };
            let mut message = vec![0; TRUNCATED_HASH_BYTE_LEN + BROADCAST_MTU];
            let signed_bytes = match announce.write_signed_material(&mut message) {
                Ok(signed_bytes) => signed_bytes,
                Err(_) => return Err(owed),
            };
            message.truncate(signed_bytes);
            (
                announce.public_keys.signing.as_bytes().to_owned(),
                message,
                announce.signature.0,
            )
        };
        Ok(Self::AnnounceVerify {
            owed,
            public_key: prepared.0,
            message: prepared.1,
            signature: prepared.2,
        })
    }

    pub(crate) fn link_proof(owed: LinkProofVerifyOwed) -> Self {
        Self::LinkProofVerify(owed)
    }

    fn kind(&self) -> ProtocolCryptoKind {
        match self {
            Self::AnnounceVerify { .. } => ProtocolCryptoKind::AnnounceVerify,
            Self::LinkProofVerify(_) => ProtocolCryptoKind::LinkProofVerify,
        }
    }
}

struct PendingProtocolCrypto {
    id: u32,
    state: ProtocolCryptoState,
    operation: ProtocolCryptoOperation,
}

pub(crate) enum ProtocolCryptoJob {
    AnnounceVerify {
        id: u32,
        public_key: [u8; Ed25519PublicKey::LEN],
        message: Vec<u8>,
        signature: [u8; Ed25519Signature::LEN],
    },
    LinkProofVerify {
        id: u32,
        public_key: [u8; Ed25519PublicKey::LEN],
        message: Vec<u8>,
        signature: [u8; Ed25519Signature::LEN],
        secret_scalar: Zeroizing<[u8; X25519SecretKey::LEN]>,
        peer_public_key: [u8; X25519PublicKey::LEN],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtocolCryptoSettlementError {
    UnknownJob,
    OperationMismatch,
    JobNotRunning,
}

pub(crate) struct ProtocolCryptoQueue {
    next_id: u32,
    pending: Vec<PendingProtocolCrypto>,
}

impl ProtocolCryptoQueue {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            pending: Vec::new(),
        }
    }

    pub(crate) fn has_capacity(&self) -> bool {
        self.pending.len() < MAXIMUM_PENDING_PROTOCOL_CRYPTO
    }

    pub(crate) fn admit(
        &mut self,
        operation: ProtocolCryptoOperation,
    ) -> Result<(), ProtocolCryptoOperation> {
        if !self.has_capacity() {
            return Err(operation);
        }
        let mut id = self.next_id;
        while self.pending.iter().any(|pending| pending.id == id) {
            id = id.checked_add(1).unwrap_or(1);
        }
        self.next_id = id.checked_add(1).unwrap_or(1);
        self.pending.push(PendingProtocolCrypto {
            id,
            state: ProtocolCryptoState::Queued,
            operation,
        });
        Ok(())
    }

    pub(crate) fn take(&mut self) -> Option<ProtocolCryptoJob> {
        let pending = self
            .pending
            .iter_mut()
            .find(|pending| matches!(pending.state, ProtocolCryptoState::Queued))?;
        pending.state = ProtocolCryptoState::Running;
        Some(match &mut pending.operation {
            ProtocolCryptoOperation::AnnounceVerify {
                public_key,
                message,
                signature,
                ..
            } => ProtocolCryptoJob::AnnounceVerify {
                id: pending.id,
                public_key: *public_key,
                message: core::mem::take(message),
                signature: *signature,
            },
            ProtocolCryptoOperation::LinkProofVerify(owed) => ProtocolCryptoJob::LinkProofVerify {
                id: pending.id,
                public_key: owed.responder_signing.0,
                message: owed.signed_data[..owed.signed_bytes].to_vec(),
                signature: owed.signature.0,
                secret_scalar: owed
                    .initiator_secret
                    .with_scalar_bytes(|bytes| Zeroizing::new(*bytes)),
                peer_public_key: owed.responder_encryption.0,
            },
        })
    }

    pub(crate) fn settle(
        &mut self,
        id: u32,
        expected: ProtocolCryptoKind,
    ) -> Result<ProtocolCryptoOperation, ProtocolCryptoSettlementError> {
        let Some(index) = self.pending.iter().position(|pending| pending.id == id) else {
            return Err(ProtocolCryptoSettlementError::UnknownJob);
        };
        if self.pending[index].operation.kind() != expected {
            return Err(ProtocolCryptoSettlementError::OperationMismatch);
        }
        if !matches!(self.pending[index].state, ProtocolCryptoState::Running) {
            return Err(ProtocolCryptoSettlementError::JobNotRunning);
        }
        Ok(self.pending.swap_remove(index).operation)
    }

    pub(crate) fn settle_any(
        &mut self,
        id: u32,
    ) -> Result<ProtocolCryptoOperation, ProtocolCryptoSettlementError> {
        let Some(index) = self.pending.iter().position(|pending| pending.id == id) else {
            return Err(ProtocolCryptoSettlementError::UnknownJob);
        };
        if !matches!(self.pending[index].state, ProtocolCryptoState::Running) {
            return Err(ProtocolCryptoSettlementError::JobNotRunning);
        }
        Ok(self.pending.swap_remove(index).operation)
    }
}
