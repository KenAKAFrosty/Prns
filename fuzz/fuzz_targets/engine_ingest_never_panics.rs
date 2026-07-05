#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::engine::{EngineState, IngestIo, InstantMillis, RatchetPolicy};
use prns_core::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use prns_core::interfaces::{
    AnnounceBandwidthCap, BitrateBps, EgressCapability, InboundPacket, IngressCapability,
    InterfaceCapabilities, InterfaceDescriptor, InterfaceId, InterfaceMode, TransportCapability,
};
use prns_core::routing::announce::defaults::JitterSeed;
use prns_core::routing::request_handlers::RequestPolicy;
use prns_core::routing::ProofStrategy;
use prns_core::storage::GrowableHeap;

const FRAME_CAP: usize = 512;

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
    }
}

fuzz_target!(|data: &[u8]| {
    let mut engine =
        EngineState::<GrowableHeap>::new(Zeroizing::new([0x07; IDENTITY_SECRET_KEY_LEN]));
    let node = engine.held_identity_hashes()[0];
    let destination = engine
        .register_single_destination(
            &node,
            "fuzz",
            &["inbound"],
            b"",
            ProofStrategy::ProveAll,
            RatchetPolicy::NoRatchets,
        )
        .expect("registers the fuzz destination");
    engine
        .register_request_handler(&destination, "/fuzz", RequestPolicy::AllowAll)
        .expect("registers the fuzz handler");

    let interfaces = [InterfaceId::new([0xBE; 8]), InterfaceId::new([0xBF; 8])];
    let descriptors = [
        interface_descriptor(interfaces[0]),
        interface_descriptor(interfaces[1]),
    ];

    let mut now = 1_000u64;
    let mut entropy_byte = 0u8;
    let mut chunk_index = 0usize;
    let mut rest = data;
    while let Some((&len_byte, tail)) = rest.split_first() {
        let take = (len_byte as usize).min(tail.len()).min(FRAME_CAP);
        let (chunk, remaining) = tail.split_at(take);
        rest = remaining;

        let mut bytes = chunk.to_vec();
        now += 7;
        chunk_index += 1;
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(now),
                source_interface: interfaces[chunk_index & 1],
                bytes: &mut bytes,
            },
            JitterSeed(0xCAFE_F00D_DEAD_BEEF),
            IngestIo {
                interfaces: &descriptors,
                now: InstantMillis(now),
                fill_entropy: &mut |buf: &mut [u8]| {
                    for byte in buf.iter_mut() {
                        *byte = entropy_byte;
                        entropy_byte = entropy_byte.wrapping_add(1);
                    }
                },
                should_prove: &mut |_| true,
                sink: &mut |_| {},
            },
        );
    }
});
