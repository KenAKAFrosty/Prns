use crate::engine::egress::EgressSerializeError;
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;

/// The proof of receipt a delivered Single packet earned under its
/// destination's [`ProofStrategy`](crate::routing::upstream_app_destinations::ProofStrategy):
/// everything [`EngineState::write_proof`](crate::engine::EngineState::write_proof)
/// needs to sign and frame the answer, carried in the ingest outcome so it
/// lives exactly one cycle; the engine keeps no proof state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofOwed {
    pub packet_hash: PacketHash,
    pub identity: IdentityHash,
    pub send_on: InterfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteProofError {
    IdentityNotHeld,
    Serialize(EgressSerializeError),
}
