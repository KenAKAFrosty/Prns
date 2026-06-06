use crate::engine::egress::EgressSerializeError;
use crate::identity::IdentityHash;
use crate::routing::dedup::PacketHash;
use crate::wire::{HEADER_MIN_LEN, SIGNATURE_LEN};

pub const IMPLICIT_PROOF_WIRE_LEN: usize = HEADER_MIN_LEN + SIGNATURE_LEN;

/// The proof of receipt a delivered Single packet earned under its
/// destination's [`ProofStrategy`](crate::routing::upstream_app_destinations::ProofStrategy):
/// everything [`EngineState::write_proof`](crate::engine::EngineState::write_proof)
/// needs to sign and frame the answer, carried in the ingest outcome so it
/// lives exactly one cycle; the engine keeps no proof state. The proof answers
/// on the interface the packet arrived on; the delivery beside it carries
/// `source_interface`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofOwed {
    pub packet_hash: PacketHash,
    pub identity: IdentityHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteProofError {
    IdentityNotHeld,
    Serialize(EgressSerializeError),
}
