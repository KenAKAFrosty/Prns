use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Receiver;
use heapless::Vec as HeaplessVec;

use crate::engine::{
    ClassifiedInboundPacket, EngineState, IngestIo, IssuedCommand, Journaled, ProofRequest,
};
use crate::interfaces::ifac::InterfaceIfac;
use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceId};
use crate::reactor::grant::{AnyGrantConsumer, FrameTarget};
use crate::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use crate::reactor::kernel::{fire_due_reason, merge_wake_schedules_delta};
use crate::reactor::timers::{wait_for_due_reason, wait_for_pacer};
use crate::reactor::{AppDeciders, Host};
use crate::routing::links::resources::ResourceOffer;
use crate::runtime::{EmbassyInterfaceStore, InterfaceInspectionStore, NoInterfaceInspectionStore};
use crate::storage::{DirtyInterfaceSet, StorageLayout};

use super::egress::{
    flush_due_pacers, ifac_for, route_reaction, soonest_pacer_release, EmbassyEgress,
    InterfacePacer, MAX_PACED_INTERFACES,
};
use super::packet_phy::retain_packet_phy;

/// Everything the reactor is wired to for one run. All borrowed: the caller owns every lane for the reactor's whole life.
pub struct ReactorWiring<'run, 'lane, M: RawMutex, const NOTIFY: usize, const COMMANDS: usize> {
    pub interfaces: AttachedInterfaces<'run>,
    pub ifacs: &'run [InterfaceIfac],
    /// The inbound doorbell is not a queue. Seams and supervisors use `try_send`; a full channel means a sweep is already owed, and one ring drains every lane. A frame committed during the sweep either joins that sweep or rings again.
    pub notify: Receiver<'run, M, InterfaceId, NOTIFY>,
    pub inbound_lanes: &'run mut [(InterfaceId, &'lane mut dyn AnyGrantConsumer)],
    pub commands: Receiver<'run, M, IssuedCommand, COMMANDS>,
    pub egress: EmbassyEgress<'run>,
}

/// Run the reactor on an embassy executor until the task is dropped. `Idle` arms no timer, leaving the executor parked on its channels.
pub async fn run<S, H, M, const NOTIFY: usize, const COMMANDS: usize>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring<'_, '_, M, NOTIFY, COMMANDS>,
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
        crate::reactor::decline_all(),
    )
    .await
}

pub async fn run_with_deciders<S, H, M, P, A, const NOTIFY: usize, const COMMANDS: usize>(
    engine: EngineState<S>,
    host: H,
    wiring: ReactorWiring<'_, '_, M, NOTIFY, COMMANDS>,
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
    wiring: ReactorWiring<'_, '_, M, NOTIFY, COMMANDS>,
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
        crate::reactor::decline_all(),
        store,
    )
    .await
}

async fn run_inner<S, H, M, P, A, Store, const NOTIFY: usize, const COMMANDS: usize>(
    mut engine: EngineState<S>,
    mut host: H,
    wiring: ReactorWiring<'_, '_, M, NOTIFY, COMMANDS>,
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
    let ReactorWiring {
        interfaces,
        ifacs,
        notify,
        inbound_lanes,
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
                    while let Some((target, packet_phy, frame)) = lane.try_peek_frame() {
                        let FrameTarget::Direct(source) = target else {
                            lane.release_frame();
                            continue;
                        };
                        let mut unmasked = [0u8; EMBEDDED_MAX_WIRE_FRAME_LEN];
                        let bytes = match ifac_for(ifacs, *lane_id) {
                            Some(entry) => {
                                let Some(clean_len) =
                                    entry.context.unmask_inbound(frame, &mut unmasked)
                                else {
                                    lane.release_frame();
                                    continue;
                                };
                                &mut unmasked[..clean_len]
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
                        let delta = engine.ingest_classified_into(
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
                        lane.release_frame();
                        merge_wake_schedules_delta(&mut wake_schedules, delta, &engine, interfaces);
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
