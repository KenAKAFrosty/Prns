//! Small synchronous runtime for protocol tests that are not themselves testing crypto
//! scheduling. It fulfills typed crypto directives after the originating step has returned and
//! forwards resource directives to the test's sink.

use std::collections::VecDeque;

use crate::crypto::{ed25519_sign, ed25519_verify, x25519_diffie_hellman, x25519_keys_for_seal};
use crate::engine::{
    CryptoOwed, Directive, EncryptCompleted, EngineReaction, EngineState, IngestIo,
    LinkReceiptSignCompleted, OwedWork, ProofSignCompleted, ReceiptProofVerification,
    WakeSchedules,
};
use crate::identity::decrypt_token_in_place_with_ratchets;
use crate::interfaces::InboundPacket;
use crate::remote_control::RemoteControlPairingAvailabilityVerification;
use crate::routing::announce::Announce;
use crate::routing::links::handshake::{link_proof_signature_valid, link_proof_signed_data};
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;

/// Drive one packet and every immediately ready crypto continuation to quiescence. Tests that
/// assert a continuation boundary call the engine directly instead of using this runtime.
pub(crate) fn drive_packet_to_quiescence<S, F, P, A, K>(
    engine: &mut EngineState<S>,
    packet: InboundPacket<'_>,
    io: IngestIo<'_, F, P, A, K>,
) -> WakeSchedules
where
    S: StorageLayout,
    F: FnMut(&mut [u8]),
    P: FnMut(&crate::routing::proof::ProofRequest) -> bool,
    A: FnMut(&crate::routing::links::resources::ResourceOffer) -> bool,
    K: FnMut(EngineReaction<'_, OwedWork<'_>>),
{
    let IngestIo {
        interfaces,
        now,
        fill_random,
        should_prove,
        should_accept_resource,
        sink,
    } = io;
    let mut ready = VecDeque::new();
    let report = engine.ingest_packet_into_report(
        packet,
        IngestIo {
            interfaces,
            now,
            fill_random: &mut *fill_random,
            should_prove: &mut *should_prove,
            should_accept_resource: &mut *should_accept_resource,
            sink: &mut |reaction| route_or_capture_crypto(reaction, &mut ready, sink),
        },
    );
    let mut wake = report.wake_schedules;

    while let Some(work) = ready.pop_front() {
        match work {
            CryptoOwed::ReceiptProofVerify(owed) => {
                let verification = if ed25519_verify(
                    owed.signing_key.as_ed25519(),
                    owed.packet_hash.as_bytes(),
                    &owed.signature,
                )
                .is_ok()
                {
                    ReceiptProofVerification::Valid
                } else {
                    ReceiptProofVerification::Invalid
                };
                wake.compose(
                    engine.resume_receipt_proof(owed, verification, &mut |reaction| {
                        sink(reaction.map_work(|never| match never {}))
                    }),
                );
            }
            CryptoOwed::Encrypt(owed) => {
                let (ephemeral_public, shared) =
                    x25519_keys_for_seal(&owed.ephemeral_secret, &owed.dh_target);
                let mut wire = [0u8; BROADCAST_MTU];
                wake.compose(engine.resume_encrypt(
                    EncryptCompleted {
                        owed,
                        ephemeral_public,
                        shared,
                    },
                    interfaces,
                    &mut wire,
                    &mut |reaction| sink(reaction.map_work(|never| match never {})),
                ));
            }
            CryptoOwed::Decrypt(owed) => {
                let shared = x25519_diffie_hellman(&owed.encryption_secret, &owed.ephemeral_public);
                engine.resume_decrypt(owed, shared, interfaces, should_prove, &mut |reaction| {
                    route_or_capture_crypto(reaction, &mut ready, sink)
                });
            }
            CryptoOwed::RatchetDecrypt(mut owed) => {
                if let Ok(opened) = decrypt_token_in_place_with_ratchets(
                    &owed.ratchet_secrets,
                    &owed.encryption_secret,
                    &owed.identity,
                    owed.identity_key_fallback,
                    &mut owed.token,
                ) {
                    let mut plaintext = heapless::Vec::<
                        u8,
                        { crate::routing::ingress::MAX_RATCHET_DECRYPT_PAYLOAD_LEN },
                    >::new();
                    if plaintext.extend_from_slice(opened.plaintext).is_ok() {
                        let opened_by = opened.opened_by;
                        engine.resume_ratchet_decrypt(
                            owed,
                            crate::identity::OpenedToken {
                                opened_by,
                                plaintext: &plaintext,
                            },
                            interfaces,
                            should_prove,
                            &mut |reaction| route_or_capture_crypto(reaction, &mut ready, sink),
                        );
                    }
                }
            }
            CryptoOwed::LinkProofVerify(owed) => {
                if link_proof_signature_valid(&owed) {
                    let shared =
                        x25519_diffie_hellman(&owed.initiator_secret, &owed.responder_encryption);
                    wake.compose(engine.resume_link_proof(
                        owed,
                        shared,
                        interfaces,
                        now,
                        fill_random,
                        &mut |reaction| sink(reaction.map_work(|never| match never {})),
                    ));
                }
            }
            CryptoOwed::LinkProofSign(owed) => {
                let (responder_encryption, shared) = x25519_keys_for_seal(
                    &owed.ephemeral_secret,
                    &owed.request.initiator_encryption,
                );
                let signed_data = link_proof_signed_data(
                    &owed.request.link_id,
                    &responder_encryption,
                    owed.responder_signing.as_ed25519(),
                    owed.mtu,
                    owed.request.mode,
                );
                let signature = ed25519_sign(&owed.signing_secret, &signed_data);
                wake.compose(engine.resume_link_proof_sign(
                    owed,
                    responder_encryption,
                    shared,
                    signature,
                    interfaces,
                    &mut |reaction| sink(reaction.map_work(|never| match never {})),
                ));
            }
            CryptoOwed::ProofSign(owed) => {
                let signature = ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
                engine.resume_proof_sign(
                    ProofSignCompleted {
                        target: owed.target,
                        packet_hash: owed.packet_hash,
                        signature,
                    },
                    &mut |reaction| {
                        sink(reaction.map_work(|never| match never {}));
                    },
                );
            }
            CryptoOwed::LinkReceiptSign(owed) => {
                let signature = ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
                engine.resume_link_receipt_sign(
                    LinkReceiptSignCompleted {
                        target: owed.target,
                        link_id: owed.link_id,
                        packet_hash: owed.packet_hash,
                        signature,
                    },
                    now,
                    &mut |reaction| {
                        sink(reaction.map_work(|never| match never {}));
                    },
                );
            }
            CryptoOwed::AnnounceVerify(owed) => {
                if Announce::from_wire_unverified(&owed.header, &owed.payload)
                    .is_ok_and(|announce| announce.signature_is_valid())
                {
                    wake.compose(engine.resume_announce(
                        owed,
                        interfaces,
                        fill_random,
                        &mut |reaction| sink(reaction.map_work(|never| match never {})),
                    ));
                }
            }
            CryptoOwed::RemoteControlPairingAvailabilityVerify(owed) => {
                if owed.verify() == RemoteControlPairingAvailabilityVerification::Valid {
                    wake.compose(engine.resume_remote_control_pairing_availability(
                        owed,
                        interfaces,
                        &mut |reaction| sink(reaction.map_work(|never| match never {})),
                    ));
                }
            }
        }
    }

    wake
}

fn route_or_capture_crypto(
    reaction: EngineReaction<'_, OwedWork<'_>>,
    ready: &mut VecDeque<CryptoOwed>,
    sink: &mut impl FnMut(EngineReaction<'_, OwedWork<'_>>),
) {
    match reaction {
        EngineReaction::Directive(Directive::Fulfill(OwedWork::Crypto(work))) => {
            ready.push_back(work);
        }
        other => sink(other),
    }
}
