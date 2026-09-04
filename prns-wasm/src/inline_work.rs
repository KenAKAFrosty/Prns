//! Synchronous fulfillment policy for the browser manifold.
//!
//! The engine only describes owed work. This module is the Wasm runtime's current scheduling
//! choice: materialize borrowed inputs while the engine call is live, fulfill already-ready work
//! inline after that borrow ends, then return each completion through the caller's reaction route.

use std::collections::VecDeque;

use personal_rns::crypto::{
    ed25519_sign, ed25519_verify, x25519_diffie_hellman, x25519_keys_for_seal,
};
use personal_rns::engine::{
    AnnounceVerification, ChannelAckSignCompleted, ChannelAckVerification, CryptoOwed, Directive,
    EncryptCompleted, EngineReaction, EngineState, IdentifySignCompleted, InstantMillis,
    LinkIdentityVerification, LinkReceiptSignCompleted, NoOwedWork, OwedWork, ProofSignCompleted,
    ReceiptProofVerification, ResourceDecompressionCompleted, ResourceOpenCompleted,
    TunnelSynthesizeSignCompleted, TunnelSynthesizeVerification, WholeResourceOpenCompleted,
    WholeResourceOpenOutcome, WholeResourceOpenPlan,
};
use personal_rns::identity::{decrypt_token_in_place_with_ratchets, OpenedToken};
use personal_rns::interfaces::AttachedInterfaces;
use personal_rns::remote_control::RemoteControlPairingAvailabilityVerification;
use personal_rns::routing::links::handshake::{link_proof_signature_valid, link_proof_signed_data};
use personal_rns::routing::links::resources::build_outgoing::{BuildRegions, SALT_REROLL_CAP};
use personal_rns::routing::links::resources::receive::part_hash::ResourcePartHashResult;
use personal_rns::routing::links::resources::send::{
    ResourceBuildCompleted, ResourceBuildPlan, ResourceSealCompleted, ResourceSealOutcome,
    ResourceSealPlan, UnavailableResourceSeal,
};
use personal_rns::routing::links::resources::table::ResourceBuildTransfer;
use personal_rns::routing::links::resources::{
    ResourceBody, ResourceHash, ResourceMetadata, MAP_HASH_LEN, RESOURCE_NONCE_LEN,
};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::proof::ProofRequest;
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::BROADCAST_MTU;

// Boxing `CryptoOwed` would add one allocation to every browser crypto continuation. The queue is
// short-lived and move-only, so retaining the payload inline is the cheaper representation.
#[allow(clippy::large_enum_variant)]
enum InlineReadyWork {
    Crypto(CryptoOwed),
    ResourceBuild {
        plan: ResourceBuildPlan,
        data: Vec<u8>,
        compressed_candidate: Option<Vec<u8>>,
        metadata: InlineResourceMetadata,
    },
    ResourceSeal {
        plan: ResourceSealPlan,
        workspace: Vec<u8>,
    },
    ResourcePartHash {
        result: ResourcePartHashResult<Vec<u8>>,
    },
    ResourceOpen(ResourceOpenCompleted<'static>),
    WholeResourceOpen {
        plan: WholeResourceOpenPlan,
        sealed: Vec<u8>,
    },
    ResourceDecompression {
        link_id: LinkId,
        hash: ResourceHash,
        stream: Vec<u8>,
        uncompressed_data_bytes: u64,
    },
}

enum InlineResourceMetadata {
    None,
    Packed(Vec<u8>),
    SentInFirstSegment { packed_len: u32 },
}

impl InlineResourceMetadata {
    fn as_resource_metadata(&self) -> ResourceMetadata<'_> {
        match self {
            Self::None => ResourceMetadata::None,
            Self::Packed(packed) => ResourceMetadata::Packed(packed),
            Self::SentInFirstSegment { packed_len } => ResourceMetadata::SentInFirstSegment {
                packed_len: *packed_len,
            },
        }
    }
}

pub(crate) struct InlineReadyWorkQueue {
    work: VecDeque<InlineReadyWork>,
}

impl InlineReadyWorkQueue {
    pub(crate) fn new() -> Self {
        Self {
            work: VecDeque::new(),
        }
    }

    /// Materializes exactly the data that must outlive the engine's reaction borrow.
    pub(crate) fn capture(&mut self, work: OwedWork<'_>) {
        let ready = match work {
            OwedWork::Crypto(owed) => InlineReadyWork::Crypto(owed),
            OwedWork::ResourceBuild(owed) => {
                let body = owed.body();
                let metadata = match body.metadata {
                    ResourceMetadata::None => InlineResourceMetadata::None,
                    ResourceMetadata::Packed(packed) => {
                        InlineResourceMetadata::Packed(packed.to_vec())
                    }
                    ResourceMetadata::SentInFirstSegment { packed_len } => {
                        InlineResourceMetadata::SentInFirstSegment { packed_len }
                    }
                };
                let data = body.data.to_vec();
                let compressed_candidate = body.compressed_candidate.map(<[u8]>::to_vec);
                InlineReadyWork::ResourceBuild {
                    plan: owed.into_plan(),
                    data,
                    compressed_candidate,
                    metadata,
                }
            }
            OwedWork::ResourceSeal(owed) => {
                let (plan, workspace) = owed.into_owned_parts();
                InlineReadyWork::ResourceSeal { plan, workspace }
            }
            OwedWork::ResourcePartHash(owed) => {
                let (plan, source) = owed.into_parts();
                let part = source.to_vec();
                InlineReadyWork::ResourcePartHash {
                    result: plan.calculate(part),
                }
            }
            OwedWork::ResourceOpen(owed) => InlineReadyWork::ResourceOpen(owed.fulfill_inline()),
            OwedWork::WholeResourceOpen(owed) => {
                let sealed = owed.sealed().to_vec();
                InlineReadyWork::WholeResourceOpen {
                    plan: owed.into_plan(),
                    sealed,
                }
            }
            OwedWork::ResourceDecompression(owed) => InlineReadyWork::ResourceDecompression {
                link_id: owed.link_id,
                hash: owed.hash,
                stream: owed.stream.to_vec(),
                uncompressed_data_bytes: owed.uncompressed_data_bytes,
            },
        };
        self.work.push_back(ready);
    }

    pub(crate) fn push_crypto(&mut self, crypto: CryptoOwed) {
        self.work.push_back(InlineReadyWork::Crypto(crypto));
    }

    pub(crate) fn take_browser_work(
        &mut self,
        fill_random: &mut impl FnMut(&mut [u8]),
    ) -> Vec<crate::browser_work::BrowserWorkOperation> {
        let pending = self.work.len();
        let mut extracted = Vec::new();
        for _ in 0..pending {
            let Some(work) = self.work.pop_front() else {
                break;
            };
            let operation = match work {
                InlineReadyWork::Crypto(CryptoOwed::AnnounceVerify(owed)) => {
                    match crate::browser_work::BrowserWorkOperation::announce(owed) {
                        Ok(operation) => operation,
                        Err(owed) => {
                            self.work.push_back(InlineReadyWork::Crypto(
                                CryptoOwed::AnnounceVerify(*owed),
                            ));
                            continue;
                        }
                    }
                }
                InlineReadyWork::Crypto(CryptoOwed::LinkProofVerify(owed)) => {
                    crate::browser_work::BrowserWorkOperation::LinkProofVerify(owed)
                }
                InlineReadyWork::ResourceSeal { plan, workspace } => {
                    let mut seal_iv = [0u8; 16];
                    fill_random(&mut seal_iv);
                    let mut salts = [[0u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP];
                    for salt in &mut salts {
                        fill_random(salt);
                    }
                    crate::browser_work::BrowserWorkOperation::ResourceSeal {
                        plan,
                        workspace,
                        seal_iv,
                        salts,
                    }
                }
                InlineReadyWork::WholeResourceOpen { plan, sealed } => {
                    crate::browser_work::BrowserWorkOperation::WholeResourceOpen { plan, sealed }
                }
                work => {
                    self.work.push_back(work);
                    continue;
                }
            };
            extracted.push(operation);
        }
        extracted
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn fulfill_ready_work(
    ready: &mut InlineReadyWorkQueue,
    engine: &mut EngineState<GrowableHeap>,
    interfaces: AttachedInterfaces<'_>,
    now: InstantMillis,
    fill_random: &mut impl FnMut(&mut [u8]),
    should_prove: &mut impl FnMut(&ProofRequest) -> bool,
    sink: &mut impl FnMut(EngineReaction<'_, NoOwedWork>),
) {
    // Only existing work is drained: a lone packet never waits for a manufactured batch.
    while let Some(work) = ready.work.pop_front() {
        match work {
            InlineReadyWork::Crypto(crypto) => match crypto {
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
                    engine.resume_receipt_proof(owed, verification, sink);
                }
                CryptoOwed::ChannelAckVerify(owed) => {
                    let verification = if ed25519_verify(
                        &owed.signing_key,
                        owed.packet_hash.as_bytes(),
                        &owed.signature,
                    )
                    .is_ok()
                    {
                        ChannelAckVerification::Valid
                    } else {
                        ChannelAckVerification::Invalid
                    };
                    engine.resume_channel_ack_verify(owed, verification, sink);
                }
                CryptoOwed::LinkIdentityVerify(owed) => {
                    let verification =
                        if ed25519_verify(&owed.signing_key, &owed.signed_data, &owed.signature)
                            .is_ok()
                        {
                            LinkIdentityVerification::Valid
                        } else {
                            LinkIdentityVerification::Invalid
                        };
                    engine.resume_link_identity_verify(owed, verification, sink);
                }
                CryptoOwed::TunnelSynthesizeVerify(owed) => {
                    let verification =
                        if ed25519_verify(&owed.signing_key, &owed.signed_region, &owed.signature)
                            .is_ok()
                        {
                            TunnelSynthesizeVerification::Valid
                        } else {
                            TunnelSynthesizeVerification::Invalid
                        };
                    engine.resume_tunnel_synthesize_verify(owed, verification);
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
                        sink,
                    );
                }
                CryptoOwed::Decrypt(owed) => {
                    let shared =
                        x25519_diffie_hellman(&owed.encryption_secret, &owed.ephemeral_public);
                    engine.resume_decrypt(
                        owed,
                        shared,
                        interfaces,
                        should_prove,
                        &mut |reaction| route_or_capture(reaction, ready, sink),
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
                            should_prove,
                            &mut |reaction| route_or_capture(reaction, ready, sink),
                        );
                    }
                }
                CryptoOwed::LinkProofVerify(owed) => {
                    if link_proof_signature_valid(&owed) {
                        let shared = x25519_diffie_hellman(
                            &owed.initiator_secret,
                            &owed.responder_encryption,
                        );
                        engine.resume_link_proof(owed, shared, interfaces, now, fill_random, sink);
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
                        sink,
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
                        sink,
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
                        sink,
                    );
                }
                CryptoOwed::ChannelAckSign(owed) => {
                    let signature = ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
                    engine.resume_channel_ack_sign(
                        ChannelAckSignCompleted {
                            target: owed.target,
                            link_id: owed.link_id,
                            packet_hash: owed.packet_hash,
                            signature,
                        },
                        now,
                        sink,
                    );
                }
                CryptoOwed::IdentifySign(owed) => {
                    let signature = ed25519_sign(&owed.signing_secret, &owed.signed_data);
                    engine.resume_identify_sign(
                        IdentifySignCompleted { owed, signature },
                        now,
                        sink,
                    );
                }
                CryptoOwed::TunnelSynthesizeSign(owed) => {
                    let signature = ed25519_sign(&owed.signing_secret, &owed.signed_region);
                    let _ = engine.resume_tunnel_synthesize_sign(
                        TunnelSynthesizeSignCompleted { owed, signature },
                        sink,
                    );
                }
                CryptoOwed::EstablishLink(owed) => {
                    engine.resume_establish_link(owed.fulfill(), interfaces, sink);
                }
                CryptoOwed::AnnounceSign(owed) => {
                    engine.resume_announce_sign(owed.fulfill(), interfaces, sink);
                }
                CryptoOwed::AnnounceVerify(owed) => {
                    if let AnnounceVerification::Verified(verified) = owed.verify() {
                        engine.resume_announce(verified, interfaces, fill_random, sink);
                    }
                }
                CryptoOwed::RemoteControlPairingAvailabilityVerify(owed) => {
                    if let RemoteControlPairingAvailabilityVerification::Verified(verified) =
                        owed.verify()
                    {
                        engine
                            .resume_remote_control_pairing_availability(verified, interfaces, sink);
                    }
                }
            },
            InlineReadyWork::ResourceSeal { plan, workspace: _ } => {
                engine.resume_resource_seal(
                    ResourceSealCompleted {
                        reservation: plan.reservation(),
                        outcome: ResourceSealOutcome::Unavailable(
                            UnavailableResourceSeal::Resident,
                        ),
                    },
                    now,
                    fill_random,
                    &mut |reaction| route_or_capture(reaction, ready, sink),
                );
            }
            InlineReadyWork::ResourceBuild {
                plan,
                data,
                compressed_candidate,
                metadata,
            } => {
                let shape = plan.shape();
                let reservation = plan.reservation();
                let mut transfer = vec![0; shape.transfer_bytes()];
                let mut names = vec![0; shape.part_count() * MAP_HASH_LEN];
                let mut seal_iv = [0; 16];
                fill_random(&mut seal_iv);
                let outcome = plan.execute(
                    &ResourceBody {
                        data: &data,
                        compressed_candidate: compressed_candidate.as_deref(),
                        metadata: metadata.as_resource_metadata(),
                    },
                    &seal_iv,
                    || {
                        let mut nonce = [0; RESOURCE_NONCE_LEN];
                        fill_random(&mut nonce);
                        nonce
                    },
                    BuildRegions {
                        transfer: &mut transfer,
                        hashmap: &mut names,
                    },
                );
                engine.resume_resource_build(
                    ResourceBuildCompleted {
                        reservation,
                        transfer: ResourceBuildTransfer::Borrowed(&transfer),
                        names: &names,
                        request_data: &data,
                        outcome,
                    },
                    now,
                    fill_random,
                    sink,
                );
            }
            InlineReadyWork::ResourcePartHash { result } => {
                let _ = result.complete_with(|completed| {
                    engine.resume_resource_part_hash(completed, now, fill_random, &mut |reaction| {
                        route_or_capture(reaction, ready, sink)
                    })
                });
            }
            InlineReadyWork::ResourceOpen(completed) => {
                engine.resume_resource_open(completed, now, &mut |reaction| {
                    route_or_capture(reaction, ready, sink)
                });
            }
            InlineReadyWork::WholeResourceOpen { plan, sealed: _ } => {
                engine.resume_whole_resource_open(
                    WholeResourceOpenCompleted {
                        reservation: plan.reservation(),
                        outcome: WholeResourceOpenOutcome::Unavailable,
                    },
                    now,
                    &mut |reaction| route_or_capture(reaction, ready, sink),
                );
            }
            InlineReadyWork::ResourceDecompression {
                link_id,
                hash,
                stream,
                uncompressed_data_bytes,
            } => {
                let maximum = prns_runtime::resource_compression::resource_decompression_bound(
                    uncompressed_data_bytes,
                );
                let plaintext =
                    prns_runtime::resource_compression::decompress_bounded(&stream, maximum)
                        .unwrap_or_default();
                engine.resume_resource_decompression(
                    ResourceDecompressionCompleted {
                        link_id,
                        hash,
                        plaintext: &plaintext,
                    },
                    now,
                    sink,
                );
            }
        }
    }
}

fn route_or_capture(
    reaction: EngineReaction<'_, OwedWork<'_>>,
    ready: &mut InlineReadyWorkQueue,
    sink: &mut impl FnMut(EngineReaction<'_, NoOwedWork>),
) {
    match reaction {
        EngineReaction::Journaled(journaled) => sink(EngineReaction::Journaled(journaled)),
        EngineReaction::Directive(Directive::Fulfill(work)) => ready.capture(work),
        EngineReaction::Directive(Directive::Send { target, bytes }) => {
            sink(EngineReaction::Directive(Directive::Send { target, bytes }));
        }
        EngineReaction::Directive(Directive::SendIfOnline {
            target,
            bytes,
            on_send,
        }) => sink(EngineReaction::Directive(Directive::SendIfOnline {
            target,
            bytes,
            on_send,
        })),
        EngineReaction::Directive(Directive::SendAnnounce {
            target,
            bytes,
            hops,
        }) => sink(EngineReaction::Directive(Directive::SendAnnounce {
            target,
            bytes,
            hops,
        })),
        EngineReaction::Directive(Directive::SendToFleet {
            supervisor,
            fan,
            bytes,
        }) => sink(EngineReaction::Directive(Directive::SendToFleet {
            supervisor,
            fan,
            bytes,
        })),
        EngineReaction::Directive(Directive::SendAnnounceToFleet {
            supervisor,
            fan,
            bytes,
            hops,
        }) => sink(EngineReaction::Directive(Directive::SendAnnounceToFleet {
            supervisor,
            fan,
            bytes,
            hops,
        })),
        EngineReaction::Directive(Directive::EmitFrame {
            target,
            size_hint,
            fill,
        }) => sink(EngineReaction::Directive(Directive::EmitFrame {
            target,
            size_hint,
            fill,
        })),
    }
}
