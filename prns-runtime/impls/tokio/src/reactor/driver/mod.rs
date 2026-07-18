use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use crate::engine::{
    ClassifiedInboundPacket, DeferredCrypto, Directive, EngineReaction, EngineState, IngestIo,
    InstantMillis, Journaled, NextWake, ProofIngest, ProofRequest, Settlement, WakeReason,
    WakeSchedules,
};
use crate::identity::OpenedToken;
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{InboundPacket, InterfaceDescriptor, InterfaceId, PacketPhyStats};
use crate::reactor::kernel::{fire_due_reason, merge_wake_schedules_delta};
use crate::reactor::AppDeciders;
use crate::reactor::Host;
use crate::routing::dedup::PacketHash;
use crate::routing::links::resources::build_outgoing::SALT_REROLL_CAP;
use crate::routing::links::resources::receive::offload::OffloadedOpenSpan;
use crate::routing::links::resources::send::OffloadedStagedSeal;
use crate::routing::links::resources::streamed_open::ResourceOpenLane;
use crate::routing::links::resources::ResourceOffer;
use crate::routing::links::resources::{MAP_HASH_LEN, RESOURCE_NONCE_LEN};
use crate::routing::proof::EXPLICIT_PROOF_WIRE_LEN;
use crate::runtime::InterfaceStore;
use crate::storage::{DirtyInterfaceSet, StorageLayout};
use crate::wire::DestinationHash;

mod command_dispatch;
mod crypto_pool;
mod egress;
mod host;
mod host_protocol;
mod interface_seam;
mod interface_status;
mod interface_topology;
mod journal_delivery;
mod persistence_snapshots;

pub use super::grant_lane::{
    tokio_grant_lane, HeapFrameSlot, TokioGrantConsumer, TokioGrantProducer,
};
pub use crypto_pool::{CryptoPoolConfig, PoolWorkers};
pub use egress::Egress;
pub use host::TokioHost;
pub use host_protocol::{
    AddInterfaceCommand, HostCommand, HostResourceMetadata, HostResourcePayload,
    HostResourcePayloadError, PersistedStateSnapshot, ProvideDecompressedHostCommand,
    RequestAnyHostCommand, ResourceInbound, RespondAnyHostCommand, SelfRatchetSnapshot,
    SelfRatchetsSnapshot, SendResourceHostCommand, SendResourceSegmentHostCommand, StreamInbound,
};
pub use interface_seam::TokioInterfaceSeam;
pub use interface_status::TokioInterfaceStatus;

use command_dispatch::{CommandDispatch, CommandEffect};
use crypto_pool::{
    CryptoJob, CryptoPool, CryptoResult, EngineVerifyJob, OpenSpanJob, StagedSealJob,
};
#[cfg(all(test, feature = "runtime-metrics"))]
use egress::enqueue_announce_for_wire;
use egress::{flush_due_pacers, ifac_for, route_reaction, soonest_pacer_release, WireScratch};
#[cfg(test)]
use egress::{offer_to_pacer, InterfacePacer, PacedAnnounce, TokioAnnouncePacer};
use host::bounded_timer_deadline;
use interface_topology::InterfaceTopology;
use journal_delivery::JournalDispatch;

fn retain_packet_phy(
    store: Option<&InterfaceStore>,
    packet_hash: PacketHash,
    packet_phy: PacketPhyStats,
) {
    if packet_phy.is_empty() {
        return;
    }
    let Some(store) = store else {
        return;
    };
    store.remember_packet_phy(packet_hash, packet_phy);
}

/// Everything the reactor is wired to for one run: the interface topology snapshot, per-interface IFAC state, the wake and command channels, the inbound grant lanes, and the egress fan-out.
pub struct ReactorWiring {
    pub interfaces: std::vec::Vec<InterfaceDescriptor>,
    pub ifacs: std::vec::Vec<InterfaceIfac>,
    pub notify: UnboundedReceiver<InterfaceId>,
    pub inbound_lanes: std::vec::Vec<(InterfaceId, TokioGrantConsumer)>,
    pub commands: UnboundedReceiver<HostCommand>,
    pub egress: Egress,
}

pub async fn run<S, H, J>(engine: EngineState<S>, host: H, wiring: ReactorWiring, on_journaled: J)
where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
{
    run_with_deciders(
        engine,
        host,
        wiring,
        on_journaled,
        crate::reactor::decline_all(),
    )
    .await
}

pub async fn run_with_deciders<S, H, J, P, A>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring,
    on_journaled: J,
    deciders: AppDeciders<P, A>,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        deciders,
        None,
        CryptoPoolConfig::host_default(),
    )
    .await
}

pub async fn run_with_store<S, H, J>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring,
    on_journaled: J,
    store: InterfaceStore,
    crypto_pool_config: CryptoPoolConfig,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        crate::reactor::decline_all(),
        Some(store),
        crypto_pool_config,
    )
    .await
}

async fn run_inner<S, H, J, P, A>(
    mut engine: EngineState<S>,
    mut host: H,
    wiring: ReactorWiring,
    on_journaled: J,
    deciders: AppDeciders<P, A>,
    store: Option<InterfaceStore>,
    crypto_pool_config: CryptoPoolConfig,
) where
    S: StorageLayout,
    H: Host,
    J: FnMut(Journaled<'_>),
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    let AppDeciders {
        mut should_prove,
        mut should_accept_resource,
    } = deciders;
    let ReactorWiring {
        interfaces,
        ifacs,
        mut notify,
        inbound_lanes,
        mut commands,
        egress,
    } = wiring;
    let mut topology =
        InterfaceTopology::new(interfaces, ifacs, inbound_lanes, egress, &mut engine, &host);
    let mut wake_schedules = engine.wake_schedules(topology.view());
    let mut scratch_cap = topology.frame_cap();
    let mut wire_scratch = WireScratch::new(scratch_cap);
    let mut unmask_scratch = std::vec![0u8; scratch_cap].into_boxed_slice();
    let mut journal = JournalDispatch::new(on_journaled);
    macro_rules! journaled_sink {
        () => {
            |journaled| journal.route(journaled)
        };
    }
    const MAX_INBOUND_BATCH: usize = 64;
    const MAX_COMMAND_BATCH: usize = 64;
    let (crypto_tx, mut crypto_rx) = tokio::sync::mpsc::unbounded_channel::<CryptoResult>();
    let crypto_pool = crypto_pool_config
        .resolved_worker_count()
        .and_then(|workers| CryptoPool::spawn(workers.get(), crypto_tx.clone()));
    let _crypto_tx = crypto_tx;
    if crypto_pool.is_some() {
        engine.resource_open_lane = ResourceOpenLane::PoolWhenContended;
    }
    let mut dirty: std::vec::Vec<InterfaceId> = std::vec::Vec::new();
    let due_timer = tokio::time::sleep_until(Instant::now());
    tokio::pin!(due_timer);
    let mut armed: Option<(InstantMillis, WakeReason)> = None;
    let pacer_timer = tokio::time::sleep_until(Instant::now());
    tokio::pin!(pacer_timer);
    let mut pacer_armed: Option<InstantMillis> = None;
    loop {
        match soonest_pacer_release(&topology.pacers) {
            None => pacer_armed = None,
            Some(at) => {
                if pacer_armed != Some(at) {
                    pacer_timer.as_mut().reset(bounded_timer_deadline(
                        Instant::now(),
                        host.now(),
                        at,
                    ));
                }
                pacer_armed = Some(at);
            }
        }
        match wake_schedules.soonest(host.now()) {
            NextWake::Idle => armed = None,
            NextWake::Due(reason) => {
                due_timer.as_mut().reset(Instant::now());
                armed = Some((InstantMillis(0), reason));
            }
            NextWake::At { at, reason } => {
                if armed.map(|(deadline, _)| deadline) != Some(at) {
                    due_timer.as_mut().reset(bounded_timer_deadline(
                        Instant::now(),
                        host.now(),
                        at,
                    ));
                }
                armed = Some((at, reason));
            }
        }
        macro_rules! dispatch_owed_open_spans {
            () => {{
                if let Some(pool) = crypto_pool.as_ref() {
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
            }};
        }
        // Retaining lanes with queued frames prevents a batch tail from becoming stranded after its notification is consumed.
        macro_rules! process_dirty_lanes {
            () => {{
                let now = host.now();
                for &source in &dirty {
                    let Some((_, lane)) = topology
                        .inbound_lanes
                        .iter_mut()
                        .find(|(id, _)| *id == source)
                    else {
                        continue;
                    };
                    lane.acknowledge();
                    for _ in 0..MAX_INBOUND_BATCH {
                        if crypto_pool
                            .as_ref()
                            .is_some_and(|pool| !pool.has_queue_capacity(2))
                        {
                            break;
                        }
                        let Some(slot) = lane.try_peek() else { break };
                        let packet_phy = slot.packet_phy;
                        let bytes = match ifac_for(&topology.ifacs, source) {
                            Some(entry) => {
                                let Some(clean_len) = entry
                                    .context
                                    .unmask_inbound(slot.frame(), &mut unmask_scratch)
                                else {
                                    lane.release();
                                    continue;
                                };
                                &mut unmask_scratch[..clean_len]
                            }
                            None => slot.frame_mut(),
                        };
                        let packet = ClassifiedInboundPacket::classify(InboundPacket {
                            arrived_at: now,
                            source_interface: source,
                            bytes,
                        });
                        if let Some(packet_hash) = packet.packet_hash() {
                            retain_packet_phy(store.as_ref(), packet_hash, packet_phy);
                        }
                        if let Some(pool) = &crypto_pool {
                            if let Some((address, payload)) = packet.proof() {
                                if let Some(deferred) = engine.settle_receipt_proof_deferred(
                                    payload,
                                    &DestinationHash::from_address(address),
                                    now,
                                ) {
                                    let settle = match deferred.ingest {
                                        ProofIngest::SendSinglePacketDelivered {
                                            id,
                                            delivered,
                                        } => {
                                            Some((id, Settlement::SendSinglePacket(Ok(delivered))))
                                        }
                                        ProofIngest::SendToLinkDelivered { id, delivered } => {
                                            Some((id, Settlement::SendToLink(Ok(delivered))))
                                        }
                                        _ => None,
                                    };
                                    if let Some((id, settlement)) = settle {
                                        pool.submit(CryptoJob::Verify(EngineVerifyJob {
                                            packet_hash: deferred.packet_hash,
                                            signing_key: deferred.signing_key,
                                            signature: deferred.signature,
                                            id,
                                            settlement,
                                        }));
                                    }
                                    lane.release();
                                    continue;
                                }
                            }
                        }
                        let wake_schedules_delta = match &crypto_pool {
                            Some(pool) => {
                                let mut deferred_sign = None;
                                let mut deferred = DeferredCrypto::default();
                                let delta = engine.ingest_classified_into_deferring(
                                    packet,
                                    IngestIo {
                                        interfaces: topology.interfaces.view(),
                                        now,
                                        fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                        should_prove: &mut should_prove,
                                        should_accept_resource: &mut should_accept_resource,
                                        sink: &mut |reaction| {
                                            route_reaction(
                                                reaction,
                                                &mut topology.egress,
                                                &topology.ifacs,
                                                &mut topology.pacers,
                                                &mut wire_scratch,
                                                now,
                                                &mut journaled_sink!(),
                                            )
                                        },
                                    },
                                    &mut deferred_sign,
                                    Some(&mut deferred),
                                );
                                if let Some(owed) = deferred_sign {
                                    pool.submit(CryptoJob::Sign(owed));
                                }
                                match deferred {
                                    DeferredCrypto::Empty => {}
                                    DeferredCrypto::Decrypt(owed) => {
                                        pool.submit(CryptoJob::Decrypt(owed));
                                    }
                                    DeferredCrypto::RatchetDecrypt(owed) => {
                                        pool.submit(CryptoJob::DecryptWithRatchets(Box::new(owed)));
                                    }
                                    DeferredCrypto::LinkProofVerify(owed) => {
                                        pool.submit(CryptoJob::VerifyLinkProof(owed));
                                    }
                                    DeferredCrypto::LinkProofSign(owed) => {
                                        pool.submit(CryptoJob::SignLinkProof(owed));
                                    }
                                    DeferredCrypto::AnnounceVerify(owed) => {
                                        pool.submit(CryptoJob::VerifyAnnounce(owed));
                                    }
                                }
                                delta
                            }
                            None => engine.ingest_classified_into(
                                packet,
                                IngestIo {
                                    interfaces: topology.interfaces.view(),
                                    now,
                                    fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                    should_prove: &mut should_prove,
                                    should_accept_resource: &mut should_accept_resource,
                                    sink: &mut |reaction| {
                                        route_reaction(
                                            reaction,
                                            &mut topology.egress,
                                            &topology.ifacs,
                                            &mut topology.pacers,
                                            &mut wire_scratch,
                                            now,
                                            &mut journaled_sink!(),
                                        )
                                    },
                                },
                            ),
                        };
                        lane.release();
                        merge_wake_schedules_delta(
                            &mut wake_schedules,
                            wake_schedules_delta,
                            &engine,
                            topology.interfaces.view(),
                        );
                        dispatch_owed_open_spans!();
                    }
                }
                dirty.retain(|source| {
                    topology
                        .inbound_lanes
                        .iter_mut()
                        .find(|(id, _)| id == source)
                        .is_some_and(|(_, lane)| lane.try_peek().is_some())
                });
            }};
        }
        tokio::select! {
            arrived = notify.recv() => {
                let Some(source) = arrived else { return };
                if !dirty.contains(&source) {
                    dirty.push(source);
                }
                while let Ok(more) = notify.try_recv() {
                    if !dirty.contains(&more) {
                        dirty.push(more);
                    }
                }
                process_dirty_lanes!();
            }
            _ = tokio::task::yield_now(), if !dirty.is_empty() => {
                while let Ok(more) = notify.try_recv() {
                    if !dirty.contains(&more) {
                        dirty.push(more);
                    }
                }
                process_dirty_lanes!();
            }
            _ = tokio::task::yield_now(), if dirty.is_empty() && engine.owed_staged_seal_link().is_some() => {
                if let Some(link_id) = engine.owed_staged_seal_link() {
                    match crypto_pool.as_ref() {
                        Some(pool) => {
                            if let Some(view) = engine.staged_seal_job_view(&link_id) {
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
                                    nonce_prefixed_len: view.nonce_prefixed_len,
                                    plaintext: view.plaintext.to_vec(),
                                    seal_iv,
                                    salts,
                                };
                                engine.mark_staged_sealing(&link_id);
                                pool.submit(CryptoJob::SealStaged(Box::new(job)));
                            }
                        }
                        None => {
                            let now = host.now();
                            engine.seal_staged_continuation(
                                &link_id,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                        }
                    }
                }
            }
            issued = commands.recv() => {
                let Some(mut issued) = issued else { return };
                let now = host.now();
                let mut command_budget = MAX_COMMAND_BATCH;
                loop {
                let effect = CommandDispatch {
                    engine: &mut engine,
                    host: &mut host,
                    topology: &mut topology,
                    wire_scratch: &mut wire_scratch,
                    journal: &mut journal,
                    crypto_pool: crypto_pool.as_ref(),
                }
                .dispatch(issued, now);
                match effect {
                    CommandEffect::Delta(delta) => merge_wake_schedules_delta(
                        &mut wake_schedules,
                        delta,
                        &engine,
                        topology.view(),
                    ),
                    CommandEffect::RecomputeWakeSchedules => {
                        wake_schedules = engine.wake_schedules(topology.view());
                    }
                    CommandEffect::InterfaceAttached { frame_capacity } => {
                        if frame_capacity > scratch_cap {
                            scratch_cap = frame_capacity;
                            unmask_scratch = std::vec![0u8; scratch_cap].into_boxed_slice();
                            wire_scratch.grow(scratch_cap);
                        }
                        wake_schedules = engine.wake_schedules(topology.view());
                    }
                }
                command_budget -= 1;
                if command_budget == 0 {
                    break;
                }
                match commands.try_recv() {
                    Ok(next) => issued = next,
                    Err(_) => break,
                }
                }
            }
            () = &mut due_timer, if armed.is_some() => {
                if let Some((deadline, reason)) = armed.take() {
                    let now = host.now();
                    if deadline <= now {
                        let wake_schedules_delta = fire_due_reason(
                            &mut engine,
                            reason,
                            now,
                            topology.interfaces.view(),
                            &mut |bytes| host.fill_entropy(bytes),
                            &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                        );
                        merge_wake_schedules_delta(&mut wake_schedules, wake_schedules_delta, &engine, topology.view());
                    }
                }
            }
            () = &mut pacer_timer, if pacer_armed.is_some() => {
                pacer_armed = None;
                let now = host.now();
                flush_due_pacers(&mut topology.pacers, now, &mut topology.egress, &topology.ifacs);
            }
            verdict = crypto_rx.recv(), if crypto_pool.is_some() => {
                let mut next = verdict;
                let now = host.now();
                let mut seal_buf = [0u8; crate::wire::BROADCAST_MTU];
                while let Some(result) = next {
                    if let Some(pool) = crypto_pool.as_ref() {
                        #[cfg(feature = "runtime-metrics")]
                        pool.record_completed();
                        if result.settles_packet_verdict() {
                            pool.packet_verdict_settled();
                        }
                    }
                    match result {
                        CryptoResult::Verified { id, settlement, valid } => {
                            if valid && engine.settle_resolved(id).is_some() {
                                route_reaction(
                                    EngineReaction::Journaled(Journaled::CommandSettled {
                                        id,
                                        settlement,
                                    }),
                                    &mut topology.egress,
                                    &topology.ifacs,
                                    &mut topology.pacers,
                                    &mut wire_scratch,
                                    now,
                                    &mut journaled_sink!(),
                                );
                            }
                        }
                        CryptoResult::Sealed {
                            owed,
                            ephemeral_public,
                            shared,
                        } => {
                            let delta = engine.complete_send_single_packet_deferred(
                                owed,
                                ephemeral_public,
                                shared,
                                topology.interfaces.view(),
                                &mut seal_buf,
                                &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, topology.view());
                        }
                        CryptoResult::Signed {
                            target,
                            packet_hash,
                            signature,
                        } => {
                            let mut proof = [0u8; EXPLICIT_PROOF_WIRE_LEN];
                            if let Ok(written) =
                                engine.write_signed_proof(&packet_hash, &signature, &mut proof)
                            {
                                route_reaction(
                                    EngineReaction::Directive(Directive::Send {
                                        target,
                                        bytes: &proof[..written],
                                    }),
                                    &mut topology.egress,
                                    &topology.ifacs,
                                    &mut topology.pacers,
                                    &mut wire_scratch,
                                    now,
                                    &mut journaled_sink!(),
                                );
                            }
                        }
                        CryptoResult::Decrypted { owed, shared } => {
                            let mut deferred_sign = None;
                            engine.resume_decrypt(
                                owed,
                                shared,
                                topology.interfaces.view(),
                                &mut should_prove,
                                &mut deferred_sign,
                                &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            if let Some(deferred) = deferred_sign {
                                if let Some(pool) = crypto_pool.as_ref() {
                                    pool.submit(CryptoJob::Sign(deferred));
                                }
                            }
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
                                    &mut should_prove,
                                    &mut deferred_sign,
                                    &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                );
                                if let Some(deferred) = deferred_sign {
                                    if let Some(pool) = crypto_pool.as_ref() {
                                        pool.submit(CryptoJob::Sign(deferred));
                                    }
                                }
                            }
                        }
                        CryptoResult::LinkProofVerified { owed, shared } => {
                            if let Some(shared) = shared {
                                let delta = engine.resume_link_proof(
                                    owed,
                                    shared,
                                    topology.interfaces.view(),
                                    now,
                                    &mut |entropy| host.fill_entropy(entropy),
                                    &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                );
                                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, topology.view());
                            }
                        }
                        CryptoResult::LinkProofSigned { owed, responder_encryption, shared, signature } => {
                            let delta = engine.resume_link_proof_sign(
                                owed,
                                responder_encryption,
                                shared,
                                signature,
                                topology.interfaces.view(),
                                &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, topology.view());
                        }
                        CryptoResult::StagedSealed { link_id, stream_nonce, nonce_prefixed_len, transfer, names, outcome } => {
                            let sealed_len = outcome.map_or(0, |sealed| sealed.sealed_transfer_len);
                            let names_len = outcome.map_or(0, |sealed| sealed.part_count * MAP_HASH_LEN);
                            engine.apply_offloaded_staged_seal(
                                OffloadedStagedSeal {
                                    link_id,
                                    stream_nonce,
                                    nonce_prefixed_len,
                                    sealed_bytes: &transfer[..sealed_len],
                                    names: &names[..names_len],
                                    outcome,
                                },
                                &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            engine.promote_staged_resource(
                                &link_id,
                                now,
                                &mut |entropy| host.fill_entropy(entropy),
                                &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(
                                &mut wake_schedules,
                                WakeSchedules {
                                    resource_deadlines: engine.resource_deadlines_wake(),
                                    ..WakeSchedules::UNCHANGED
                                },
                                &engine,
                                topology.view(),
                            );
                        }
                        CryptoResult::AnnounceVerified { owed, valid } => {
                            if valid {
                                let delta = engine.resume_announce(
                                    owed,
                                    topology.interfaces.view(),
                                    &mut |entropy| host.fill_entropy(entropy),
                                    &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                                );
                                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, topology.view());
                            }
                        }
                        CryptoResult::SpanOpened { link_id, hash, span_start, state, bytes } => {
                            let delta = engine.apply_opened_span(
                                OffloadedOpenSpan {
                                    link_id,
                                    hash,
                                    span_start,
                                    state,
                                    bytes: &bytes,
                                },
                                now,
                                &mut |reaction| route_reaction(reaction, &mut topology.egress, &topology.ifacs, &mut topology.pacers, &mut wire_scratch, now, &mut journaled_sink!()),
                            );
                            merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, topology.view());
                            dispatch_owed_open_spans!();
                        }
                    }
                    next = crypto_rx.try_recv().ok();
                }
                dispatch_owed_open_spans!();
            }
            _ = tokio::task::yield_now(), if crypto_pool.as_ref().is_some_and(CryptoPool::awaits_packet_verdict) => {}
        }
        if let Some(store) = &store {
            let mut dirty_interfaces = engine.take_dirty_interfaces();
            let mut changed = false;
            dirty_interfaces.drain(|interface| {
                if topology.view().descriptor_for(interface).is_some() {
                    store.set(interface, engine.interface_counts(interface));
                } else {
                    store.forget(interface);
                }
                changed = true;
            });
            if changed {
                store.bump();
            }
        }
    }
}

#[cfg(test)]
mod tests;
