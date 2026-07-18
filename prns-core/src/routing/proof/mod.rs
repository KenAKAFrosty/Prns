mod model;
mod wire;

pub use model::{
    DeferredProof, DeferredProofSign, LinkProofOwed, ProofIngest, ProofObligation, ProofOwed,
    ProofRequest, WriteChannelAckError, WriteProofError,
};
pub use wire::{
    write_explicit_proof_wire_packet, write_implicit_proof_wire_packet,
    write_link_proof_wire_packet, EXPLICIT_PROOF_PAYLOAD_LEN, EXPLICIT_PROOF_WIRE_LEN,
    IMPLICIT_PROOF_PAYLOAD_LEN, IMPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN,
};
