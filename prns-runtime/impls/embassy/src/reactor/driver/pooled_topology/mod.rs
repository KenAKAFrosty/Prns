use embassy_futures::select::{select5, Either5};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Receiver;
use heapless::Vec as HeaplessVec;

use crate::engine::{
    ClassifiedInboundPacket, Departure, EngineState, IngestIo, IssuedCommand, Journaled,
    ProofRequest,
};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{AttachedInterfaces, InboundPacket, InterfaceDescriptor, InterfaceId};
use crate::reactor::grant::{AnyGrantConsumer, FrameTarget};
use crate::reactor::interface_seam::{EMBEDDED_MAX_LINK_MTU, EMBEDDED_MAX_WIRE_FRAME_LEN};
use crate::reactor::kernel::{fire_due_reason, merge_wake_schedules_delta};
use crate::reactor::timers::{wait_for_due_reason, wait_for_pacer};
use crate::reactor::{AppDeciders, Host};
use crate::routing::links::resources::ResourceOffer;
use crate::runtime::InterfaceInspectionStore;
use crate::storage::{DirtyInterfaceSet, StorageLayout};

use super::super::grant_lane::EmbassyGrantConsumer;
use super::egress::{
    flush_due_pacers, ifac_for, owns_dedicated_lane, route_reaction, soonest_pacer_release,
    InterfacePacer, PooledEgress,
};
use super::packet_phy::retain_packet_phy;

/// Changes the live descriptor set without reallocating the fixed lane pool.
#[repr(C)]
pub enum InterfaceLifecycle {
    Add {
        descriptor: InterfaceDescriptor,
    },
    Remove {
        id: InterfaceId,
    },
    Retag {
        old_id: InterfaceId,
        new_id: InterfaceId,
        descriptor: InterfaceDescriptor,
    },
}

fn clamp_to_embedded_ceiling(mut descriptor: InterfaceDescriptor) -> InterfaceDescriptor {
    if let Some(mtu) = descriptor.hardware_mtu {
        descriptor.hardware_mtu = Some(mtu.min(EMBEDDED_MAX_LINK_MTU));
    }
    descriptor
}

/// Borrowed lanes and channels for one pooled-topology reactor run.
pub struct PooledWiring<
    'run,
    M: RawMutex + 'static,
    const FRAME: usize,
    const LANE_COUNT: usize,
    const INTERFACE_CAPACITY: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
> {
    pub descriptors: &'run mut HeaplessVec<InterfaceDescriptor, INTERFACE_CAPACITY>,
    pub ifacs: &'run mut HeaplessVec<InterfaceIfac, LANE_COUNT>,
    pub inbound:
        &'run mut HeaplessVec<(InterfaceId, EmbassyGrantConsumer<'static, M, FRAME>), LANE_COUNT>,
    pub egress: &'run mut PooledEgress<M, FRAME, LANE_COUNT>,
    pub notify: Receiver<'run, M, InterfaceId, NOTIFY>,
    pub commands: Receiver<'run, M, IssuedCommand, COMMANDS>,
    pub lifecycle: Receiver<'run, M, InterfaceLifecycle, LIFECYCLE>,
}

/// Runs a mutable descriptor set over a fixed lane pool; `LANE_COUNT` bounds pacers.
pub(crate) async fn run_pooled<
    S,
    H,
    M,
    Store,
    const FRAME: usize,
    const LANE_COUNT: usize,
    const INTERFACE_CAPACITY: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
>(
    engine: &mut EngineState<S>,
    host: &mut H,
    wiring: PooledWiring<'_, M, FRAME, LANE_COUNT, INTERFACE_CAPACITY, NOTIFY, COMMANDS, LIFECYCLE>,
    mut on_journaled: impl FnMut(Journaled<'_>),
    deciders: AppDeciders<impl FnMut(&ProofRequest) -> bool, impl FnMut(&ResourceOffer) -> bool>,
    store: &Store,
) where
    S: StorageLayout,
    H: Host,
    M: RawMutex + 'static,
    Store: InterfaceInspectionStore,
{
    let AppDeciders {
        mut should_prove,
        mut should_accept_resource,
    } = deciders;
    let PooledWiring {
        descriptors,
        ifacs,
        inbound,
        egress,
        notify,
        commands,
        lifecycle,
    } = wiring;
    let mut pacers: HeaplessVec<InterfacePacer, LANE_COUNT> = HeaplessVec::new();
    for descriptor in descriptors.iter_mut() {
        *descriptor = clamp_to_embedded_ceiling(*descriptor);
        engine.interface_attached(descriptor.id, host.now());
        if owns_dedicated_lane(inbound, descriptor.id) {
            let _ = pacers.push(InterfacePacer::from_descriptor(descriptor.id, descriptor));
        }
    }
    let mut wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);

        match select5(
            notify.receive(),
            commands.receive(),
            wait_for_due_reason(&*host, wake),
            wait_for_pacer(&*host, pacer_wake),
            lifecycle.receive(),
        )
        .await
        {
            Either5::First(_) => {
                while notify.try_receive().is_ok() {}
                for (lane_id, lane) in inbound.iter_mut() {
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
                                interfaces: AttachedInterfaces::new(&*descriptors),
                                now,
                                fill_entropy: &mut |entropy| host.fill_entropy(entropy),
                                should_prove: &mut should_prove,
                                should_accept_resource: &mut should_accept_resource,
                                sink: &mut |reaction| {
                                    route_reaction(
                                        reaction,
                                        &mut *egress,
                                        ifacs,
                                        &mut pacers,
                                        now,
                                        &mut on_journaled,
                                    )
                                },
                            },
                        );
                        lane.release_frame();
                        merge_wake_schedules_delta(
                            &mut wake_schedules,
                            delta,
                            &*engine,
                            AttachedInterfaces::new(&*descriptors),
                        );
                    }
                }
            }
            Either5::Second(issued) => {
                let now = host.now();
                let delta = engine.ingest_command_into(
                    issued,
                    AttachedInterfaces::new(&*descriptors),
                    now,
                    &mut |entropy| host.fill_entropy(entropy),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut *egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    },
                );
                merge_wake_schedules_delta(
                    &mut wake_schedules,
                    delta,
                    &*engine,
                    AttachedInterfaces::new(&*descriptors),
                );
            }
            Either5::Third(reason) => {
                let now = host.now();
                let delta = fire_due_reason(
                    &mut *engine,
                    reason,
                    now,
                    AttachedInterfaces::new(&*descriptors),
                    &mut |bytes| host.fill_entropy(bytes),
                    &mut |reaction| {
                        route_reaction(
                            reaction,
                            &mut *egress,
                            ifacs,
                            &mut pacers,
                            now,
                            &mut on_journaled,
                        )
                    },
                );
                merge_wake_schedules_delta(
                    &mut wake_schedules,
                    delta,
                    &*engine,
                    AttachedInterfaces::new(&*descriptors),
                );
            }
            Either5::Fourth(()) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &mut *egress, ifacs);
            }
            Either5::Fifth(message) => match message {
                InterfaceLifecycle::Add { descriptor } => {
                    let descriptor = clamp_to_embedded_ceiling(descriptor);
                    let id = descriptor.id;
                    let present = descriptors.iter().any(|existing| existing.id == id);
                    if !present {
                        engine.interface_attached(id, host.now());
                        let _ = descriptors.push(descriptor);
                        if owns_dedicated_lane(inbound, id) {
                            let _ = pacers.push(InterfacePacer::from_descriptor(id, &descriptor));
                        }
                        wake_schedules =
                            engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
                    }
                    #[cfg(feature = "log")]
                    log::info!(
                        "reactor: Add kind={:?} present={present} descriptors={}",
                        id.kind(),
                        descriptors.len()
                    );
                }
                InterfaceLifecycle::Remove { id } => {
                    let now = host.now();
                    engine.interface_departed(id, Departure::Forgotten, now);
                    let found = descriptors
                        .iter()
                        .position(|descriptor| descriptor.id == id);
                    if let Some(pos) = found {
                        let _ = descriptors.swap_remove(pos);
                    }
                    #[cfg(feature = "log")]
                    log::info!(
                        "reactor: Remove kind={:?} found={} descriptors={}",
                        id.kind(),
                        found.is_some(),
                        descriptors.len()
                    );
                    if let Some(pos) = pacers.iter().position(|pacer| pacer.id == id) {
                        let _ = pacers.swap_remove(pos);
                    }
                    engine.cull_expired_routes(
                        now,
                        AttachedInterfaces::new(&*descriptors),
                        &mut |reaction| {
                            route_reaction(
                                reaction,
                                &mut *egress,
                                ifacs,
                                &mut pacers,
                                now,
                                &mut on_journaled,
                            )
                        },
                    );
                    wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
                }
                InterfaceLifecycle::Retag {
                    old_id,
                    new_id,
                    descriptor,
                } => {
                    let descriptor = clamp_to_embedded_ceiling(descriptor);
                    let present = descriptors
                        .iter()
                        .position(|existing| existing.id == old_id);
                    let collides = descriptors.iter().any(|existing| existing.id == new_id);
                    if let (Some(slot), false) = (present, collides) {
                        descriptors[slot] = descriptor;
                        egress.retag(old_id, new_id);
                        if let Some(entry) = inbound.iter_mut().find(|(id, _)| *id == old_id) {
                            entry.0 = new_id;
                        }
                        if let Some(entry) = ifacs.iter_mut().find(|entry| entry.id == old_id) {
                            entry.id = new_id;
                        }
                        if let Some(pos) = pacers.iter().position(|pacer| pacer.id == old_id) {
                            pacers[pos] = InterfacePacer::from_descriptor(new_id, &descriptor);
                        }
                        wake_schedules =
                            engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
                    }
                }
            },
        }
        if Store::RETAINS_COUNTS {
            let mut dirty = engine.take_dirty_interfaces();
            let mut changed = false;
            dirty.drain(|interface| {
                if descriptors
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
