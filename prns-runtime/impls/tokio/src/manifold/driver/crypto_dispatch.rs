use crate::engine::{
    Directive, EngineReaction, EngineState, InstantMillis, Journaled, ProofRequest,
    ResolvedReceiptSettlement, WakeSchedules,
};
use crate::identity::OpenedToken;
use crate::interfaces::FrameAccountingEvent;
use crate::manifold::Host;
use crate::routing::links::resources::build_outgoing::SALT_REROLL_CAP;
use crate::routing::links::resources::receive::offload::OffloadedOpenSpan;
use crate::routing::links::resources::send::OffloadedStagedSeal;
use crate::routing::links::resources::{MAP_HASH_LEN, RESOURCE_NONCE_LEN};
use crate::routing::proof::EXPLICIT_PROOF_WIRE_LEN;
use crate::storage::StorageLayout;

use super::crypto_pool::{CryptoJob, CryptoPool, CryptoResult, OpenSpanJob, StagedSealJob};
use super::egress::{route_reaction, WireScratch};
use super::interface_topology::InterfaceTopology;
use super::journal_delivery::JournalDispatch;

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
}

impl<S, H, J> CryptoDispatch<'_, S, H, J>
where
    S: StorageLayout,
    H: Host,
    J: for<'a> FnMut(Journaled<'a>),
{
    pub(super) fn dispatch_staged_seal(self) {
        let Self {
            engine,
            host,
            topology,
            wire_scratch,
            journal,
            crypto_pool,
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
                host.fill_entropy(&mut seal_iv);
                let mut salts = [[0u8; RESOURCE_NONCE_LEN]; SALT_REROLL_CAP];
                for salt in &mut salts {
                    host.fill_entropy(salt);
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
                let now = host.now();
                engine.seal_staged_continuation(
                    &link_id,
                    &mut |entropy| host.fill_entropy(entropy),
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
        result: CryptoResult,
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
        } = self;
        if let Some(pool) = crypto_pool {
            #[cfg(feature = "runtime-metrics")]
            pool.record_completed();
            if result.settles_packet_verdict() {
                pool.packet_verdict_settled();
            }
        }
        macro_rules! journaled_sink {
            () => {
                |journaled| journal.route(journaled)
            };
        }
        macro_rules! reaction_sink {
            () => {
                |reaction| {
                    route_reaction(
                        reaction,
                        &mut topology.egress,
                        &topology.ifacs,
                        &mut topology.pacers,
                        wire_scratch,
                        now,
                        &mut journaled_sink!(),
                    )
                }
            };
        }

        match result {
            CryptoResult::Verified {
                id,
                packet_hash,
                settlement,
                arrived_at,
                valid,
            } => {
                if !valid {
                    return CryptoCompletionEffect::NoWakeChange;
                }
                match engine.settle_resolved_receipt_proof(id, &packet_hash, arrived_at) {
                    ResolvedReceiptSettlement::Settled => {
                        route_reaction(
                            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }),
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            now,
                            &mut journaled_sink!(),
                        );
                        settled_receipt_effect(engine)
                    }
                    ResolvedReceiptSettlement::NoMatchingReceipt => {
                        CryptoCompletionEffect::NoWakeChange
                    }
                }
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
                    &mut reaction_sink!(),
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
                        &mut journaled_sink!(),
                    );
                }
                CryptoCompletionEffect::NoWakeChange
            }
            CryptoResult::Decrypted { owed, shared } => {
                let mut deferred_sign = None;
                engine.resume_decrypt(
                    owed,
                    shared,
                    topology.interfaces.view(),
                    should_prove,
                    &mut deferred_sign,
                    &mut reaction_sink!(),
                );
                if let (Some(deferred), Some(pool)) = (deferred_sign, crypto_pool) {
                    pool.submit(CryptoJob::Sign(deferred));
                }
                CryptoCompletionEffect::NoWakeChange
            }
            CryptoResult::RatchetDecrypted { owed, opened } => {
                if let Some((opened_by, plaintext)) = opened {
                    let mut deferred_sign = None;
                    engine.resume_ratchet_decrypt(
                        *owed,
                        OpenedToken {
                            opened_by,
                            plaintext: &plaintext,
                        },
                        topology.interfaces.view(),
                        should_prove,
                        &mut deferred_sign,
                        &mut reaction_sink!(),
                    );
                    if let (Some(deferred), Some(pool)) = (deferred_sign, crypto_pool) {
                        pool.submit(CryptoJob::Sign(deferred));
                    }
                }
                CryptoCompletionEffect::NoWakeChange
            }
            CryptoResult::LinkProofVerified { owed, shared } => match shared {
                Some(shared) => CryptoCompletionEffect::WakeSchedules(engine.resume_link_proof(
                    owed,
                    shared,
                    topology.interfaces.view(),
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut reaction_sink!(),
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
                &mut reaction_sink!(),
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
                    &mut reaction_sink!(),
                );
                engine.promote_staged_resource(
                    &link_id,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut reaction_sink!(),
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
                        &mut |entropy| host.fill_entropy(entropy),
                        &mut reaction_sink!(),
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
                &mut reaction_sink!(),
            )),
        }
    }
}

fn settled_receipt_effect<S: StorageLayout>(engine: &EngineState<S>) -> CryptoCompletionEffect {
    CryptoCompletionEffect::WakeSchedules(WakeSchedules {
        receipt_timeouts: engine.receipt_timeouts_wake(),
        ..WakeSchedules::UNCHANGED
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::WakeSchedule;
    use crate::storage::GrowableHeap;

    #[test]
    fn verified_receipt_settlement_recomputes_its_timeout_wake() {
        let engine = EngineState::<GrowableHeap>::default();
        let CryptoCompletionEffect::WakeSchedules(delta) = settled_receipt_effect(&engine) else {
            panic!("verified receipt settlement must publish a wake delta");
        };
        assert_eq!(delta.receipt_timeouts, WakeSchedule::Idle);
        assert_eq!(delta.scheduled_announces, WakeSchedule::Unchanged);
    }
}
