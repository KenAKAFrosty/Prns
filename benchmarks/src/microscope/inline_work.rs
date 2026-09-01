use heapless::Deque;
use personal_rns::crypto::{
    ed25519_sign, ed25519_verify, x25519_diffie_hellman, x25519_keys_for_seal,
};
use personal_rns::engine::{
    CryptoOwed, Directive, EncryptCompleted, EngineReaction, EngineState, IngestIo, InstantMillis,
    LinkReceiptSignCompleted, NoOwedWork, OwedWork, ProofSignCompleted, ReceiptProofVerification,
    ResourceDecompressionCompleted, ResourceOpenCompleted,
};
use personal_rns::identity::{decrypt_token_in_place_with_ratchets, OpenedToken};
use personal_rns::interfaces::{AttachedInterfaces, InboundPacket};
use personal_rns::remote_control::RemoteControlPairingAvailabilityVerification;
use personal_rns::routing::announce::Announce;
use personal_rns::routing::links::handshake::{link_proof_signature_valid, link_proof_signed_data};
use personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError;
use personal_rns::routing::links::resources::send::ResourceBuildCompleted;
use personal_rns::routing::links::resources::table::ResourceBuildReservation;
use personal_rns::routing::links::resources::ResourceHash;
use personal_rns::routing::links::LinkId;
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::BROADCAST_MTU;

use super::{FeedCapture, Splitmix};

enum ReadyWork {
    Crypto(CryptoOwed),
    ResourceBuildUnsupported {
        reservation: ResourceBuildReservation,
    },
    ResourceOpen(ResourceOpenCompleted<'static>),
    ResourceDecompressionUnsupported {
        link_id: LinkId,
        hash: ResourceHash,
    },
}

const MAX_READY_WORK_PER_STEP: usize = 2;
type ReadyWorkQueue = Deque<ReadyWork, MAX_READY_WORK_PER_STEP>;

fn route_or_capture_work(
    reaction: EngineReaction<'_, OwedWork<'_>>,
    capture: &mut FeedCapture,
    scratch: &mut Vec<u8>,
    ready: &mut ReadyWorkQueue,
) {
    match reaction {
        EngineReaction::Directive(Directive::Fulfill(work)) => {
            let ready_work = match work {
                OwedWork::Crypto(owed) => ReadyWork::Crypto(owed),
                OwedWork::ResourceBuild(owed) => ReadyWork::ResourceBuildUnsupported {
                    reservation: owed.reservation(),
                },
                OwedWork::ResourceOpen(owed) => ReadyWork::ResourceOpen(owed.fulfill_inline()),
                OwedWork::ResourceDecompression(owed) => {
                    ReadyWork::ResourceDecompressionUnsupported {
                        link_id: owed.link_id,
                        hash: owed.hash,
                    }
                }
            };
            assert!(
                ready.push_back(ready_work).is_ok(),
                "one microscope step exceeded the two owed-work continuation bound",
            );
        }
        other => capture.absorb(other, scratch),
    }
}

/// Drives every immediately ready continuation without waiting or recursively re-entering the
/// engine from its reaction sink. This is the synchronous manifold used by the pure-engine
/// microscope, not a production scheduling policy.
#[allow(clippy::too_many_arguments)]
pub(super) fn feed_packet_inline(
    engine: &mut EngineState<GrowableHeap>,
    packet: InboundPacket<'_>,
    interfaces: AttachedInterfaces<'_>,
    now: InstantMillis,
    entropy: &mut Splitmix,
    capture: &mut FeedCapture,
    scratch: &mut Vec<u8>,
) {
    let mut ready = ReadyWorkQueue::new();
    engine.ingest_packet_into(
        packet,
        IngestIo {
            interfaces,
            now,
            fill_random: &mut |bytes| entropy.fill(bytes),
            should_prove: &mut |_| true,
            should_accept_resource: &mut |_| false,
            sink: &mut |reaction| route_or_capture_work(reaction, capture, scratch, &mut ready),
        },
    );

    while let Some(work) = ready.pop_front() {
        match work {
            ReadyWork::Crypto(crypto) => match crypto {
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
                    engine.resume_receipt_proof(owed, verification, &mut |reaction| {
                        capture.absorb(reaction, scratch)
                    });
                }
                CryptoOwed::Encrypt(owed) => {
                    let (ephemeral_public, shared) =
                        x25519_keys_for_seal(&owed.ephemeral_secret, &owed.dh_target);
                    let mut wire = [0u8; BROADCAST_MTU];
                    engine.resume_encrypt(
                        EncryptCompleted {
                            owed,
                            ephemeral_public,
                            shared,
                        },
                        interfaces,
                        &mut wire,
                        &mut |reaction| capture.absorb(reaction, scratch),
                    );
                }
                CryptoOwed::Decrypt(owed) => {
                    let shared =
                        x25519_diffie_hellman(&owed.encryption_secret, &owed.ephemeral_public);
                    engine.resume_decrypt(
                        owed,
                        shared,
                        interfaces,
                        &mut |_| true,
                        &mut |reaction| {
                            route_or_capture_work(reaction, capture, scratch, &mut ready)
                        },
                    );
                }
                CryptoOwed::RatchetDecrypt(mut owed) => {
                    if let Ok(opened) = decrypt_token_in_place_with_ratchets(
                        &owed.ratchet_secrets,
                        &owed.encryption_secret,
                        &owed.identity,
                        owed.identity_key_fallback,
                        &mut owed.token,
                    ) {
                        let opened_by = opened.opened_by;
                        let plaintext = opened.plaintext.to_vec();
                        engine.resume_ratchet_decrypt(
                            owed,
                            OpenedToken {
                                opened_by,
                                plaintext: &plaintext,
                            },
                            interfaces,
                            &mut |_| true,
                            &mut |reaction| {
                                route_or_capture_work(reaction, capture, scratch, &mut ready)
                            },
                        );
                    }
                }
                CryptoOwed::LinkProofVerify(owed) => {
                    if link_proof_signature_valid(&owed) {
                        let shared = x25519_diffie_hellman(
                            &owed.initiator_secret,
                            &owed.responder_encryption,
                        );
                        engine.resume_link_proof(
                            owed,
                            shared,
                            interfaces,
                            now,
                            &mut |bytes| entropy.fill(bytes),
                            &mut |reaction| capture.absorb(reaction, scratch),
                        );
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
                    engine.resume_link_proof_sign(
                        owed,
                        responder_encryption,
                        shared,
                        signature,
                        interfaces,
                        &mut |reaction| capture.absorb(reaction, scratch),
                    );
                }
                CryptoOwed::ProofSign(owed) => {
                    let signature = ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
                    engine.resume_proof_sign(
                        ProofSignCompleted {
                            target: owed.target,
                            packet_hash: owed.packet_hash,
                            signature,
                        },
                        &mut |reaction| capture.absorb(reaction, scratch),
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
                        &mut |reaction| capture.absorb(reaction, scratch),
                    );
                }
                CryptoOwed::AnnounceVerify(owed) => {
                    if Announce::from_wire_unverified(&owed.header, &owed.payload)
                        .is_ok_and(|announce| announce.signature_is_valid())
                    {
                        engine.resume_announce(
                            owed,
                            interfaces,
                            &mut |bytes| entropy.fill(bytes),
                            &mut |reaction| capture.absorb(reaction, scratch),
                        );
                    }
                }
                CryptoOwed::RemoteControlPairingAvailabilityVerify(owed) => {
                    if owed.verify() == RemoteControlPairingAvailabilityVerification::Valid {
                        engine.resume_remote_control_pairing_availability(
                            owed,
                            interfaces,
                            &mut |reaction| capture.absorb(reaction, scratch),
                        );
                    }
                }
            },
            ReadyWork::ResourceBuildUnsupported { reservation } => {
                engine.resume_resource_build(
                    ResourceBuildCompleted {
                        reservation,
                        transfer: &[],
                        names: &[],
                        request_data: &[],
                        outcome: Err(BuildOutgoingResourceError::BufferShapeMismatch),
                    },
                    now,
                    &mut |bytes| entropy.fill(bytes),
                    &mut |reaction: EngineReaction<'_, NoOwedWork>| {
                        capture.absorb(reaction, scratch)
                    },
                );
            }
            ReadyWork::ResourceOpen(completed) => {
                engine.resume_resource_open(completed, now, &mut |reaction| {
                    route_or_capture_work(reaction, capture, scratch, &mut ready)
                });
            }
            ReadyWork::ResourceDecompressionUnsupported { link_id, hash } => {
                engine.resume_resource_decompression(
                    ResourceDecompressionCompleted {
                        link_id,
                        hash,
                        plaintext: &[],
                    },
                    now,
                    &mut |reaction: EngineReaction<'_, NoOwedWork>| {
                        capture.absorb(reaction, scratch)
                    },
                );
            }
        }
    }
}
