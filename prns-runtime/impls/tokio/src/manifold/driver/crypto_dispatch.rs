use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, Journaled, OwedWork, ProofRequest,
    ResourceDecompressionCompleted, WakeSchedules,
};
use crate::identity::OpenedToken;
use crate::interfaces::{FrameAccountingEvent, InterfaceIfac};
use crate::manifold::Host;
use crate::routing::links::resources::build_outgoing::SALT_REROLL_CAP;
use crate::routing::links::resources::receive::offload::OffloadedOpenSpan;
use crate::routing::links::resources::send::{OffloadedStagedSeal, ResourceBuildCompleted};
use crate::routing::links::resources::{MAP_HASH_LEN, RESOURCE_NONCE_LEN};
use crate::routing::proof::{EXPLICIT_PROOF_WIRE_LEN, LINK_PROOF_WIRE_LEN};
use crate::storage::StorageLayout;

use super::crypto_pool::{
    CryptoCompletion, CryptoJob, CryptoPool, CryptoResult, OpenSpanJob, StagedSealJob,
};
use super::egress::{
    route_reaction, route_reaction_with_work, Egress, InterfacePacer, WireScratch,
};
use super::interface_topology::InterfaceTopology;
use super::journal_delivery::JournalDispatch;
use super::owed_work::PendingOwedWork;
use crate::remote_control::RemoteControlPairingAvailabilityVerification;

fn route_completion_reaction<J>(
    reaction: EngineReaction<'_, OwedWork<'_>>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    wire_scratch: &mut WireScratch,
    journal: &mut JournalDispatch<J>,
    owed_work: &mut PendingOwedWork,
    now: InstantMillis,
) where
    J: for<'a> FnMut(Journaled<'a>),
{
    route_reaction_with_work(
        reaction,
        egress,
        ifacs,
        pacers,
        wire_scratch,
        now,
        &mut |journaled| journal.route(journaled),
        &mut |work| owed_work.push(work),
    );
}

fn route_completed_reaction_without_work<J>(
    reaction: EngineReaction<'_>,
    egress: &mut Egress,
    ifacs: &[InterfaceIfac],
    pacers: &mut [InterfacePacer],
    wire_scratch: &mut WireScratch,
    journal: &mut JournalDispatch<J>,
    now: InstantMillis,
) where
    J: for<'a> FnMut(Journaled<'a>),
{
    route_reaction(
        reaction,
        egress,
        ifacs,
        pacers,
        wire_scratch,
        now,
        &mut |journaled| journal.route(journaled),
    );
}

pub(super) enum CryptoCompletionEffect {
    NoWakeChange,
    WakeSchedules(WakeSchedules),
    OpenSpanAdvanced(WakeSchedules),
}

pub(super) struct CryptoDispatch<'a, S, H, J>
where
    S: StorageLayout,
    H: Host,
    J: for<'b> FnMut(Journaled<'b>),
{
    pub(super) engine: &'a mut EngineState<S>,
    pub(super) host: &'a mut H,
    pub(super) topology: &'a mut InterfaceTopology,
    pub(super) wire_scratch: &'a mut WireScratch,
    pub(super) journal: &'a mut JournalDispatch<J>,
    pub(super) crypto_pool: Option<&'a CryptoPool>,
    pub(super) owed_work: &'a mut PendingOwedWork,
}

impl<S, H, J> CryptoDispatch<'_, S, H, J>
where
    S: StorageLayout,
    H: Host,
    J: for<'a> FnMut(Journaled<'a>),
{
    pub(super) fn dispatch_staged_seal(self, now: InstantMillis) {
        let Self {
            engine,
            host,
            topology,
            wire_scratch,
            journal,
            crypto_pool,
            owed_work: _,
        } = self;
        let Some(link_id) = engine.owed_staged_seal_link() else {
            return;
        };
        match crypto_pool {
            Some(pool) => {
                let Some(view) = engine.staged_seal_job_view(&link_id) else {
                    return;
                };
                let mut seal_iv = [0u8; 16];
                host.fill_random(&mut seal_iv);
                let mut salts = [[0u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP];
                for salt in &mut salts {
                    host.fill_random(salt);
                }
                let job = StagedSealJob {
                    link_id,
                    key: view.key.cloned(),
                    sdu: view.sdu,
                    nonce_prefixed_bytes: view.nonce_prefixed_bytes,
                    plaintext: view.plaintext.to_vec(),
                    seal_iv,
                    salts,
                };
                engine.mark_staged_sealing(&link_id);
                pool.submit(CryptoJob::SealStaged(Box::new(job)));
            }
            None => {
                engine.seal_staged_continuation(
                    &link_id,
                    &mut |entropy| host.fill_random(entropy),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            now,
                            &mut |journaled| journal.route(journaled),
                        )
                    },
                );
            }
        }
    }

    pub(super) fn complete<P>(
        self,
        completion: CryptoCompletion,
        now: InstantMillis,
        seal_buf: &mut [u8; crate::wire::BROADCAST_MTU],
        should_prove: &mut P,
    ) -> CryptoCompletionEffect
    where
        P: FnMut(&ProofRequest) -> bool,
    {
        let Self {
            engine,
            host,
            topology,
            wire_scratch,
            journal,
            crypto_pool,
            owed_work,
        } = self;
        let CryptoCompletion {
            worker,
            result,
            work,
        } = completion;
        if let Some(pool) = crypto_pool {
            pool.record_completed(worker, work);
            if result.settles_packet_verdict() {
                pool.packet_verdict_settled();
            }
        }
        match result {
            CryptoResult::ReceiptProofVerified { owed, verification } => {
                CryptoCompletionEffect::WakeSchedules(engine.resume_receipt_proof(
                    owed,
                    verification,
                    &mut |reaction| {
                        route_completed_reaction_without_work(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            now,
                        )
                    },
                ))
            }
            CryptoResult::Sealed {
                owed,
                ephemeral_public,
                shared,
            } => {
                CryptoCompletionEffect::WakeSchedules(engine.complete_send_single_packet_deferred(
                    owed,
                    ephemeral_public,
                    shared,
                    topology.interfaces.view(),
                    seal_buf,
                    &mut |reaction| {
                        route_completed_reaction_without_work(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            now,
                        )
                    },
                ))
            }
            CryptoResult::Signed {
                target,
                packet_hash,
                signature,
            } => {
                let mut proof = [0u8; EXPLICIT_PROOF_WIRE_LEN];
                if let Ok(written) = engine.write_signed_proof(&packet_hash, &signature, &mut proof)
                {
                    route_reaction(
                        EngineReaction::Directive(Directive::Send {
                            target,
                            bytes: &proof[..written],
                        }),
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        now,
                        &mut |journaled| journal.route(journaled),
                    );
                }
                CryptoCompletionEffect::NoWakeChange
            }
            CryptoResult::LinkReceiptSigned {
                target,
                link_id,
                packet_hash,
                signature,
            } => {
                let mut proof = [0u8; LINK_PROOF_WIRE_LEN];
                if let Ok(written) = engine.complete_link_receipt_sign(
                    &link_id,
                    &packet_hash,
                    &signature,
                    now,
                    &mut proof,
                ) {
                    route_reaction(
                        EngineReaction::Directive(Directive::Send {
                            target,
                            bytes: &proof[..written],
                        }),
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        now,
                        &mut |journaled| journal.route(journaled),
                    );
                }
                CryptoCompletionEffect::NoWakeChange
            }
            CryptoResult::Decrypted { owed, shared } => {
                engine.resume_decrypt(
                    owed,
                    shared,
                    topology.interfaces.view(),
                    should_prove,
                    &mut |reaction| {
                        route_completion_reaction(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            owed_work,
                            now,
                        )
                    },
                );
                CryptoCompletionEffect::NoWakeChange
            }
            CryptoResult::RatchetDecrypted { owed, opened } => {
                if let Some((opened_by, plaintext)) = opened {
                    engine.resume_ratchet_decrypt(
                        *owed,
                        OpenedToken {
                            opened_by,
                            plaintext: &plaintext,
                        },
                        topology.interfaces.view(),
                        should_prove,
                        &mut |reaction| {
                            route_completion_reaction(
                                reaction,
                                &mut topology.egress,
                                &topology.ifacs,
                                &mut topology.pacers,
                                wire_scratch,
                                journal,
                                owed_work,
                                now,
                            )
                        },
                    );
                }
                CryptoCompletionEffect::NoWakeChange
            }
            CryptoResult::LinkProofVerified { owed, shared } => match shared {
                Some(shared) => CryptoCompletionEffect::WakeSchedules(engine.resume_link_proof(
                    owed,
                    shared,
                    topology.interfaces.view(),
                    now,
                    &mut |entropy| host.fill_random(entropy),
                    &mut |reaction| {
                        route_completed_reaction_without_work(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            now,
                        )
                    },
                )),
                None => {
                    if let Some(recorder) =
                        topology.frame_accounting_recorder(owed.source_interface)
                    {
                        recorder.record(FrameAccountingEvent::ProtocolViolation);
                    }
                    CryptoCompletionEffect::NoWakeChange
                }
            },
            CryptoResult::LinkProofSigned {
                owed,
                responder_encryption,
                shared,
                signature,
            } => CryptoCompletionEffect::WakeSchedules(engine.resume_link_proof_sign(
                owed,
                responder_encryption,
                shared,
                signature,
                topology.interfaces.view(),
                &mut |reaction| {
                    route_completed_reaction_without_work(
                        reaction,
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        journal,
                        now,
                    )
                },
            )),
            CryptoResult::ResourceBuilt {
                reservation,
                request_data,
                transfer,
                names,
                outcome,
            } => CryptoCompletionEffect::WakeSchedules(engine.resume_resource_build(
                ResourceBuildCompleted {
                    reservation,
                    transfer: &transfer,
                    names: &names,
                    request_data: request_data.as_slice(),
                    outcome,
                },
                now,
                &mut |entropy| host.fill_random(entropy),
                &mut |reaction| {
                    route_completed_reaction_without_work(
                        reaction,
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        journal,
                        now,
                    )
                },
            )),
            CryptoResult::ResourceDecompressed {
                link_id,
                hash,
                plaintext,
            } => CryptoCompletionEffect::WakeSchedules(engine.resume_resource_decompression(
                ResourceDecompressionCompleted {
                    link_id,
                    hash,
                    plaintext: &plaintext,
                },
                now,
                &mut |reaction| {
                    route_completion_reaction(
                        reaction,
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        journal,
                        owed_work,
                        now,
                    )
                },
            )),
            CryptoResult::StagedSealed {
                link_id,
                stream_nonce,
                nonce_prefixed_bytes,
                transfer,
                names,
                outcome,
            } => {
                let sealed_len = outcome.map_or(0, |sealed| sealed.sealed_transfer_bytes);
                let names_len = outcome.map_or(0, |sealed| sealed.part_count * MAP_HASH_LEN);
                engine.apply_offloaded_staged_seal(
                    OffloadedStagedSeal {
                        link_id,
                        stream_nonce,
                        nonce_prefixed_bytes,
                        sealed_bytes: &transfer[..sealed_len],
                        names: &names[..names_len],
                        outcome,
                    },
                    &mut |reaction| {
                        route_completion_reaction(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            owed_work,
                            now,
                        )
                    },
                );
                engine.promote_staged_resource(
                    &link_id,
                    now,
                    &mut |entropy| host.fill_random(entropy),
                    &mut |reaction| {
                        route_completion_reaction(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            journal,
                            owed_work,
                            now,
                        )
                    },
                );
                CryptoCompletionEffect::WakeSchedules(WakeSchedules {
                    resource_deadlines: engine.resource_deadlines_wake(),
                    ..WakeSchedules::UNCHANGED
                })
            }
            CryptoResult::AnnounceVerified { owed, valid } => {
                if valid {
                    CryptoCompletionEffect::WakeSchedules(engine.resume_announce(
                        owed,
                        topology.interfaces.view(),
                        &mut |entropy| host.fill_random(entropy),
                        &mut |reaction| {
                            route_completed_reaction_without_work(
                                reaction,
                                &mut topology.egress,
                                &topology.ifacs,
                                &mut topology.pacers,
                                wire_scratch,
                                journal,
                                now,
                            )
                        },
                    ))
                } else {
                    if let Some(recorder) =
                        topology.frame_accounting_recorder(owed.source_interface)
                    {
                        recorder.record(FrameAccountingEvent::ProtocolViolation);
                    }
                    CryptoCompletionEffect::NoWakeChange
                }
            }
            CryptoResult::RemoteControlPairingAvailabilityVerified { owed, verification } => {
                match verification {
                    RemoteControlPairingAvailabilityVerification::Valid => {
                        CryptoCompletionEffect::WakeSchedules(
                            engine.resume_remote_control_pairing_availability(
                                owed,
                                topology.interfaces.view(),
                                &mut |reaction| {
                                    route_completed_reaction_without_work(
                                        reaction,
                                        &mut topology.egress,
                                        &topology.ifacs,
                                        &mut topology.pacers,
                                        wire_scratch,
                                        journal,
                                        now,
                                    )
                                },
                            ),
                        )
                    }
                    RemoteControlPairingAvailabilityVerification::Invalid => {
                        if let Some(recorder) =
                            topology.frame_accounting_recorder(owed.source_interface())
                        {
                            recorder.record(FrameAccountingEvent::ProtocolViolation);
                        }
                        CryptoCompletionEffect::NoWakeChange
                    }
                }
            }
            CryptoResult::SpanOpened {
                link_id,
                hash,
                span_start,
                state,
                bytes,
            } => CryptoCompletionEffect::OpenSpanAdvanced(engine.apply_opened_span(
                OffloadedOpenSpan {
                    link_id,
                    hash,
                    span_start,
                    state,
                    bytes: &bytes,
                },
                now,
                &mut |reaction| {
                    route_completion_reaction(
                        reaction,
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        journal,
                        owed_work,
                        now,
                    )
                },
            )),
        }
    }
}

pub(super) fn dispatch_open_spans<S: StorageLayout>(
    engine: &mut EngineState<S>,
    crypto_pool: Option<&CryptoPool>,
) {
    let Some(pool) = crypto_pool else {
        return;
    };
    while let Some((link_id, hash)) = engine.owed_open_span() {
        if !pool.has_queue_capacity(1) {
            break;
        }
        let Some(view) = engine.open_span_job_view(&link_id, &hash) else {
            break;
        };
        let span_start = view.span_start;
        let bytes = view.bytes.to_vec();
        let Some(state) = engine.begin_open_chew(&link_id, &hash) else {
            break;
        };
        pool.submit(CryptoJob::OpenSpan(Box::new(OpenSpanJob {
            link_id,
            hash,
            span_start,
            state,
            bytes,
        })));
    }
}
