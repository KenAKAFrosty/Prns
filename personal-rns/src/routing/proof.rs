use crate::crypto::Ed25519Signature;
use crate::engine::commands::{CommandId, Delivered};
use crate::engine::egress::EgressSerializeError;
use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::links::LinkId;
use crate::wire::{DestinationHash, HEADER_MIN_LEN, SIGNATURE_LEN};

pub const IMPLICIT_PROOF_WIRE_LEN: usize = HEADER_MIN_LEN + SIGNATURE_LEN;

/// RNS 1.3.1 `PacketReceipt.IMPL_LENGTH`
pub const IMPLICIT_PROOF_PAYLOAD_LEN: usize = SIGNATURE_LEN;
/// RNS 1.3.1 `PacketReceipt.EXPL_LENGTH`
pub const EXPLICIT_PROOF_PAYLOAD_LEN: usize = PACKET_HASH_LEN + SIGNATURE_LEN;

/// A packet proof over a link is always the explicit form (RNS 1.3.1
/// `Link.prove_packet`: "hardcoded as explicit proof for now").
pub const LINK_PROOF_WIRE_LEN: usize = HEADER_MIN_LEN + EXPLICIT_PROOF_PAYLOAD_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofIngest {
    SendSingleDelivered { id: CommandId, delivered: Delivered },
    SendLinkDelivered { id: CommandId, delivered: Delivered },
    Ignored,
}

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

/// The proof a delivered link packet earned from the responder — RNS 1.3.1
/// `Link.prove_packet`: 96 unencrypted bytes (`packet_hash ‖ sig(packet_hash)`)
/// answered to the link destination, signed by the registered identity the link
/// responds for. Only the responder ever owes one: the initiator's side of a
/// link is a remote destination, and a remote destination never proves.
/// `destination` is what a deferred decider is asked about.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteProofError {
    IdentityNotHeld,
    Serialize(EgressSerializeError),
}

use crate::engine::egress::{write_implicit_proof_wire_packet, write_link_proof_wire_packet};
use crate::engine::EngineState;
use crate::identity::IdentitySigner;
use crate::routing::delivery::receipts::ReceiptKind;
use crate::storage::StorageLayout;

impl<S: StorageLayout> EngineState<S> {
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

    /// Sign and frame the proof a delivered link packet earned
    /// ([`LinkProofOwed`], same one-cycle custody as [`ProofOwed`]) into `buf`.
    /// The same best-effort posture applies: the initiator's timeout is the
    /// designed recovery for a proof that can't be written.
    pub fn write_link_proof(
        &self,
        owed: &LinkProofOwed,
        buf: &mut [u8],
    ) -> Result<usize, WriteProofError> {
        let identity = self
            .held_identities
            .get(&owed.identity)
            .ok_or(WriteProofError::IdentityNotHeld)?;
        let signature = identity.sign(owed.packet_hash.as_bytes());
        write_link_proof_wire_packet(&owed.link_id, &owed.packet_hash, &signature, buf)
            .map_err(WriteProofError::Serialize)
    }

    /// An arriving proof settles the outstanding send it validates. RNS 1.3.1
    /// `PacketReceipt.validate_proof` for both forms. Settlement removes the
    /// receipt, so a replayed proof finds nothing; exactly-once is structural.
    pub fn ingest_proof(&mut self, payload: &[u8], arrived_at: InstantMillis) -> ProofIngest {
        let proven = match payload.len() {
            EXPLICIT_PROOF_PAYLOAD_LEN => {
                let (named_hash, signature) = payload.split_at(PACKET_HASH_LEN);
                let (Ok(named_hash), Ok(signature)) = (named_hash.try_into(), signature.try_into())
                else {
                    return ProofIngest::Ignored;
                };
                self.receipts.settle_by_explicit_proof(
                    &PacketHash::new(named_hash),
                    &Ed25519Signature(signature),
                )
            }
            IMPLICIT_PROOF_PAYLOAD_LEN => {
                let Ok(signature) = payload.try_into() else {
                    return ProofIngest::Ignored;
                };
                self.receipts
                    .settle_by_implicit_proof(&Ed25519Signature(signature))
            }
            _ => return ProofIngest::Ignored,
        };
        match proven {
            Some(receipt) => {
                let delivered = Delivered {
                    rtt_ms: arrived_at.0.saturating_sub(receipt.sent_at.0),
                };
                match receipt.kind {
                    ReceiptKind::SendSingle => ProofIngest::SendSingleDelivered {
                        id: receipt.command_id,
                        delivered,
                    },
                    ReceiptKind::SendLink => ProofIngest::SendLinkDelivered {
                        id: receipt.command_id,
                        delivered,
                    },
                    ReceiptKind::SendRequest => ProofIngest::Ignored,
                }
            }
            None => ProofIngest::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::{
        Directive, EngineReaction, EngineState, IngestPacketOutcome, RatchetPolicy,
    };
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::dedup::PacketHash;
    use crate::routing::delivery::Delivery;
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::BROADCAST_MTU;

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
                b"",
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let mut raw = sealed_single_packet(&identity, destination, b"proof-parity");
        assert_eq!(raw, hx(RAW_SEALED_FOR_PROOF));

        let outcome = state.ingest_packet(
            plain_data_packet(&mut raw),
            TEST_ENTROPY,
            &transporting_view(),
        );
        let IngestPacketOutcome::Delivery {
            proof: ProofObligation::Owed(owed),
            ..
        } = outcome
        else {
            panic!("a ProveAll delivery owes a proof");
        };

        let mut buf = [0u8; BROADCAST_MTU];
        let written = state.write_proof(&owed, &mut buf).unwrap();
        assert_eq!(&buf[..written], hx(RNS_1_3_1_IMPLICIT_PROOF).as_slice());
    }

    fn prove_if_state() -> (
        EngineState<Cap>,
        InMemoryNodeIdentity,
        crate::wire::DestinationHash,
    ) {
        let mut state: EngineState<Cap> = EngineState::<Cap>::default();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let held = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &held,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveIf,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        (state, identity, destination)
    }

    #[test]
    fn a_prove_if_delivery_defers_the_proof_to_the_app() {
        let (mut state, identity, destination) = prove_if_state();
        let mut raw = sealed_single_packet(&identity, destination, b"prove-if");
        let IngestPacketOutcome::Delivery {
            delivery: Delivery::Single(single),
            proof: ProofObligation::OwedIfApp(_),
        } = state.ingest_packet(
            plain_data_packet(&mut raw),
            TEST_ENTROPY,
            &transporting_view(),
        )
        else {
            panic!("a ProveIf delivery defers its proof to the app");
        };
        assert_eq!(
            single.plaintext, b"prove-if",
            "the deferred decision sees the decrypted content",
        );
    }

    fn prove_if_proof_directive(
        decide: impl FnMut(&ProofRequest) -> bool,
    ) -> (bool, std::vec::Vec<u8>) {
        let (mut state, identity, destination) = prove_if_state();
        let mut raw = sealed_single_packet(&identity, destination, b"prove-if");
        let mut decide = decide;
        let mut seen = std::vec::Vec::new();
        let mut proved = false;
        state.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(1_000),
                source_interface: InterfaceId::new([0xEE; 16]),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &transporting_view(),
            InstantMillis(1_000),
            &mut |bytes| bytes.fill(0),
            &mut |request| {
                seen = request.plaintext.to_vec();
                decide(request)
            },
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { .. }) = reaction {
                    proved = true;
                }
            },
        );
        (proved, seen)
    }

    #[test]
    fn the_app_decider_gates_the_prove_if_proof() {
        let (proved, seen) = prove_if_proof_directive(|_| true);
        assert!(proved, "the decider agreed, so the reactor answers a proof");
        assert_eq!(seen, b"prove-if", "the decider sees the decrypted content");

        let (proved, _) = prove_if_proof_directive(|_| false);
        assert!(!proved, "the decider declined, so no proof goes out");
    }

    #[test]
    fn write_proof_for_an_unheld_identity_reports_it() {
        let state: EngineState<Cap> = EngineState::<Cap>::default();
        let owed = ProofOwed {
            packet_hash: PacketHash::new([0xAA; 32]),
            identity: IdentityHash::new([0x4c; 16]),
        };
        let mut buf = [0u8; BROADCAST_MTU];
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
