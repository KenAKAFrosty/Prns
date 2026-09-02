use personal_rns::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey, X25519SecretKey};
use personal_rns::engine::{AnnounceVerifyOwed, WholeResourceOpenPlan};
use personal_rns::routing::announce::Announce;
use personal_rns::routing::links::handshake::LinkProofVerifyOwed;
use personal_rns::routing::links::resources::build_outgoing::SALT_REROLL_CAP;
use personal_rns::routing::links::resources::send::ResourceSealPlan;
use personal_rns::routing::links::resources::RESOURCE_NONCE_LEN;
use personal_rns::wire::{BROADCAST_MTU, TRUNCATED_HASH_BYTE_LEN};
use zeroize::Zeroizing;

const MAXIMUM_PENDING_BROWSER_WORK: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserWorkKind {
    AnnounceVerify,
    LinkProofVerify,
    ResourceSeal,
    WholeResourceOpen,
}

enum BrowserWorkState {
    Queued,
    Running,
}

pub(crate) enum BrowserWorkOperation {
    AnnounceVerify {
        owed: Box<AnnounceVerifyOwed>,
        public_key: [u8; Ed25519PublicKey::LEN],
        message: Vec<u8>,
        signature: [u8; Ed25519Signature::LEN],
    },
    LinkProofVerify(LinkProofVerifyOwed),
    ResourceSeal {
        plan: ResourceSealPlan,
        workspace: Vec<u8>,
        seal_iv: [u8; 16],
        salts: [[u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP],
    },
    WholeResourceOpen {
        plan: WholeResourceOpenPlan,
        sealed: Vec<u8>,
    },
}

impl BrowserWorkOperation {
    pub(crate) fn announce(owed: AnnounceVerifyOwed) -> Result<Self, Box<AnnounceVerifyOwed>> {
        let prepared = {
            let announce = match Announce::from_wire_unverified(&owed.header, &owed.payload) {
                Ok(announce) => announce,
                Err(_) => return Err(Box::new(owed)),
            };
            let mut message = vec![0; TRUNCATED_HASH_BYTE_LEN + BROADCAST_MTU];
            let signed_bytes = match announce.write_signed_material(&mut message) {
                Ok(signed_bytes) => signed_bytes,
                Err(_) => return Err(Box::new(owed)),
            };
            message.truncate(signed_bytes);
            (
                announce.public_keys.signing.as_bytes().to_owned(),
                message,
                announce.signature.0,
            )
        };
        Ok(Self::AnnounceVerify {
            owed: Box::new(owed),
            public_key: prepared.0,
            message: prepared.1,
            signature: prepared.2,
        })
    }

    fn kind(&self) -> BrowserWorkKind {
        match self {
            Self::AnnounceVerify { .. } => BrowserWorkKind::AnnounceVerify,
            Self::LinkProofVerify(_) => BrowserWorkKind::LinkProofVerify,
            Self::ResourceSeal { .. } => BrowserWorkKind::ResourceSeal,
            Self::WholeResourceOpen { .. } => BrowserWorkKind::WholeResourceOpen,
        }
    }

}

struct PendingBrowserWork {
    id: u32,
    state: BrowserWorkState,
    operation: BrowserWorkOperation,
}

pub(crate) enum BrowserWorkJob {
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
    ResourceSeal {
        id: u32,
        link_id: personal_rns::routing::links::LinkId,
        nonce_prefixed_bytes: usize,
        total_segments: u64,
        workspace: Vec<u8>,
        signing_key: [u8; 32],
        encryption_key: [u8; 32],
        seal_iv: [u8; 16],
        salts: [[u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP],
    },
    WholeResourceOpen {
        id: u32,
        link_id: personal_rns::routing::links::LinkId,
        hash: personal_rns::routing::links::resources::ResourceHash,
        signing_key: [u8; 32],
        encryption_key: [u8; 32],
        sealed: Vec<u8>,
        compression: personal_rns::routing::links::resources::ResourceCompression,
        salt_nonce: personal_rns::routing::links::resources::SaltNonce,
        total_segments: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserWorkSettlementError {
    UnknownJob,
    JobNotRunning,
}

pub(crate) struct BrowserWorkQueue {
    next_id: u32,
    pending: Vec<PendingBrowserWork>,
}

impl BrowserWorkQueue {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            pending: Vec::new(),
        }
    }

    pub(crate) fn has_capacity(&self) -> bool {
        self.pending.len() < MAXIMUM_PENDING_BROWSER_WORK
    }

    pub(crate) fn admit(
        &mut self,
        operation: BrowserWorkOperation,
    ) -> Result<(), Box<BrowserWorkOperation>> {
        if !self.has_capacity() {
            return Err(Box::new(operation));
        }
        let mut id = self.next_id;
        while self.pending.iter().any(|pending| pending.id == id) {
            id = id.checked_add(1).unwrap_or(1);
        }
        self.next_id = id.checked_add(1).unwrap_or(1);
        self.pending.push(PendingBrowserWork {
            id,
            state: BrowserWorkState::Queued,
            operation,
        });
        Ok(())
    }

    pub(crate) fn take(&mut self) -> Option<BrowserWorkJob> {
        let pending = self
            .pending
            .iter_mut()
            .find(|pending| matches!(pending.state, BrowserWorkState::Queued))?;
        pending.state = BrowserWorkState::Running;
        Some(match &mut pending.operation {
            BrowserWorkOperation::AnnounceVerify {
                public_key,
                message,
                signature,
                ..
            } => BrowserWorkJob::AnnounceVerify {
                id: pending.id,
                public_key: *public_key,
                message: core::mem::take(message),
                signature: *signature,
            },
            BrowserWorkOperation::LinkProofVerify(owed) => BrowserWorkJob::LinkProofVerify {
                id: pending.id,
                public_key: owed.responder_signing.0,
                message: owed.signed_data[..owed.signed_bytes].to_vec(),
                signature: owed.signature.0,
                secret_scalar: owed
                    .initiator_secret
                    .with_scalar_bytes(|bytes| Zeroizing::new(*bytes)),
                peer_public_key: owed.responder_encryption.0,
            },
            BrowserWorkOperation::ResourceSeal {
                plan,
                workspace,
                seal_iv,
                salts,
            } => BrowserWorkJob::ResourceSeal {
                    id: pending.id,
                    link_id: plan.link_id(),
                    nonce_prefixed_bytes: plan.nonce_prefixed_bytes(),
                    total_segments: plan.total_segments(),
                    workspace: core::mem::take(workspace),
                    signing_key: *plan.signing_key_material(),
                    encryption_key: *plan.encryption_key_material(),
                    seal_iv: *seal_iv,
                    salts: *salts,
                },
            BrowserWorkOperation::WholeResourceOpen { plan, sealed } => {
                BrowserWorkJob::WholeResourceOpen {
                    id: pending.id,
                    link_id: plan.link_id(),
                    hash: plan.hash(),
                    signing_key: *plan.signing_key_material(),
                    encryption_key: *plan.encryption_key_material(),
                    sealed: core::mem::take(sealed),
                    compression: plan.compression(),
                    salt_nonce: plan.salt_nonce(),
                    total_segments: plan.total_segments(),
                }
            }
        })
    }

    pub(crate) fn settle_any(
        &mut self,
        id: u32,
    ) -> Result<BrowserWorkOperation, BrowserWorkSettlementError> {
        let Some(index) = self.pending.iter().position(|pending| pending.id == id) else {
            return Err(BrowserWorkSettlementError::UnknownJob);
        };
        if !matches!(self.pending[index].state, BrowserWorkState::Running) {
            return Err(BrowserWorkSettlementError::JobNotRunning);
        }
        Ok(self.pending.swap_remove(index).operation)
    }

    pub(crate) fn running_kind(
        &self,
        id: u32,
    ) -> Result<BrowserWorkKind, BrowserWorkSettlementError> {
        let Some(pending) = self.pending.iter().find(|pending| pending.id == id) else {
            return Err(BrowserWorkSettlementError::UnknownJob);
        };
        if !matches!(pending.state, BrowserWorkState::Running) {
            return Err(BrowserWorkSettlementError::JobNotRunning);
        }
        Ok(pending.operation.kind())
    }

    pub(crate) fn restore(&mut self, id: u32, operation: BrowserWorkOperation) {
        self.pending.push(PendingBrowserWork {
            id,
            state: BrowserWorkState::Running,
            operation,
        });
    }
}
