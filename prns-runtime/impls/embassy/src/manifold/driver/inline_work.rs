use crate::crypto::{ed25519_sign, ed25519_verify, x25519_diffie_hellman, x25519_keys_for_seal};
use crate::engine::{
    AnnounceVerification, ChannelAckSignCompleted, ChannelAckVerification, CryptoOwed,
    EncryptCompleted, EngineReaction, EngineState, IdentifySignCompleted, InstantMillis, Journaled,
    LinkIdentityVerification, LinkReceiptSignCompleted, OwedWork, ProofRequest, ProofSignCompleted,
    ReceiptProofVerification, ResourceDecompressionCompleted, ResourceOpenCompleted,
    TunnelSynthesizeSignCompleted, TunnelSynthesizeVerification, WakeSchedules,
};
use crate::identity::decrypt_token_in_place_with_ratchets;
use crate::interfaces::{AttachedInterfaces, InterfaceId, InterfaceIfac, InterfaceStatus};
use crate::manifold::Host;
use crate::remote_control::RemoteControlPairingAvailabilityVerification;
use crate::routing::links::handshake::{link_proof_signature_valid, link_proof_signed_data};
use crate::routing::links::resources::table::ResourceBuildReservation;
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;
use heapless::Deque;

use super::egress::{route_reaction, route_reaction_with_work, InterfacePacer, ManifoldEgress};
use super::interface_status::EmbassyInterfaceStatus;

const MAX_OWED_WORK_PER_TICK: usize = 2;

// Embassy has no allocator to box continuation payloads, and the two-entry queue is short-lived.
// Keeping work by value also preserves move-only secrets and zero-copy resource-open state.
#[allow(clippy::large_enum_variant)]
pub(super) enum InlineReadyWork {
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

pub(super) type InlineOwedWorkQueue = Deque<InlineReadyWork, MAX_OWED_WORK_PER_TICK>;

/// Routes one engine reaction and captures the externally fulfilled work, if any.
///
/// A single engine transition may journal and send many things, but it may suspend at only one
/// continuation boundary. Keeping that invariant here prevents immediate Embassy fulfillment
/// from recursively re-entering the engine through its own reaction sink.
pub(super) fn route_and_capture_owed_work(
    reaction: EngineReaction<'_, OwedWork<'_>>,
    egress: &mut impl ManifoldEgress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    now: InstantMillis,
    app: &mut impl FnMut(Journaled<'_>),
    pending: &mut InlineOwedWorkQueue,
) {
    route_reaction_with_work(reaction, egress, ifacs, pacers, now, app, &mut |work| {
        #[allow(unreachable_patterns)]
        let ready = match work {
            OwedWork::Crypto(crypto) => InlineReadyWork::Crypto(crypto),
            OwedWork::ResourceBuild(owed) => InlineReadyWork::ResourceBuildUnsupported {
                reservation: owed.reservation(),
            },
            OwedWork::ResourceOpen(owed) => InlineReadyWork::ResourceOpen(owed.fulfill_inline()),
            OwedWork::ResourceDecompression(owed) => {
                InlineReadyWork::ResourceDecompressionUnsupported {
                    link_id: owed.link_id,
                    hash: owed.hash,
                }
            }
            _ => return,
        };
        assert!(
            pending.push_back(ready).is_ok(),
            "one manifold tick exceeded the two owed-work continuation bound"
        );
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn fulfill_owed_work_inline<S, H, E, P, J>(
    mut pending: InlineOwedWorkQueue,
    engine: &mut EngineState<S>,
    host: &mut H,
    interfaces: AttachedInterfaces<'_>,
    egress: &mut E,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    frame_accounting_statuses: &[&EmbassyInterfaceStatus],
    now: InstantMillis,
    should_prove: &mut P,
    on_journaled: &mut J,
) -> WakeSchedules
where
    S: StorageLayout,
    H: Host,
    E: ManifoldEgress,
    P: FnMut(&ProofRequest) -> bool,
    J: FnMut(Journaled<'_>),
{
    let mut wake = WakeSchedules::UNCHANGED;
    // Work emitted while resuming is already ready. Keep advancing without manufacturing a
    // batch or returning to the executor between continuation steps.
    while let Some(work) = pending.pop_front() {
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
                    wake.compose(engine.resume_receipt_proof(
                        owed,
                        verification,
                        &mut |reaction| {
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
                    ));
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
                    wake.compose(engine.resume_channel_ack_verify(
                        owed,
                        verification,
                        &mut |reaction| {
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
                    ));
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
                    engine.resume_link_identity_verify(owed, verification, &mut |reaction| {
                        route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                    });
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
                    wake.compose(engine.resume_tunnel_synthesize_verify(owed, verification));
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
                        &mut |reaction| {
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
                    ));
                }
                CryptoOwed::Decrypt(owed) => {
                    let shared =
                        x25519_diffie_hellman(&owed.encryption_secret, &owed.ephemeral_public);
                    engine.resume_decrypt(
                        owed,
                        shared,
                        interfaces,
                        should_prove,
                        &mut |reaction| {
                            route_and_capture_owed_work(
                                reaction,
                                egress,
                                ifacs,
                                pacers,
                                now,
                                on_journaled,
                                &mut pending,
                            );
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
                        let mut plaintext = heapless::Vec::<
                            u8,
                            { crate::routing::ingress::MAX_RATCHET_DECRYPT_PAYLOAD_LEN },
                        >::new();
                        if plaintext.extend_from_slice(opened.plaintext).is_err() {
                            continue;
                        }
                        let opened_by = opened.opened_by;
                        engine.resume_ratchet_decrypt(
                            owed,
                            crate::identity::OpenedToken {
                                opened_by,
                                plaintext: &plaintext,
                            },
                            interfaces,
                            should_prove,
                            &mut |reaction| {
                                route_and_capture_owed_work(
                                    reaction,
                                    egress,
                                    ifacs,
                                    pacers,
                                    now,
                                    on_journaled,
                                    &mut pending,
                                );
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
                        wake.compose(engine.resume_link_proof(
                            owed,
                            shared,
                            interfaces,
                            now,
                            &mut |entropy| host.fill_random(entropy),
                            &mut |reaction| {
                                route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                            },
                        ));
                    } else {
                        record_protocol_violation(frame_accounting_statuses, owed.source_interface);
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
                        &mut |reaction| {
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
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
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
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
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
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
                        &mut |reaction| {
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
                    );
                }
                CryptoOwed::IdentifySign(owed) => {
                    let signature = ed25519_sign(&owed.signing_secret, &owed.signed_data);
                    wake.compose(engine.resume_identify_sign(
                        IdentifySignCompleted { owed, signature },
                        now,
                        &mut |reaction| {
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
                    ));
                }
                CryptoOwed::TunnelSynthesizeSign(owed) => {
                    let signature = ed25519_sign(&owed.signing_secret, &owed.signed_region);
                    let _ = engine.resume_tunnel_synthesize_sign(
                        TunnelSynthesizeSignCompleted { owed, signature },
                        &mut |reaction| {
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
                    );
                }
                CryptoOwed::EstablishLink(owed) => {
                    wake.compose(engine.resume_establish_link(
                        owed.fulfill(),
                        interfaces,
                        &mut |reaction| {
                            route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                        },
                    ));
                }
                CryptoOwed::AnnounceSign(owed) => {
                    engine.resume_announce_sign(owed.fulfill(), interfaces, &mut |reaction| {
                        route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                    });
                }
                CryptoOwed::AnnounceVerify(owed) => match owed.verify() {
                    AnnounceVerification::Verified(verified) => {
                        wake.compose(engine.resume_announce(
                            verified,
                            interfaces,
                            &mut |entropy| host.fill_random(entropy),
                            &mut |reaction| {
                                route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                            },
                        ));
                    }
                    AnnounceVerification::Invalid(invalid) => {
                        record_protocol_violation(
                            frame_accounting_statuses,
                            invalid.source_interface(),
                        );
                    }
                },
                CryptoOwed::RemoteControlPairingAvailabilityVerify(owed) => match owed.verify() {
                    RemoteControlPairingAvailabilityVerification::Verified(verified) => {
                        wake.compose(engine.resume_remote_control_pairing_availability(
                            verified,
                            interfaces,
                            &mut |reaction| {
                                route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                            },
                        ));
                    }
                    RemoteControlPairingAvailabilityVerification::Invalid(invalid) => {
                        record_protocol_violation(
                            frame_accounting_statuses,
                            invalid.source_interface(),
                        );
                    }
                },
            },
            InlineReadyWork::ResourceBuildUnsupported { reservation } => {
                wake.compose(engine.resume_resource_build_unavailable(
                    reservation,
                    &mut |reaction| {
                        route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                    },
                ));
            }
            InlineReadyWork::ResourceOpen(completed) => {
                wake.compose(
                    engine.resume_resource_open(completed, now, &mut |reaction| {
                        route_and_capture_owed_work(
                            reaction,
                            egress,
                            ifacs,
                            pacers,
                            now,
                            on_journaled,
                            &mut pending,
                        );
                    }),
                );
            }
            InlineReadyWork::ResourceDecompressionUnsupported { link_id, hash } => {
                // The no-alloc Embassy runtime does not carry a bzip2 implementation. An empty
                // completion is the engine's typed build-failure signal; boards that add a
                // decompressor later can fulfill the same directive without changing core.
                wake.compose(engine.resume_resource_decompression(
                    ResourceDecompressionCompleted {
                        link_id,
                        hash,
                        plaintext: &[],
                    },
                    now,
                    &mut |reaction| {
                        route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                    },
                ));
            }
        }
    }

    wake
}

fn record_protocol_violation(statuses: &[&EmbassyInterfaceStatus], source: InterfaceId) {
    if let Some(status) = statuses.iter().find(|status| status.id() == source) {
        status.count_protocol_violation();
    }
}
