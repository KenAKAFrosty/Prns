use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Receiver;
use heapless::Vec as HeaplessVec;

use crate::engine::{
    ClassifiedInboundPacket, EngineState, IngestIo, IssuedCommand, Journaled, ProofRequest,
};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{AttachedInterfaces, IfacUnmaskError, InboundPacket, InterfaceId};
use crate::manifold::grant::{FrameTarget, ManifoldLaneReader};
use crate::manifold::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::manifold::kernel::{fire_due_reason, merge_wake_schedules_delta};
use crate::manifold::timers::{wait_for_due_reason, wait_for_pacer};
use crate::manifold::{AppDeciders, Host};
use crate::routing::links::resources::ResourceOffer;
use crate::runtime::{EmbassyInterfaceStore, InterfaceInspectionStore, NoInterfaceInspectionStore};
use crate::storage::{DirtyInterfaceSet, StorageLayout};

use super::egress::{
    flush_due_pacers, ifac_for, route_reaction, soonest_pacer_release, EmbassyEgress,
    InterfacePacer, MAX_PACED_INTERFACES,
};
use super::interface_status::account_protocol_violation;
use super::packet_phy::retain_packet_phy;
use super::EmbassyInterfaceStatus;

/// Borrowed lanes and channels for one fixed-topology manifold run.
pub struct ManifoldWiring<'run, 'lane, M: RawMutex, const NOTIFY: usize, const COMMANDS: usize> {
    pub interfaces: AttachedInterfaces<'run>,
    pub ifacs: &'run [InterfaceIfac],
    /// A doorbell rather than a frame count: one receive drains every lane, a full channel means a sweep is pending, and a frame committed during a sweep either joins it or rings again.
    pub notify: Receiver<'run, M, InterfaceId, NOTIFY>,
    pub inbound_lanes: &'run mut [(InterfaceId, &'lane mut dyn ManifoldLaneReader)],
    pub frame_accounting_statuses: &'run [&'lane EmbassyInterfaceStatus],
    pub commands: Receiver<'run, M, IssuedCommand, COMMANDS>,
    pub egress: EmbassyEgress<'run>,
}

/// Runs until dropped; an `Idle` wake schedule arms no timer.
pub async fn run<S, H, M, const NOTIFY: usize, const COMMANDS: usize>(
    engine: EngineState<S>,
    host: H,
    wiring: ManifoldWiring<'_, '_, M, NOTIFY, COMMANDS>,
    on_journaled: impl FnMut(Journaled<'_>),
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex,
{
    run_with_deciders(
        engine,
        host,
        wiring,
        on_journaled,
        crate::manifold::decline_all(),
    )
    .await
}

pub async fn run_with_deciders<S, H, M, P, A, const NOTIFY: usize, const COMMANDS: usize>(
    engine: EngineState<S>,
    host: H,
    wiring: ManifoldWiring<'_, '_, M, NOTIFY, COMMANDS>,
    on_journaled: impl FnMut(Journaled<'_>),
    deciders: AppDeciders<P, A>,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex,
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
{
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        deciders,
        &NoInterfaceInspectionStore,
    )
    .await
}

pub async fn run_with_store<
    S,
    H,
    M,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const INTERFACES: usize,
    const PACKET_PHY_CAPACITY: usize,
    const PACKET_PHY_INDEX_BUCKETS: usize,
>(
    engine: EngineState<S>,
    host: H,
    wiring: ManifoldWiring<'_, '_, M, NOTIFY, COMMANDS>,
    on_journaled: impl FnMut(Journaled<'_>),
    store: &EmbassyInterfaceStore<M, INTERFACES, PACKET_PHY_CAPACITY, PACKET_PHY_INDEX_BUCKETS>,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex + Sync,
{
    assert!(
        INTERFACES >= wiring.interfaces.len(),
        "EmbassyInterfaceStore INTERFACES must cover every attached interface"
    );
    run_inner(
        engine,
        host,
        wiring,
        on_journaled,
        crate::manifold::decline_all(),
        store,
    )
    .await
}

async fn run_inner<S, H, M, P, A, Store, const NOTIFY: usize, const COMMANDS: usize>(
    mut engine: EngineState<S>,
    mut host: H,
    wiring: ManifoldWiring<'_, '_, M, NOTIFY, COMMANDS>,
    mut on_journaled: impl FnMut(Journaled<'_>),
    deciders: AppDeciders<P, A>,
    store: &Store,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex,
    P: FnMut(&ProofRequest) -> bool,
    A: FnMut(&ResourceOffer) -> bool,
    Store: InterfaceInspectionStore,
{
    let AppDeciders {
        mut should_prove,
        mut should_accept_resource,
    } = deciders;
    let ManifoldWiring {
        interfaces,
        ifacs,
        notify,
        inbound_lanes,
        frame_accounting_statuses,
        commands,
        mut egress,
    } = wiring;
    let mut wake_schedules = engine.wake_schedules(interfaces);
    let mut pacers: HeaplessVec<InterfacePacer, MAX_PACED_INTERFACES> = HeaplessVec::new();
    for descriptor in interfaces {
        let _ = pacers.push(InterfacePacer::from_descriptor(descriptor.id, descriptor));
    }
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);

        match select4(
            notify.receive(),
            commands.receive(),
            wait_for_due_reason(&host, wake),
            wait_for_pacer(&host, pacer_wake),
        )
        .await
        {
            Either4::First(_) => {
                while notify.try_receive().is_ok() {}
                for (lane_id, lane) in inbound_lanes.iter_mut() {
                    while let Some((target, packet_phy, frame)) = lane.try_read() {
                        let FrameTarget::Direct(source) = target else {
                            lane.release();
                            continue;
                        };
                        let mut unmasked = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
                        let bytes = match ifac_for(ifacs, *lane_id) {
                            Some(entry) => {
                                match entry.context.try_unmask_inbound(frame, &mut unmasked) {
                                    Ok(clean_len) => &mut unmasked[..clean_len],
                                    Err(IfacUnmaskError::PacketTooShort) => {
                                        account_protocol_violation(
                                            frame_accounting_statuses,
                                            source,
                                            Some(
                                                crate::engine::ProtocolViolationKind::InvalidIfacEnvelope,
                                            ),
                                        );
                                        lane.release();
                                        continue;
                                    }
                                    Err(
                                        IfacUnmaskError::MissingFlag
                                        | IfacUnmaskError::InvalidSignature
                                        | IfacUnmaskError::OutputTooSmall { .. },
                                    ) => {
                                        lane.release();
                                        continue;
                                    }
                                }
                            }
                            None => frame,
                        };
                        let now = host.now();
                        let packet = ClassifiedInboundPacket::classify(InboundPacket {
                            arrived_at: now,
                            source_interface: source,
                            bytes,
                        });
                        retain_packet_phy(store, &packet, packet_phy);
                        let report = engine.ingest_classified_into_report(
                            packet,
                            IngestIo {
                                interfaces,
                                now,
                                fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                should_prove: &mut should_prove,
                                should_accept_resource: &mut should_accept_resource,
                                sink: &mut |reaction| {
                                    route_reaction(
                                        reaction,
                                        &mut egress,
                                        ifacs,
                                        &mut pacers,
                                        now,
                                        &mut on_journaled,
                                    )
                                },
                            },
                        );
                        account_protocol_violation(
                            frame_accounting_statuses,
                            source,
                            report.protocol_violation,
                        );
                        lane.release();
                        merge_wake_schedules_delta(
                            &mut wake_schedules,
                            report.wake_schedules,
                            &engine,
                            interfaces,
                        );
                    }
                }
            }
            Either4::Second(issued) => {
                let now = host.now();
                let delta = engine.ingest_command_into(
                    issued,
                    interfaces,
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    },
                );
                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
            }
            Either4::Third(reason) => {
                let now = host.now();
                let delta = fire_due_reason(
                    &mut engine,
                    reason,
                    now,
                    interfaces,
                    &mut |bytes| host.fill_entropy(bytes),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    },
                );
                merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
            }
            Either4::Fourth(()) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &mut egress, ifacs);
            }
        }
        if Store::RETAINS_COUNTS {
            let mut dirty = engine.take_dirty_interfaces();
            let mut changed = false;
            dirty.drain(|interface| {
                if interfaces
                    .iter()
                    .any(|descriptor| descriptor.id == interface)
                {
                    store.set_interface_counts(interface, engine.interface_counts(interface));
                } else {
                    store.forget_interface(interface);
                }
                changed = true;
            });
            if changed {
                store.signal_interface_counts_changed();
            }
        }
    }
}

#[cfg(test)]
mod tests;
