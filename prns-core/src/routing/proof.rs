use crate::crypto::{ed25519_sign, Ed25519SecretKey, Ed25519Signature};
use crate::engine::EgressSerializeError;
use crate::engine::InstantMillis;
use crate::engine::{CommandId, PacketReceiptDelivered};
use crate::identity::{IdentityHash, IdentitySigningPublicKey};
use crate::interfaces::InterfaceId;
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::links::table::{LinkPhase, LinkRole};
use crate::routing::links::LinkId;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, HEADER_MIN_LEN, SIGNATURE_BYTE_LEN};

pub const IMPLICIT_PROOF_WIRE_LEN: usize = HEADER_MIN_LEN + SIGNATURE_BYTE_LEN;

/// RNS 1.3.5 `PacketReceipt.IMPL_LENGTH`
pub const IMPLICIT_PROOF_PAYLOAD_LEN: usize = SIGNATURE_BYTE_LEN;
/// RNS 1.3.5 `PacketReceipt.EXPL_LENGTH`
pub const EXPLICIT_PROOF_PAYLOAD_LEN: usize = PACKET_HASH_LEN + SIGNATURE_BYTE_LEN;

/// A packet proof over a link is always the explicit form (RNS 1.3.5
/// `Link.prove_packet`: "hardcoded as explicit proof for now").
pub const LINK_PROOF_WIRE_LEN: usize = HEADER_MIN_LEN + EXPLICIT_PROOF_PAYLOAD_LEN;

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

/// Carried in the ingest outcome so it lives exactly one cycle; the engine keeps
/// no proof state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofOwed {
    pub packet_hash: PacketHash,
    pub identity: IdentityHash,
}

/// RNS 1.3.5 `Link.prove_packet`: 96 unencrypted bytes (`packet_hash ‖ sig(packet_hash)`).
/// Only the responder ever owes one; the initiator's side is a remote destination,
/// and a remote destination never proves.
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
    /// RNS 1.3.5 `Transport.inbound`'s local-delivery leg: `PROVE_ALL` proves every delivered packet, `PROVE_APP` asks the app first, `PROVE_NONE` never proves.
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
    Serialize(EgressSerializeError),
}

/// The responder signs with the held destination identity; the initiator signs with
/// the link's own ephemeral key, so only the responder path can miss its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteChannelAckError {
    LinkNotActive,
    IdentityNotHeld,
    Serialize(EgressSerializeError),
}

use crate::engine::EngineState;
use crate::engine::{write_implicit_proof_wire_packet, write_link_proof_wire_packet};
use crate::identity::IdentitySigner;
use crate::routing::delivery::receipts::{ProvenReceipt, ReceiptKind};
use crate::storage::StorageLayout;

impl<S: StorageLayout> EngineState<S> {
    /// Best-effort by RNS 1.3.5 parity: an unwritable proof is dropped; the sender's
    /// timeout-and-resend is the designed recovery, so nothing here is retried.
    pub fn write_proof(&self, owed: &ProofOwed, buf: &mut [u8]) -> Result<usize, WriteProofError> {
        let identity = self
            .held_identities
            .get(&owed.identity)
            .ok_or(WriteProofError::IdentityNotHeld)?;
        let signature = identity.sign(owed.packet_hash.as_bytes());
        write_implicit_proof_wire_packet(&owed.packet_hash, &signature, buf)
            .map_err(WriteProofError::Serialize)
    }

    /// Same best-effort posture: the initiator's timeout is the designed recovery.
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

    /// RNS 1.3.5 `Link.receive`'s CHANNEL branch: `packet.prove()` whenever a channel
    /// is open, on either side.
    pub fn write_channel_ack(
        &self,
        link_id: &LinkId,
        packet_hash: &PacketHash,
        buf: &mut [u8],
    ) -> Result<usize, WriteChannelAckError> {
        let Some(LinkPhase::Active { role, .. }) = self.links.phase_for(link_id) else {
            return Err(WriteChannelAckError::LinkNotActive);
        };
        let signature = match role {
            LinkRole::Responder { identity, .. } => self
                .held_identities
                .get(identity)
                .ok_or(WriteChannelAckError::IdentityNotHeld)?
                .sign(packet_hash.as_bytes()),
            LinkRole::Initiator { link_signing } => {
                ed25519_sign(link_signing, packet_hash.as_bytes())
            }
        };
        write_link_proof_wire_packet(link_id, packet_hash, &signature, buf)
            .map_err(WriteChannelAckError::Serialize)
    }

    /// RNS 1.3.5 `PacketReceipt.validate_proof`, both forms. Settlement removes the
    /// receipt, so a replayed proof finds nothing; exactly-once is structural.
    pub fn settle_receipt_proof(
        &mut self,
        payload: &[u8],
        arrived_at: InstantMillis,
    ) -> ProofIngest {
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
                let delivered = PacketReceiptDelivered {
                    rtt: RttMillis::measured_between(receipt.sent_at, arrived_at),
                };
                match receipt.kind {
                    ReceiptKind::SendSinglePacket => ProofIngest::SendSinglePacketDelivered {
                        id: receipt.command_id,
                        delivered,
                    },
                    ReceiptKind::SendToLink => ProofIngest::SendToLinkDelivered {
                        id: receipt.command_id,
                        delivered,
                    },
                    ReceiptKind::SendRequest => ProofIngest::Ignored,
                }
            }
            None => ProofIngest::Ignored,
        }
    }

    pub fn settle_receipt_proof_deferred(
        &mut self,
        payload: &[u8],
        proof_destination: &DestinationHash,
        arrived_at: InstantMillis,
    ) -> Option<DeferredProof> {
        let (resolved, signature) = match payload.len() {
            EXPLICIT_PROOF_PAYLOAD_LEN => {
                let (named_hash, signature) = payload.split_at(PACKET_HASH_LEN);
                let (Ok(named_hash), Ok(signature)) = (named_hash.try_into(), signature.try_into())
                else {
                    return None;
                };
                let signature = Ed25519Signature(signature);
                (
                    self.receipts
                        .resolve_explicit_for_deferred_verify(&PacketHash::new(named_hash)),
                    signature,
                )
            }
            IMPLICIT_PROOF_PAYLOAD_LEN => {
                let Ok(signature) = payload.try_into() else {
                    return None;
                };
                let signature = Ed25519Signature(signature);
                (
                    self.receipts
                        .resolve_proof_by_destination(proof_destination),
                    signature,
                )
            }
            _ => return None,
        };
        let resolved = resolved?;
        let delivered = PacketReceiptDelivered {
            rtt: RttMillis::measured_between(resolved.proven.sent_at, arrived_at),
        };
        let ingest = match resolved.proven.kind {
            ReceiptKind::SendSinglePacket => ProofIngest::SendSinglePacketDelivered {
                id: resolved.proven.command_id,
                delivered,
            },
            ReceiptKind::SendToLink => ProofIngest::SendToLinkDelivered {
                id: resolved.proven.command_id,
                delivered,
            },
            ReceiptKind::SendRequest => return None,
        };
        Some(DeferredProof {
            ingest,
            packet_hash: resolved.packet_hash,
            signing_key: resolved.signing_key,
            signature,
        })
    }

    pub fn settle_resolved(&mut self, command_id: CommandId) -> Option<ProvenReceipt> {
        self.receipts.settle_resolved(command_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::IngestIo;
    use crate::engine::{
        Directive, EngineReaction, EngineState, IngestPacketOutcome, RatchetPolicy,
    };
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::interfaces::AttachedInterfaces;
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::dedup::PacketHash;
    use crate::routing::delivery::Delivery;
    use crate::routing::links::table::LinkActivation;
    use crate::routing::upstream_app_destinations::ProofStrategy;
    use crate::wire::BROADCAST_MTU;

    #[test]
    fn write_proof_is_byte_identical_to_the_rns_1_3_5_implicit_proof() {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
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
        assert_eq!(raw, bytes_from_hex(RNS_1_3_5_SEALED_FOR_PROOF));

        let outcome = state.ingest_packet_with(
            plain_data_packet(&mut raw),
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
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
        assert_eq!(
            &buf[..written],
            bytes_from_hex(RNS_1_3_5_IMPLICIT_PROOF).as_slice()
        );
    }

    fn prove_if_state() -> (
        EngineState<TestStorageLayout>,
        InMemoryNodeIdentity,
        crate::wire::DestinationHash,
    ) {
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
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
        } = state.ingest_packet_with(
            plain_data_packet(&mut raw),
            &mut |_| {},
            AttachedInterfaces::new(&transporting_interfaces()),
            &mut |_| {},
            None,
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
                source_interface: InterfaceId::new([0xEE; 8]),
                bytes: &mut raw,
            },
            IngestIo {
                interfaces: AttachedInterfaces::new(&transporting_interfaces()),
                now: InstantMillis(1_000),
                fill_entropy: &mut |bytes| bytes.fill(0),
                should_prove: &mut |request| {
                    seen = request.plaintext.to_vec();
                    decide(request)
                },
                sink: &mut |reaction| {
                    if let EngineReaction::Directive(Directive::Send { .. }) = reaction {
                        proved = true;
                    }
                },
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
        let state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
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
        let mut state: EngineState<TestStorageLayout> = EngineState::<TestStorageLayout>::default();
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

    #[test]
    fn an_initiator_channel_ack_is_signed_by_the_link_key() {
        use crate::crypto::{
            ed25519_public_key, ed25519_verify, x25519_diffie_hellman, Ed25519PublicKey,
            Ed25519SecretKey, X25519PublicKey, X25519SecretKey,
        };
        use crate::engine::CommandId;
        use crate::routing::links::table::InitiatedLink;
        use crate::routing::links::{LinkId, LinkKey};

        let mut state = EngineState::<TestStorageLayout>::default();
        let link_id = LinkId::new([0x5C; 16]);
        let link_signing = Ed25519SecretKey::new([0x42; 32]);
        let link_signing_public = ed25519_public_key(&link_signing);
        state
            .links
            .track_initiated(InitiatedLink {
                link_id,
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing,
                requested_at: InstantMillis(0),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new([0x33; 32]),
            &X25519PublicKey([0x44; 32]),
        );
        state
            .links
            .activate_initiated(
                &link_id,
                LinkKey::derive(&link_id, &shared),
                &LinkActivation {
                    rtt: crate::units::RttMillis::new(250),
                    mtu: BROADCAST_MTU,
                    attached_interface: InterfaceId::new([0xEE; 8]),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(1_000),
            )
            .unwrap();

        let packet_hash = PacketHash::new([0xAB; 32]);
        let mut buf = [0u8; BROADCAST_MTU];
        let written = state
            .write_channel_ack(&link_id, &packet_hash, &mut buf)
            .unwrap();
        assert_eq!(written, LINK_PROOF_WIRE_LEN);
        assert_eq!(
            &buf[HEADER_MIN_LEN..HEADER_MIN_LEN + PACKET_HASH_LEN],
            packet_hash.as_bytes(),
            "the proof names the packet it acks",
        );
        let signature = Ed25519Signature(
            buf[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN]
                .try_into()
                .unwrap(),
        );
        ed25519_verify(&link_signing_public, packet_hash.as_bytes(), &signature)
            .expect("the initiator signs the ack with its own ephemeral link key");
    }

    #[test]
    fn a_responder_channel_ack_is_signed_by_the_held_identity() {
        use crate::crypto::{
            ed25519_verify, x25519_diffie_hellman, Ed25519PublicKey, X25519PublicKey,
            X25519SecretKey,
        };
        use crate::identity::IdentitySigner;
        use crate::routing::links::table::RespondingLink;
        use crate::routing::links::{LinkId, LinkKey};

        let mut state = EngineState::<TestStorageLayout>::default();
        let identity = state.hold_identity(fixed_secret_key()).unwrap();
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&fixed_secret_key());
        let signing_public = *signer.signing_public_key().as_ed25519();

        let link_id = LinkId::new([0x6D; 16]);
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new([0x55; 32]),
            &X25519PublicKey([0x66; 32]),
        );
        state
            .links
            .track_responding(RespondingLink {
                link_id,
                key: LinkKey::derive(&link_id, &shared),
                requested_at: InstantMillis(0),
                timeout_at: InstantMillis(5_000),
                mtu: BROADCAST_MTU,
                initiator_signing: Ed25519PublicKey([0x99; 32]),
                destination: DestinationHash::new([0x77; 16]),
                identity,
                proof_strategy: ProofStrategy::ProveAll,
            })
            .unwrap();
        state
            .links
            .activate_responding(
                &link_id,
                crate::units::RttMillis::new(250),
                InterfaceId::new([0xEE; 8]),
                InstantMillis(1_000),
            )
            .unwrap();

        let packet_hash = PacketHash::new([0xCD; 32]);
        let mut buf = [0u8; BROADCAST_MTU];
        let written = state
            .write_channel_ack(&link_id, &packet_hash, &mut buf)
            .unwrap();
        assert_eq!(written, LINK_PROOF_WIRE_LEN);
        let signature = Ed25519Signature(
            buf[HEADER_MIN_LEN + PACKET_HASH_LEN..LINK_PROOF_WIRE_LEN]
                .try_into()
                .unwrap(),
        );
        ed25519_verify(&signing_public, packet_hash.as_bytes(), &signature)
            .expect("the responder signs the ack with the destination identity it answers for");
    }

    #[test]
    fn a_channel_ack_for_an_inactive_link_reports_it() {
        use crate::routing::links::LinkId;

        let state = EngineState::<TestStorageLayout>::default();
        let mut buf = [0u8; BROADCAST_MTU];
        assert_eq!(
            state.write_channel_ack(
                &LinkId::new([0x01; 16]),
                &PacketHash::new([0u8; 32]),
                &mut buf
            ),
            Err(WriteChannelAckError::LinkNotActive),
        );
    }
}
