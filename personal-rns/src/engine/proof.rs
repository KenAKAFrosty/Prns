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

use crate::engine::egress::write_implicit_proof_wire_packet;
use crate::engine::EngineState;
use crate::identity::IdentitySigner;
use crate::routing::storage::EngineStorage;

impl<S: EngineStorage> EngineState<S> {
    /// Sign and frame the proof a delivered packet earned ([`ProofOwed`], from
    /// this same cycle's ingest outcome) into `buf`, returning the wire length.
    /// Best-effort by RNS 1.3.1 parity: a proof that can't be written is simply
    /// dropped. The sender's timeout-and-resend (fresh ciphertext, fresh
    /// packet hash) is the designed recovery, so nothing here is retried.
    pub fn write_proof(&self, owed: &ProofOwed, buf: &mut [u8]) -> Result<usize, WriteProofError> {
        let identity = self
            .held_identities
            .get(&owed.identity)
            .ok_or(WriteProofError::IdentityNotHeld)?;
        let signature = identity.sign(owed.packet_hash.as_bytes());
        write_implicit_proof_wire_packet(&owed.packet_hash, &signature, buf)
            .map_err(WriteProofError::Serialize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{EngineState, IngestPacketOutcome, RatchetPolicy};
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::routing::dedup::PacketHash;
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::MTU;

    const RAW_SEALED_FOR_PROOF: &str =
        "0000c3cfae69b36bb6e3bbfd96a3b5867a59007b0d47d93427f8311160781c7c733fd89f88970aef490d8a\
         a0ee19a4cb8a1b1444444444444444444444444444444444084624da14eb2a916d8a20cad6da4623aff598\
         25ec6b58715afe16269730584f5fe3a55a6429ded73c3d4b2458f67ef9";

    const RNS_1_3_1_IMPLICIT_PROOF: &str =
        "0300a34e24b00ebdda0179b642579b71266c00f52e874f44101203b553179c107604fc01ef99e210895f95\
         423f14aca8094a5a09938d9337aec5c6cb1bc38458d65da559450a9f8e0e78921ca690bed8430100";

    #[test]
    fn write_proof_is_byte_identical_to_the_rns_1_3_1_implicit_proof() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let mut raw = sealed_single_packet(&identity, destination, b"proof-parity");
        assert_eq!(raw, hx(RAW_SEALED_FOR_PROOF));

        let outcome = state.ingest_packet(plain_data_packet(&mut raw), TEST_ENTROPY);
        let IngestPacketOutcome::Delivery {
            maybe_owed_proof: Some(owed),
            ..
        } = outcome
        else {
            panic!("a ProveAll delivery owes a proof");
        };

        let mut buf = [0u8; MTU];
        let written = state.write_proof(&owed, &mut buf).unwrap();
        assert_eq!(&buf[..written], hx(RNS_1_3_1_IMPLICIT_PROOF).as_slice());
    }

    #[test]
    fn write_proof_for_an_unheld_identity_reports_it() {
        let state: EngineState<Cap> = EngineState::<Cap>::default();
        let owed = ProofOwed {
            packet_hash: PacketHash::new([0xAA; 32]),
            identity: IdentityHash::new([0x4c; 16]),
        };
        let mut buf = [0u8; MTU];
        assert_eq!(
            state.write_proof(&owed, &mut buf),
            Err(WriteProofError::IdentityNotHeld),
        );
    }

    #[test]
    fn write_proof_into_a_short_buffer_reports_it() {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let owed = ProofOwed {
            packet_hash: PacketHash::new([0xAA; 32]),
            identity: held,
        };
        let mut buf = [0u8; 8];
        assert_eq!(
            state.write_proof(&owed, &mut buf),
            Err(WriteProofError::Serialize(
                EgressSerializeError::BufferTooShort
            )),
        );
    }
}
