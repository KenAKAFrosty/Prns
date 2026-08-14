use crate::crypto::{ed25519_sign, Ed25519Signature};
use crate::engine::{
    CommandId, DeliveryEvidence, DeliveryProof, EngineState, InstantMillis, PacketReceiptDelivered,
    ProofForm,
};
use crate::identity::IdentitySigner;
use crate::routing::dedup::{PacketHash, PACKET_HASH_LEN};
use crate::routing::delivery::receipts::{ProvenReceipt, ReceiptKind};
use crate::routing::links::table::{LinkPhase, LinkRole};
use crate::routing::links::LinkId;
use crate::routing::proof::{
    write_explicit_proof_wire_packet, write_implicit_proof_wire_packet,
    write_link_proof_wire_packet, DeferredProof, LinkProofOwed, ProofIngest, ProofOwed,
    WriteChannelAckError, WriteProofError, EXPLICIT_PROOF_PAYLOAD_LEN, IMPLICIT_PROOF_PAYLOAD_LEN,
};
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::{DestinationHash, WireError};

impl<S: StorageLayout> EngineState<S> {
    /// Best-effort by RNS 1.4.2 parity: an unwritable proof is dropped; the sender's timeout-and-resend is the designed recovery, so nothing here is retried.
    pub fn write_proof(&self, owed: &ProofOwed, buf: &mut [u8]) -> Result<usize, WriteProofError> {
        let identity = self
            .held_identities
            .get(&owed.identity)
            .ok_or(WriteProofError::IdentityNotHeld)?;
        let signature = identity.sign(owed.packet_hash.as_bytes());
        self.write_signed_proof(&owed.packet_hash, &signature, buf)
            .map_err(WriteProofError::Serialize)
    }

    pub fn write_signed_proof(
        &self,
        packet_hash: &PacketHash,
        signature: &Ed25519Signature,
        buf: &mut [u8],
    ) -> Result<usize, WireError> {
        match self.protocol.proof_form {
            ProofForm::Implicit => write_implicit_proof_wire_packet(packet_hash, signature, buf),
            ProofForm::Explicit => write_explicit_proof_wire_packet(packet_hash, signature, buf),
        }
    }

    /// Best-effort by RNS 1.4.2 parity: an unwritable link proof is dropped; the initiator's timeout is the designed recovery.
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

    /// RNS 1.4.2 `Link.receive`'s CHANNEL branch: `packet.prove()` whenever a channel is open, on either side.
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

    /// RNS 1.4.2 `PacketReceipt.validate_proof`, both forms. Settlement removes the receipt, so a replayed proof finds nothing; exactly-once is structural.
    pub fn settle_receipt_proof(
        &mut self,
        payload: &[u8],
        proof_packet_hash: PacketHash,
        arrived_at: InstantMillis,
    ) -> ProofIngest {
        let (proven, proof) = match payload.len() {
            EXPLICIT_PROOF_PAYLOAD_LEN => {
                let (named_hash, signature) = payload.split_at(PACKET_HASH_LEN);
                let (Ok(named_hash), Ok(signature)) = (named_hash.try_into(), signature.try_into())
                else {
                    return ProofIngest::Ignored;
                };
                (
                    self.receipts.settle_by_explicit_proof(
                        &PacketHash::new(named_hash),
                        &Ed25519Signature(signature),
                    ),
                    DeliveryProof::Explicit(proof_packet_hash),
                )
            }
            IMPLICIT_PROOF_PAYLOAD_LEN => {
                let Ok(signature) = payload.try_into() else {
                    return ProofIngest::Ignored;
                };
                (
                    self.receipts
                        .settle_by_implicit_proof(&Ed25519Signature(signature)),
                    DeliveryProof::Implicit(proof_packet_hash),
                )
            }
            _ => return ProofIngest::Ignored,
        };
        match proven {
            Some(receipt) => {
                let delivered = PacketReceiptDelivered {
                    rtt: RttMillis::measured_between(receipt.sent_at, arrived_at),
                    evidence: DeliveryEvidence::Proof(proof),
                };
                match receipt.kind {
                    ReceiptKind::SendSinglePacket => ProofIngest::SendSinglePacketDelivered {
                        id: receipt.command_id,
                        delivered,
                    },
                    ReceiptKind::SendToLink(link_id) => {
                        self.links.note_inbound(&link_id, arrived_at);
                        ProofIngest::SendToLinkDelivered {
                            id: receipt.command_id,
                            delivered,
                        }
                    }
                    ReceiptKind::SendRequest { .. } => ProofIngest::Ignored,
                }
            }
            None => ProofIngest::Ignored,
        }
    }

    pub fn settle_receipt_proof_deferred(
        &mut self,
        payload: &[u8],
        proof_destination: &DestinationHash,
        proof_packet_hash: PacketHash,
        arrived_at: InstantMillis,
    ) -> Option<DeferredProof> {
        let (resolved, signature, proof) = match payload.len() {
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
                    DeliveryProof::Explicit(proof_packet_hash),
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
                    DeliveryProof::Implicit(proof_packet_hash),
                )
            }
            _ => return None,
        };
        let resolved = resolved?;
        let delivered = PacketReceiptDelivered {
            rtt: RttMillis::measured_between(resolved.proven.sent_at, arrived_at),
            evidence: DeliveryEvidence::Proof(proof),
        };
        let ingest = match resolved.proven.kind {
            ReceiptKind::SendSinglePacket => ProofIngest::SendSinglePacketDelivered {
                id: resolved.proven.command_id,
                delivered,
            },
            ReceiptKind::SendToLink(link_id) => {
                self.links.note_unverified_inbound(&link_id, arrived_at);
                ProofIngest::SendToLinkDelivered {
                    id: resolved.proven.command_id,
                    delivered,
                }
            }
            ReceiptKind::SendRequest { .. } => return None,
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
mod tests;
