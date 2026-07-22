#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::engine::{
    Departure, EngineState, IngestIo, InstantMillis, NextWake, RatchetPolicy, WakeSchedules,
};
use prns_core::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use prns_core::interfaces::{
    AnnounceBandwidthCap, AttachedInterfaces, BitrateBps, EgressCapability, InboundPacket,
    IngressCapability, InterfaceCapabilities, InterfaceDescriptor, InterfaceId, InterfaceMode,
    TransportCapability,
};
use prns_core::routing::request_handlers::RequestPolicy;
use prns_core::routing::{LinkRequestPolicy, ProofStrategy};
use prns_core::storage::GrowableHeap;
use prns_runtime::reactor::kernel::{fire_due_reason, merge_wake_schedules_delta};

const FRAME_CAP: usize = u16::MAX as usize;

fn interface_descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        bitrate: BitrateBps::guess(1_000_000_000),
        hardware_mtu: None,
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
        airtime_duty_cycle: None,
        common: prns_runtime::interfaces::InterfaceCommonPolicy::RNS_DEFAULT,
    }
}

fn ingest_frame(
    engine: &mut EngineState<GrowableHeap>,
    descriptors: &[InterfaceDescriptor],
    wake_schedules: &mut WakeSchedules,
    source_interface: InterfaceId,
    now: u64,
    entropy_byte: &mut u8,
    chunk: &[u8],
) {
    let mut bytes = chunk.to_vec();
    let delta = engine.ingest_packet_into(
        InboundPacket {
            arrived_at: InstantMillis(now),
            source_interface,
            bytes: &mut bytes,
        },
        IngestIo {
            interfaces: AttachedInterfaces::new(descriptors),
            now: InstantMillis(now),
            fill_entropy: &mut |buf: &mut [u8]| {
                for byte in buf.iter_mut() {
                    *byte = *entropy_byte;
                    *entropy_byte = entropy_byte.wrapping_add(1);
                }
            },
            should_prove: &mut |_| true,
            should_accept_resource: &mut |_| true,
            sink: &mut |_| {},
        },
    );
    merge_wake_schedules_delta(
        wake_schedules,
        delta,
        engine,
        AttachedInterfaces::new(descriptors),
    );
}

fuzz_target!(|data: &[u8]| {
    let mut engine =
        EngineState::<GrowableHeap>::new(Zeroizing::new([0x07; IDENTITY_SECRET_KEY_LEN]));
    let node = engine
        .held_identity_hashes()
        .first()
        .copied()
        .expect("the engine holds its initial identity");
    let destination = engine
        .register_single_destination(
            &node,
            "fuzz",
            &["inbound"],
            b"",
            ProofStrategy::ProveAll,
            LinkRequestPolicy::AcceptAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the fuzz destination");
    engine
        .register_request_handler(&destination, "/fuzz", RequestPolicy::AllowAll)
        .expect("registers the fuzz handler");

    let first_interface = InterfaceId::new([0xBE; 8]);
    let second_interface = InterfaceId::new([0xBF; 8]);
    let interfaces = [first_interface, second_interface];
    let mut descriptors = std::vec![
        interface_descriptor(first_interface),
        interface_descriptor(second_interface),
    ];

    let mut now = 1_000u64;
    for interface in interfaces {
        engine.interface_attached(interface, InstantMillis(now));
    }
    let mut wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&descriptors));
    let mut entropy_byte = 0u8;
    if data.first().is_some_and(|first| first & 0x80 != 0) {
        let mut legacy_index = 0usize;
        let mut legacy_rest = data;
        while let Some((&len, tail)) = legacy_rest.split_first() {
            let take = usize::from(len).min(tail.len());
            let (chunk, remaining) = tail.split_at(take);
            legacy_rest = remaining;
            let source_interface = if legacy_index & 1 == 0 {
                first_interface
            } else {
                second_interface
            };
            legacy_index = legacy_index.saturating_add(1);
            ingest_frame(
                &mut engine,
                &descriptors,
                &mut wake_schedules,
                source_interface,
                now,
                &mut entropy_byte,
                chunk,
            );
        }
    }
    let mut chunk_index = 0usize;
    let mut rest = data;
    while let Some((&operation, tail)) = rest.split_first() {
        rest = tail;
        match operation % 6 {
            0 => {
                let Some((&low, tail)) = rest.split_first() else {
                    break;
                };
                let Some((&high, tail)) = tail.split_first() else {
                    break;
                };
                let take = usize::from(u16::from_le_bytes([low, high]))
                    .min(tail.len())
                    .min(FRAME_CAP);
                let (chunk, remaining) = tail.split_at(take);
                rest = remaining;
                let Some(source) = descriptors.get(chunk_index % descriptors.len().max(1)) else {
                    continue;
                };
                let source_interface = source.id;
                chunk_index = chunk_index.saturating_add(1);
                ingest_frame(
                    &mut engine,
                    &descriptors,
                    &mut wake_schedules,
                    source_interface,
                    now,
                    &mut entropy_byte,
                    chunk,
                );
            }
            1 | 2 => {
                let mut encoded = [0u8; 8];
                let take = rest.len().min(encoded.len());
                let (value_bytes, remaining) = rest.split_at(take);
                for (target, source) in encoded.iter_mut().zip(value_bytes) {
                    *target = *source;
                }
                rest = remaining;
                let value = u64::from_le_bytes(encoded);
                if operation % 6 == 1 {
                    now = now.max(value);
                } else {
                    now = now.saturating_add(value);
                }
            }
            3 => {
                let Some((&selector, tail)) = rest.split_first() else {
                    break;
                };
                rest = tail;
                let id = if selector & 1 == 0 {
                    first_interface
                } else {
                    second_interface
                };
                if let Some(position) = descriptors.iter().position(|entry| entry.id == id) {
                    descriptors.swap_remove(position);
                    let departure = if selector & 2 == 0 {
                        Departure::MayReturn
                    } else {
                        Departure::Forgotten
                    };
                    engine.interface_departed(id, departure, InstantMillis(now));
                } else {
                    descriptors.push(interface_descriptor(id));
                    engine.interface_attached(id, InstantMillis(now));
                }
                wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&descriptors));
            }
            4 => {
                let due = match wake_schedules.soonest(InstantMillis(now)) {
                    NextWake::Idle => None,
                    NextWake::Due(reason) => Some(reason),
                    NextWake::At { at, reason } => {
                        now = at.0;
                        Some(reason)
                    }
                };
                if let Some(reason) = due {
                    let delta = fire_due_reason(
                        &mut engine,
                        reason,
                        InstantMillis(now),
                        AttachedInterfaces::new(&descriptors),
                        &mut |buf: &mut [u8]| {
                            for byte in buf.iter_mut() {
                                *byte = entropy_byte;
                                entropy_byte = entropy_byte.wrapping_add(1);
                            }
                        },
                        &mut |_| {},
                    );
                    merge_wake_schedules_delta(
                        &mut wake_schedules,
                        delta,
                        &engine,
                        AttachedInterfaces::new(&descriptors),
                    );
                }
            }
            _ => {
                wake_schedules = engine.wake_schedules(AttachedInterfaces::new(&descriptors));
            }
        }
    }
});
