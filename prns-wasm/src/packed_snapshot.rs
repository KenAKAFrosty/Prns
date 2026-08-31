use personal_rns::engine::EngineState;
use personal_rns::interfaces::{AttachedInterfaces, InterfaceDescriptor};
use personal_rns::routing::routes::NextHop;
use personal_rns::storage::GrowableHeap;

use crate::parameters::bitrate_bps_u32;

const PACKED_SNAPSHOT_MAGIC: u32 = u32::from_le_bytes(*b"PSNP");
const PACKED_SNAPSHOT_VERSION: u32 = 1;
const PACKED_SNAPSHOT_HEADER_BYTES: usize = 80;
const PACKED_INTERFACE_MAXIMUM_BYTES: usize = 41;
const PACKED_ROUTE_MAXIMUM_BYTES: usize = 66;
const PACKED_DESTINATION_IDENTITY_BYTES: usize = 32;

pub(crate) fn encode(
    engine: &EngineState<GrowableHeap>,
    interfaces: &[InterfaceDescriptor],
    revision: u64,
) -> Vec<u8> {
    let estimated_bytes = PACKED_SNAPSHOT_HEADER_BYTES
        .saturating_add(
            interfaces
                .len()
                .saturating_mul(PACKED_INTERFACE_MAXIMUM_BYTES),
        )
        .saturating_add(
            engine
                .route_count()
                .saturating_mul(PACKED_ROUTE_MAXIMUM_BYTES),
        )
        .saturating_add(
            engine
                .destination_identities()
                .count()
                .saturating_mul(PACKED_DESTINATION_IDENTITY_BYTES),
        );
    let mut writer = PackedSnapshotWriter::with_capacity(estimated_bytes);
    writer.u32(PACKED_SNAPSHOT_MAGIC);
    writer.u32(PACKED_SNAPSHOT_VERSION);
    writer.u64(revision);
    writer.u64(engine.ingested_packet_count());
    writer.u64(engine.ingested_command_count());
    writer.usize(engine.route_count());
    writer.usize(engine.scheduled_announce_count());
    writer.u64(u64::from(engine.link_count()));
    writer.usize(interfaces.len());
    let route_count_offset = writer.reserve_u64();
    let destination_identity_count_offset = writer.reserve_u64();

    for interface in interfaces {
        writer.bytes(interface.id.as_bytes());
        writer.u32(bitrate_bps_u32(interface.bitrate));
        writer.optional_u32(interface.hardware_mtu);
        writer.usize(engine.route_count_via(interface.id));
        writer.usize(engine.link_count_via(interface.id));
        writer.usize(engine.transported_link_count_via(interface.id));
    }

    let mut route_count = 0u64;
    engine.visit_route_snapshots(AttachedInterfaces::new(interfaces), |route| {
        writer.bytes(route.destination.as_bytes());
        writer.u8(route.hops);
        match route.via {
            NextHop::Direct => writer.u8(0),
            NextHop::Via(identity) => {
                writer.u8(1);
                writer.bytes(identity.as_bytes());
            }
        }
        writer.bytes(route.interface.as_bytes());
        writer.u64(route.learned_at.0);
        writer.u64(route.last_route_activity_at.0);
        writer.u64(route.expires_at.0);
        route_count += 1;
    });
    writer.replace_u64(route_count_offset, route_count);

    let mut destination_identity_count = 0u64;
    for identity in engine.destination_identities() {
        writer.bytes(identity.destination.as_bytes());
        writer.bytes(identity.identity.as_bytes());
        destination_identity_count += 1;
    }
    writer.replace_u64(
        destination_identity_count_offset,
        destination_identity_count,
    );
    writer.finish()
}

struct PackedSnapshotWriter {
    bytes: Vec<u8>,
}

impl PackedSnapshotWriter {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn usize(&mut self, value: usize) {
        self.u64(value as u64);
    }

    fn optional_u32(&mut self, value: Option<usize>) {
        match value {
            Some(value) => {
                self.u8(1);
                self.u32(value as u32);
            }
            None => self.u8(0),
        }
    }

    fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    fn reserve_u64(&mut self) -> usize {
        let offset = self.bytes.len();
        self.u64(0);
        offset
    }

    fn replace_u64(&mut self, offset: usize, value: u64) {
        self.bytes[offset..offset + size_of::<u64>()].copy_from_slice(&value.to_le_bytes());
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
