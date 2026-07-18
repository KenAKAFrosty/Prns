use crate::crypto::{Ed25519SecretKey, Ed25519Signature};
use crate::engine::{CommandId, PacketReceiptDelivered};
use crate::identity::{IdentityHash, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::routing::links::LinkId;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::wire::{DestinationHash, WireError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofIngest {
    SendSinglePacketDelivered {
        id: CommandId,
        delivered: PacketReceiptDelivered,
    },
    SendToLinkDelivered {
        id: CommandId,
        delivered: PacketReceiptDelivered,
    },
    SendToChannelDelivered {
        id: CommandId,
        delivered: PacketReceiptDelivered,
    },
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeferredProof {
    pub ingest: ProofIngest,
    pub packet_hash: PacketHash,
    pub signing_key: IdentitySigningPublicKey,
    pub signature: Ed25519Signature,
}

pub struct DeferredProofSign {
    pub target: InterfaceId,
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
