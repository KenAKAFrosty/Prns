use crate::crypto::{ed25519_sign, ed25519_verify, x25519_diffie_hellman, x25519_keys_for_seal};
use crate::engine::{
    CryptoOwed, Directive, EngineReaction, EngineState, InstantMillis, Journaled, OwedWork,
    ProofRequest, ReceiptProofVerification, ResourceDecompressionCompleted, WakeSchedules,
};
use crate::identity::decrypt_token_in_place_with_ratchets;
use crate::interfaces::{AttachedInterfaces, InterfaceId, InterfaceIfac, InterfaceStatus};
use crate::manifold::Host;
use crate::remote_control::RemoteControlPairingAvailabilityVerification;
use crate::routing::announce::Announce;
use crate::routing::links::handshake::{link_proof_signature_valid, link_proof_signed_data};
use crate::routing::links::resources::build_outgoing::BuildOutgoingResourceError;
use crate::routing::links::resources::send::ResourceBuildCompleted;
use crate::routing::links::resources::table::ResourceBuildReservation;
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;
use crate::routing::proof::{EXPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN};
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;
use heapless::Deque;

use super::egress::{route_reaction, route_reaction_with_work, InterfacePacer, ManifoldEgress};
use super::interface_status::EmbassyInterfaceStatus;

const MAX_OWED_WORK_PER_TICK: usize = 2;

pub(super) enum InlineReadyWork {
    Crypto(CryptoOwed),
    ResourceBuildUnsupported {
        reservation: ResourceBuildReservation,
    },
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
        let ready = match work {
            OwedWork::Crypto(crypto) => InlineReadyWork::Crypto(crypto),
            OwedWork::ResourceBuild(owed) => InlineReadyWork::ResourceBuildUnsupported {
                reservation: owed.reservation(),
            },
            OwedWork::ResourceDecompression(owed) => {
                InlineReadyWork::ResourceDecompressionUnsupported {
                    link_id: owed.link_id,
                    hash: owed.hash,
                }
            }
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
                CryptoOwed::Encrypt(owed) => {
                    let (ephemeral_public, shared) =
                        x25519_keys_for_seal(&owed.ephemeral_secret, &owed.dh_target);
                    let mut wire = [0u8; BROADCAST_MTU];
                    wake.compose(engine.complete_send_single_packet_deferred(
                        owed,
                        ephemeral_public,
                        shared,
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
                    let mut proof = [0u8; EXPLICIT_PROOF_WIRE_LEN];
                    if let Ok(written) =
                        engine.write_signed_proof(&owed.packet_hash, &signature, &mut proof)
                    {
                        route_reaction(
                            EngineReaction::Directive(Directive::Send {
                                target: owed.target,
                                bytes: &proof[..written],
                            }),
                            egress,
                            ifacs,
                            pacers,
                            now,
                            on_journaled,
                        );
                    }
                }
                CryptoOwed::LinkReceiptSign(owed) => {
                    let signature = ed25519_sign(&owed.signing_secret, owed.packet_hash.as_bytes());
                    let mut proof = [0u8; LINK_PROOF_WIRE_LEN];
                    if let Ok(written) = engine.complete_link_receipt_sign(
                        &owed.link_id,
                        &owed.packet_hash,
                        &signature,
                        now,
                        &mut proof,
                    ) {
                        route_reaction(
                            EngineReaction::Directive(Directive::Send {
                                target: owed.target,
                                bytes: &proof[..written],
                            }),
                            egress,
                            ifacs,
                            pacers,
                            now,
                            on_journaled,
                        );
                    }
                }
                CryptoOwed::AnnounceVerify(owed) => {
                    let valid = Announce::from_wire_unverified(&owed.header, &owed.payload)
                        .is_ok_and(|announce| announce.signature_is_valid());
                    if valid {
                        wake.compose(engine.resume_announce(
                            owed,
                            interfaces,
                            &mut |entropy| host.fill_random(entropy),
                            &mut |reaction| {
                                route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                            },
                        ));
                    } else {
                        record_protocol_violation(frame_accounting_statuses, owed.source_interface);
                    }
                }
                CryptoOwed::RemoteControlPairingAvailabilityVerify(owed) => match owed.verify() {
                    RemoteControlPairingAvailabilityVerification::Valid => {
                        wake.compose(engine.resume_remote_control_pairing_availability(
                            owed,
                            interfaces,
                            &mut |reaction| {
                                route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                            },
                        ));
                    }
                    RemoteControlPairingAvailabilityVerification::Invalid => {
                        record_protocol_violation(
                            frame_accounting_statuses,
                            owed.source_interface(),
                        );
                    }
                },
            },
            InlineReadyWork::ResourceBuildUnsupported { reservation } => {
                wake.compose(engine.resume_resource_build(
                    ResourceBuildCompleted {
                        reservation,
                        transfer: &[],
                        names: &[],
                        request_data: &[],
                        outcome: Err(BuildOutgoingResourceError::BufferShapeMismatch),
                    },
                    now,
                    &mut |entropy| host.fill_random(entropy),
                    &mut |reaction| {
                        route_reaction(reaction, egress, ifacs, pacers, now, on_journaled);
                    },
                ));
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
