use crate::crypto::{Ed25519SecretKey, Ed25519Signature};
use crate::engine::{CommandId, InstantMillis, PacketReceiptDelivered};
use crate::identity::{IdentityHash, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::routing::links::LinkId;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::wire::{DestinationHash, WireError};

/// The command a valid receipt proof will settle.
///
/// Receipt proofs cannot settle requests: responses own that lifecycle. Keeping only the two
/// proof-settled command kinds here lets [`ReceiptProofVerifyOwed`] carry an exhaustive resume
/// decision instead of a generic settlement assembled by the runtime.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptProofClaim {
    SendSinglePacket {
        id: CommandId,
        delivered: PacketReceiptDelivered,
    },
    SendToLink {
        id: CommandId,
        delivered: PacketReceiptDelivered,
    },
}

impl ReceiptProofClaim {
    #[must_use]
    pub const fn command_id(self) -> CommandId {
        match self {
            Self::SendSinglePacket { id, .. } | Self::SendToLink { id, .. } => id,
        }
    }
}

/// An outstanding receipt proof whose signature must be verified before the engine may settle
/// its command or credit its route/link evidence.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptProofVerifyOwed {
    pub claim: ReceiptProofClaim,
    pub packet_hash: PacketHash,
    pub signing_key: IdentitySigningPublicKey,
    pub signature: Ed25519Signature,
    pub arrived_at: InstantMillis,
}

/// The runtime's verdict for one [`ReceiptProofVerifyOwed`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptProofVerification {
    Valid,
    Invalid,
}

pub struct DeferredProofSign {
    pub target: InterfaceId,
    pub packet_hash: PacketHash,
    pub signing_secret: Ed25519SecretKey,
}

pub struct DeferredLinkReceiptSign {
    pub target: InterfaceId,
    pub link_id: LinkId,
    pub packet_hash: PacketHash,
    pub signing_secret: Ed25519SecretKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofOwed {
    pub packet_hash: PacketHash,
    pub identity: IdentityHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkProofOwed {
    pub link_id: LinkId,
    pub packet_hash: PacketHash,
    pub identity: IdentityHash,
    pub destination: DestinationHash,
}

pub struct ProofRequest<'a> {
    pub destination: DestinationHash,
    pub plaintext: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofObligation {
    None,
    Owed(ProofOwed),
    OwedIfApp(ProofOwed),
    OwedOverLink(LinkProofOwed),
    OwedIfAppOverLink(LinkProofOwed),
}

impl ProofObligation {
    pub fn for_delivery(strategy: ProofStrategy, owed: ProofOwed) -> Self {
        match strategy {
            ProofStrategy::ProveAll => Self::Owed(owed),
            ProofStrategy::ProveNone => Self::None,
            ProofStrategy::ProveIf => Self::OwedIfApp(owed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteProofError {
    IdentityNotHeld,
    Serialize(WireError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteChannelAckError {
    LinkNotActive,
    IdentityNotHeld,
    Serialize(WireError),
}
