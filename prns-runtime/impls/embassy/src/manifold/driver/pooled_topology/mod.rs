use embassy_futures::select::{select6, Either6};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Receiver;
use heapless::Vec as HeaplessVec;

use crate::engine::{
    ClassifiedInboundPacket, Departure, EngineState, IngestIo, IssuedCommand, Journaled,
    ProofRequest,
};
use crate::interfaces::InterfaceIfac;
use crate::interfaces::{
    AttachedInterfaces, IfacUnmaskError, InboundPacket, InterfaceDescriptor, InterfaceId,
};
use crate::manifold::grant::{FrameTarget, ManifoldLaneReader};
use crate::manifold::interface_seam::{EMBEDDED_MAX_LINK_MTU, EMBEDDED_MAX_WIRE_FRAME_LEN};
use crate::manifold::kernel::{fire_due_reason, merge_wake_schedules_delta};
use crate::manifold::timers::{wait_for_due_reason, wait_for_pacer};
use crate::manifold::{AppDeciders, Host};
use crate::routing::links::resources::ResourceOffer;
use crate::runtime::{InterfaceInspectionStore, ManifoldPersistence};
use crate::storage::{DirtyInterfaceSet, StorageLayout};

use super::egress::{
    flush_due_pacers, ifac_for, route_reaction, soonest_pacer_release, InterfacePacer,
    ManifoldEgress, PooledEgress,
};
use super::interface_status::account_protocol_violation;
use super::packet_phy::retain_packet_phy;
use super::EmbassyInterfaceStatus;

/// Changes the live descriptor set without reallocating the fixed lane pool.
#[repr(C)]
pub enum InterfaceLifecycle {
    Add {
        descriptor: InterfaceDescriptor,
    },
    Remove {
        id: InterfaceId,
    },
    Update {
        descriptor: InterfaceDescriptor,
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

fn inbound_source(
    lane_id: InterfaceId,
    stamped_source: InterfaceId,
    descriptors: &[InterfaceDescriptor],
) -> InterfaceId {
    if descriptors
        .iter()
        .any(|descriptor| descriptor.id == lane_id)
    {
        lane_id
    } else {
        stamped_source
    }
}

/// Borrowed lanes and channels for one pooled-topology manifold run.
pub struct PooledWiring<
    'run,
    M: RawMutex + 'static,
    const LANE_COUNT: usize,
    const INTERFACE_CAPACITY: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
> {
    pub descriptors: &'run mut HeaplessVec<InterfaceDescriptor, INTERFACE_CAPACITY>,
    pub ifacs: &'run mut HeaplessVec<InterfaceIfac, LANE_COUNT>,
    pub inbound:
        &'run mut HeaplessVec<(InterfaceId, &'static mut dyn ManifoldLaneReader), LANE_COUNT>,
    pub frame_accounting_statuses: &'run [&'static EmbassyInterfaceStatus],
    pub egress: &'run mut PooledEgress<LANE_COUNT>,
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
    const LANE_COUNT: usize,
    const INTERFACE_CAPACITY: usize,
    const NOTIFY: usize,
    const COMMANDS: usize,
    const LIFECYCLE: usize,
>(
    engine: &mut EngineState<S>,
    host: &mut H,
    wiring: PooledWiring<'_, M, LANE_COUNT, INTERFACE_CAPACITY, NOTIFY, COMMANDS, LIFECYCLE>,
    mut on_journaled: impl FnMut(Journaled<'_>),
    deciders: AppDeciders<impl FnMut(&ProofRequest) -> bool, impl FnMut(&ResourceOffer) -> bool>,
    store: &Store,
    persistence: &mut impl ManifoldPersistence<S>,
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
        frame_accounting_statuses,
        egress,
        notify,
        commands,
        lifecycle,
    } = wiring;
    let mut pacers: HeaplessVec<InterfacePacer, LANE_COUNT> = HeaplessVec::new();
    for descriptor in descriptors.iter_mut() {
        *descriptor = clamp_to_embedded_ceiling(*descriptor);
        engine.interface_attached(descriptor.id, host.now());
        if let Some(lane) = egress.lane_for(descriptor.id) {
            if !pacers.iter().any(|pacer| pacer.id == lane) {
                let _ = pacers.push(InterfacePacer::from_descriptor(lane, descriptor));
            }
        }
    }
    let mut wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
    loop {
        let wake = wake_schedules.soonest(host.now());
        let pacer_wake = soonest_pacer_release(&pacers);

        let persistence_deadline = persistence.deadline(host.now());
        match select6(
            notify.receive(),
            commands.receive(),
            wait_for_due_reason(&*host, wake),
            wait_for_pacer(&*host, pacer_wake),
            lifecycle.receive(),
            wait_for_persistence(&*host, persistence_deadline),
        )
        .await
        {
            Either6::First(_) => {
                while notify.try_receive().is_ok() {}
                for (lane_id, lane) in inbound.iter_mut() {
                    while let Some((target, packet_phy, frame)) = lane.try_read() {
                        let FrameTarget::Direct(stamped_source) = target else {
                            lane.release();
                            continue;
                        };
                        // A dedicated lane's live key is authoritative. Runtime retagging updates
                        // that key atomically with the descriptor, while the already-constructed
                        // interface seam can still have stamped a queued frame with its prior id.
                        // Fleet lanes have no descriptor of their own, so their per-member stamp
                        // remains authoritative instead.
                        let source = inbound_source(*lane_id, stamped_source, descriptors);
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
                                        &mut |journaled| {
                                            persistence.observe(&journaled, now);
                                            on_journaled(journaled);
                                        },
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
                            &*engine,
                            AttachedInterfaces::new(&*descriptors),
                        );
                    }
                }
            }
            Either6::Second(issued) => {
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
                            &mut |journaled| {
                                persistence.observe(&journaled, now);
                                on_journaled(journaled);
                            },
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
            Either6::Third(reason) => {
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
                            &mut |journaled| {
                                persistence.observe(&journaled, now);
                                on_journaled(journaled);
                            },
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
            Either6::Fourth(()) => {
                let now = host.now();
                flush_due_pacers(&mut pacers, now, &mut *egress, ifacs);
            }
            Either6::Fifth(message) => match message {
                InterfaceLifecycle::Add { descriptor } => {
                    let descriptor = clamp_to_embedded_ceiling(descriptor);
                    let id = descriptor.id;
                    let present = descriptors.iter().any(|existing| existing.id == id);
                    if !present {
                        engine.interface_attached(id, host.now());
                        let _ = descriptors.push(descriptor);
                        if let Some(lane) = egress.lane_for(id) {
                            if !pacers.iter().any(|pacer| pacer.id == lane) {
                                let _ =
                                    pacers.push(InterfacePacer::from_descriptor(lane, &descriptor));
                            }
                        }
                        wake_schedules =
                            engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
                    }
                    #[cfg(feature = "log")]
                    log::info!(
                        target: "personal_hopspot_esp32",
                        "manifold: Add kind={:?} present={present} descriptors={}",
                        id.kind(),
                        descriptors.len()
                    );
                }
                InterfaceLifecycle::Remove { id } => {
                    let now = host.now();
                    let departed_lane = egress.lane_for(id);
                    engine.interface_departed(id, Departure::Forgotten, now);
                    let found = descriptors
                        .iter()
                        .position(|descriptor| descriptor.id == id);
                    if let Some(pos) = found {
                        let _ = descriptors.swap_remove(pos);
                    }
                    #[cfg(feature = "log")]
                    log::info!(
                        target: "personal_hopspot_esp32",
                        "manifold: Remove kind={:?} found={} descriptors={}",
                        id.kind(),
                        found.is_some(),
                        descriptors.len()
                    );
                    if let Some(lane) = departed_lane {
                        let lane_still_serves_a_descriptor = descriptors
                            .iter()
                            .any(|descriptor| egress.lane_for(descriptor.id) == Some(lane));
                        if !lane_still_serves_a_descriptor {
                            if let Some(pos) = pacers.iter().position(|pacer| pacer.id == lane) {
                                let _ = pacers.swap_remove(pos);
                            }
                        }
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
                                &mut |journaled| {
                                    persistence.observe(&journaled, now);
                                    on_journaled(journaled);
                                },
                            )
                        },
                    );
                    wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
                }
                InterfaceLifecycle::Update { descriptor } => {
                    let descriptor = clamp_to_embedded_ceiling(descriptor);
                    if let Some(slot) = descriptors
                        .iter()
                        .position(|existing| existing.id == descriptor.id)
                    {
                        descriptors[slot] = descriptor;
                        if let Some(lane) = egress.lane_for(descriptor.id) {
                            if let Some(pos) = pacers.iter().position(|pacer| pacer.id == lane) {
                                pacers[pos] = InterfacePacer::from_descriptor(lane, &descriptor);
                            }
                        }
                        wake_schedules =
                            engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
                    }
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
                        let old_lane = egress.lane_for(old_id);
                        descriptors[slot] = descriptor;
                        egress.retag(old_id, new_id);
                        if let Some(entry) = inbound.iter_mut().find(|(id, _)| *id == old_id) {
                            entry.0 = new_id;
                        }
                        if let Some(entry) = ifacs.iter_mut().find(|entry| entry.id == old_id) {
                            entry.id = new_id;
                        }
                        if let (Some(old_lane), Some(new_lane)) =
                            (old_lane, egress.lane_for(new_id))
                        {
                            if let Some(pos) = pacers.iter().position(|pacer| pacer.id == old_lane)
                            {
                                pacers[pos] =
                                    InterfacePacer::from_descriptor(new_lane, &descriptor);
                            }
                        }
                        wake_schedules =
                            engine.wake_schedules(AttachedInterfaces::new(&*descriptors));
                    }
                }
            },
            Either6::Sixth(()) => {}
        }
        let now = host.now();
        if persistence
            .deadline(now)
            .is_some_and(|deadline| deadline.0 <= now.0)
        {
            persistence.progress(engine, now).await;
            yield_now().await;
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

async fn wait_for_persistence(host: &impl Host, deadline: Option<crate::engine::InstantMillis>) {
    match deadline {
        Some(deadline) => host.sleep_until(deadline).await,
        None => core::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
