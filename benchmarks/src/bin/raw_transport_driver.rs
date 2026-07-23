use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use benchmarks::{ScenarioManifest, SizeSequence};
use personal_rns::crypto::{Ed25519PublicKey, X25519PublicKey};
use personal_rns::engine::InstantMillis;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
#[cfg(test)]
use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;
use personal_rns::interfaces::rns_serial_framing::{encode, max_encoded_len, RnsSerialScanner};
use personal_rns::interfaces::{FrameSink, FrameSinkError};
use personal_rns::routing::announce::{
    expand_name, write_announce_wire_packet, Announce, AnnounceEntropy, AnnounceId,
};
use personal_rns::routing::dedup::PacketHash;
use personal_rns::routing::links::data::write_link_raw_packet;
use personal_rns::routing::links::handshake::{
    parse_link_request, validate_link_proof, write_link_proof, write_link_request,
};
use personal_rns::routing::links::{LinkId, LinkMode, MAX_LINK_MTU};
use personal_rns::wire::{
    ContextFlag, DestinationHash, DestinationType, IfacFlag, PacketType, PropagationType,
    TransportId, WireContext, WirePacketHeader, BROADCAST_MTU, HEADER_MAX_LEN, HEADER_MIN_LEN,
    IFAC_MIN_LEN,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify, Semaphore};

#[cfg(test)]
mod allocation_gate {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    }

    pub struct TrackingAllocator;

    fn record_allocation() {
        ENABLED.with(|enabled| {
            if enabled.get() {
                ALLOCATIONS.with(|allocations| allocations.set(allocations.get() + 1));
            }
        });
    }

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: this allocator transparently delegates to the process system allocator.
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                record_allocation();
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            // SAFETY: this allocator transparently delegates to the process system allocator.
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() {
                record_allocation();
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            // SAFETY: pointer and layout came from the delegated system allocator.
            unsafe { System.dealloc(pointer, layout) };
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            // SAFETY: pointer and layout came from the delegated system allocator.
            let pointer = unsafe { System.realloc(pointer, layout, size) };
            if !pointer.is_null() {
                record_allocation();
            }
            pointer
        }
    }

    struct DisableOnDrop;

    impl Drop for DisableOnDrop {
        fn drop(&mut self) {
            ENABLED.with(|enabled| enabled.set(false));
        }
    }

    pub fn count(run: impl FnOnce()) -> u64 {
        ALLOCATIONS.with(|allocations| allocations.set(0));
        ENABLED.with(|enabled| enabled.set(true));
        let disable = DisableOnDrop;
        run();
        drop(disable);
        ALLOCATIONS.with(Cell::get)
    }
}

#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: allocation_gate::TrackingAllocator = allocation_gate::TrackingAllocator;

const DRIVER_SLUG: &str = "benchmark-wire-driver";
const DATA_MAGIC: &[u8; 8] = b"PRNSRAW1";
const PROOF_MAGIC: &[u8; 8] = b"PRNSPRF1";
const RESOURCE_MAGIC: &[u8; 8] = b"PRNSRES1";
const FRAME_CAP: usize = MAX_LINK_MTU;
const READ_CHUNK: usize = 16 * 1024;
const WRITER_QUEUE: usize = 1024;
const WRITE_BATCH_BYTES: usize = 64 * 1024;
const CALIBRATION_SECONDS: u64 = 2;
const SMOKE_CALIBRATION_MILLIS: u64 = 100;

#[derive(Clone, Copy)]
struct Direction {
    id: u8,
    seed_xor: u64,
}

const A_TO_B: Direction = Direction {
    id: 0,
    seed_xor: 0xA5A5_A5A5_A5A5_A5A5,
};
const B_TO_A: Direction = Direction {
    id: 1,
    seed_xor: 0x5A5A_5A5A_5A5A_5A5A,
};

struct FramedReader<R> {
    inner: R,
    scanner: RnsSerialScanner,
    frame: CappedFrame,
    read_buf: [u8; READ_CHUNK],
    read_len: usize,
    read_offset: usize,
    wire_bytes: u64,
}

struct CappedFrame {
    bytes: Vec<u8>,
}

impl CappedFrame {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(FRAME_CAP),
        }
    }
}

impl FrameSink for CappedFrame {
    fn clear(&mut self) {
        self.bytes.clear();
    }

    fn frame_len(&self) -> usize {
        self.bytes.len()
    }

    fn free_capacity(&self) -> usize {
        FRAME_CAP.saturating_sub(self.bytes.len())
    }

    fn push(&mut self, byte: u8) -> Result<(), FrameSinkError> {
        if self.bytes.len() == FRAME_CAP {
            return Err(FrameSinkError::Full);
        }
        self.bytes.push(byte);
        Ok(())
    }

    fn extend_from_slice(&mut self, run: &[u8]) -> Result<(), FrameSinkError> {
        if run.len() > self.free_capacity() {
            return Err(FrameSinkError::Full);
        }
        self.bytes.extend_from_slice(run);
        Ok(())
    }
}

impl<R: AsyncRead + Unpin> FramedReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            scanner: RnsSerialScanner::new(),
            frame: CappedFrame::new(),
            read_buf: [0; READ_CHUNK],
            read_len: 0,
            read_offset: 0,
            wire_bytes: 0,
        }
    }

    fn reset_wire_bytes(&mut self) {
        self.wire_bytes = 0;
    }

    fn wire_bytes(&self) -> u64 {
        self.wire_bytes
    }

    async fn next_into(&mut self, output: &mut Vec<u8>) -> io::Result<()> {
        loop {
            if self.read_offset < self.read_len {
                match self.scanner.next_frame_into(
                    &self.read_buf[..self.read_len],
                    &mut self.read_offset,
                    &mut self.frame,
                ) {
                    Ok(Some(0)) => {
                        self.frame.clear();
                        continue;
                    }
                    Ok(Some(_)) => {
                        output.clear();
                        std::mem::swap(output, &mut self.frame.bytes);
                        self.frame.bytes.clear();
                        return Ok(());
                    }
                    Ok(None) => {}
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "relay emitted an oversized HDLC frame",
                        ));
                    }
                }
            }
            let read = self.inner.read(&mut self.read_buf).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "relay TCP stream closed",
                ));
            }
            self.read_len = read;
            self.read_offset = 0;
            self.wire_bytes += read as u64;
        }
    }
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &[u8]) -> io::Result<usize> {
    let mut encoded = vec![0u8; max_encoded_len(frame.len())];
    let len = encode(frame, &mut encoded)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "HDLC encode failed"))?;
    writer.write_all(&encoded[..len]).await?;
    Ok(len)
}

fn make_announce(side: u8) -> (DestinationHash, Vec<u8>) {
    let secret = [side; personal_rns::identity::IDENTITY_SECRET_KEY_LEN];
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
    let aspect = if side == 0x31 {
        "raw-transport-a"
    } else {
        "raw-transport-b"
    };
    let dotted = expand_name("bench", &[aspect]).expect("valid benchmark destination name");
    let announce = Announce::build_signed(
        &signer,
        dotted,
        AnnounceId::mint(
            AnnounceEntropy::new([side.wrapping_add(1); AnnounceEntropy::LEN]),
            InstantMillis(1_000 + u64::from(side)),
        ),
        None,
        b"",
    )
    .expect("builds deterministic benchmark announce");
    let destination = announce.destination;
    let mut wire = [0u8; BROADCAST_MTU];
    let len = write_announce_wire_packet(&announce, 0, &mut wire)
        .expect("announce fits the broadcast MTU");
    (destination, wire[..len].to_vec())
}

fn write_payload(payload: &mut [u8], direction: Direction, sequence: u64, seed: u64) {
    let len = payload.len();
    assert!(len >= 17, "raw transport payload carries its identity");
    payload[..8].copy_from_slice(DATA_MAGIC);
    payload[8] = direction.id;
    payload[9..17].copy_from_slice(&sequence.to_be_bytes());
    let mut state = seed ^ direction.seed_xor;
    for chunk in payload[17..].chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut word = state;
        word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        word ^= word >> 31;
        chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
    }
}

fn payload_for(direction: Direction, sequence: u64, len: usize, seed: u64) -> Vec<u8> {
    let mut payload = vec![0u8; len];
    write_payload(&mut payload, direction, sequence, seed);
    payload
}

fn parse_data_payload(payload: &[u8], seed: u64) -> Option<(Direction, u64)> {
    if payload.len() < 17 || &payload[..8] != DATA_MAGIC {
        return None;
    }
    let direction = match payload[8] {
        0 => A_TO_B,
        1 => B_TO_A,
        _ => return None,
    };
    let sequence = u64::from_be_bytes(payload[9..17].try_into().ok()?);
    let mut state = seed ^ direction.seed_xor;
    let valid_tail = payload[17..].chunks(8).all(|chunk| {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut word = state;
        word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        word ^= word >> 31;
        chunk == &word.to_le_bytes()[..chunk.len()]
    });
    valid_tail.then_some((direction, sequence))
}

fn payload_template(direction: Direction, len: usize, seed: u64) -> Vec<u8> {
    payload_for(direction, 0, len, seed)
}

fn parse_data_payload_against(
    payload: &[u8],
    expected_template: &[u8],
) -> Option<(Direction, u64)> {
    if payload.len() < 17
        || payload.len() > expected_template.len()
        || payload[..9] != expected_template[..9]
        || payload[17..] != expected_template[17..payload.len()]
    {
        return None;
    }
    let direction = match payload[8] {
        0 => A_TO_B,
        1 => B_TO_A,
        _ => return None,
    };
    Some((
        direction,
        u64::from_be_bytes(payload[9..17].try_into().ok()?),
    ))
}

fn resource_payload_template(direction: Direction, len: usize, seed: u64) -> Vec<u8> {
    assert!(
        len >= 17,
        "transported resource payload carries its identity"
    );
    let mut payload = vec![0u8; len];
    payload[..8].copy_from_slice(RESOURCE_MAGIC);
    payload[8] = direction.id;
    let mut state = seed ^ direction.seed_xor;
    for chunk in payload[17..].chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut word = state;
        word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        word ^= word >> 31;
        chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
    }
    payload
}

fn resource_frame_template(
    link_id: LinkId,
    direction: Direction,
    payload_len: usize,
    seed: u64,
) -> Vec<u8> {
    let payload = resource_payload_template(direction, payload_len, seed);
    let mut frame = vec![0u8; payload_len + HEADER_MIN_LEN + IFAC_MIN_LEN];
    let written = write_link_raw_packet(
        &link_id,
        PacketType::Data,
        WireContext::Resource,
        frame.len(),
        &payload,
        &mut frame,
    )
    .expect("effective-MTU resource part fits exactly");
    frame.truncate(written);
    frame
}

fn prepare_resource_frame(frame: &mut Vec<u8>, payload_len: usize, sequence: u64) {
    let payload_offset = frame.len() - payload_len;
    frame[payload_offset + 9..payload_offset + 17].copy_from_slice(&sequence.to_be_bytes());
}

fn resource_frame(template: &[u8], payload_len: usize, sequence: u64) -> Vec<u8> {
    let mut frame = template.to_vec();
    prepare_resource_frame(&mut frame, payload_len, sequence);
    frame
}

fn parse_resource_payload(payload: &[u8], expected_template: &[u8]) -> Option<(Direction, u64)> {
    if payload.len() < 17
        || payload.len() != expected_template.len()
        || &payload[..8] != RESOURCE_MAGIC
        || payload[..9] != expected_template[..9]
        || payload[17..] != expected_template[17..]
    {
        return None;
    }
    let direction = match payload[8] {
        0 => A_TO_B,
        1 => B_TO_A,
        _ => return None,
    };
    Some((
        direction,
        u64::from_be_bytes(payload[9..17].try_into().ok()?),
    ))
}

fn prepare_data_frame(
    frame: &mut Vec<u8>,
    destination: DestinationHash,
    relay: TransportId,
    direction: Direction,
    sequence: u64,
    payload_len: usize,
    seed: u64,
) {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Transport,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: Some(relay),
        address: destination.to_address(),
        context: WireContext::None,
    };
    frame.resize(HEADER_MAX_LEN + payload_len, 0);
    let header_len = header.write(frame).expect("data header fits");
    write_payload(
        &mut frame[header_len..header_len + payload_len],
        direction,
        sequence,
        seed,
    );
    frame.truncate(header_len + payload_len);
}

fn prepare_data_frame_from_template(
    frame: &mut Vec<u8>,
    destination: DestinationHash,
    relay: TransportId,
    sequence: u64,
    payload_len: usize,
    template: &[u8],
) {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Transport,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: Some(relay),
        address: destination.to_address(),
        context: WireContext::None,
    };
    frame.resize(HEADER_MAX_LEN + payload_len, 0);
    let header_len = header.write(frame).expect("data header fits");
    let payload = &mut frame[header_len..header_len + payload_len];
    payload.copy_from_slice(&template[..payload_len]);
    payload[9..17].copy_from_slice(&sequence.to_be_bytes());
    frame.truncate(header_len + payload_len);
}

fn data_frame(
    destination: DestinationHash,
    relay: TransportId,
    direction: Direction,
    sequence: u64,
    payload_len: usize,
    seed: u64,
) -> Vec<u8> {
    let mut frame = Vec::with_capacity(BROADCAST_MTU);
    prepare_data_frame(
        &mut frame,
        destination,
        relay,
        direction,
        sequence,
        payload_len,
        seed,
    );
    frame
}

fn prepare_proof_frame(
    frame: &mut Vec<u8>,
    packet_hash: PacketHash,
    direction: Direction,
    sequence: u64,
) {
    let header = WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Proof,
        hops: 0,
        transport_id: None,
        address: packet_hash.proof_destination().to_address(),
        context: WireContext::None,
    };
    frame.resize(HEADER_MIN_LEN + 49, 0);
    let header_len = header.write(frame).expect("proof header fits");
    let payload = &mut frame[header_len..header_len + 49];
    payload[..8].copy_from_slice(PROOF_MAGIC);
    payload[8] = direction.id;
    payload[9..17].copy_from_slice(&sequence.to_be_bytes());
    payload[17..].copy_from_slice(packet_hash.as_bytes());
    frame.truncate(header_len + 49);
}

fn proof_frame(packet_hash: PacketHash, direction: Direction, sequence: u64) -> Vec<u8> {
    let mut frame = Vec::with_capacity(BROADCAST_MTU);
    prepare_proof_frame(&mut frame, packet_hash, direction, sequence);
    frame
}

fn parse_proof_payload(payload: &[u8]) -> Option<(Direction, u64, [u8; 32])> {
    if payload.len() != 49 || &payload[..8] != PROOF_MAGIC {
        return None;
    }
    let direction = match payload[8] {
        0 => A_TO_B,
        1 => B_TO_A,
        _ => return None,
    };
    let sequence = u64::from_be_bytes(payload[9..17].try_into().ok()?);
    let hash = payload[17..].try_into().ok()?;
    Some((direction, sequence, hash))
}

async fn relayed_announce(
    reader: &mut FramedReader<OwnedReadHalf>,
    expected: DestinationHash,
) -> io::Result<TransportId> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut frame = Vec::with_capacity(FRAME_CAP);
        loop {
            reader.next_into(&mut frame).await?;
            let Ok((header, _)) = WirePacketHeader::parse(&frame) else {
                continue;
            };
            if header.packet_type == PacketType::Announce
                && DestinationHash::from_address(header.address) == expected
                && header.propagation == PropagationType::Transport
            {
                return header.transport_id.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "relayed announce lacks transport id",
                    )
                });
            }
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no relayed announce"))?
}

async fn next_non_announce(reader: &mut FramedReader<OwnedReadHalf>) -> io::Result<Vec<u8>> {
    let mut frame = Vec::with_capacity(FRAME_CAP);
    loop {
        reader.next_into(&mut frame).await?;
        let Ok((header, _)) = WirePacketHeader::parse(&frame) else {
            return Ok(frame);
        };
        if header.packet_type != PacketType::Announce {
            return Ok(frame);
        }
    }
}

struct WarmRoute {
    destination: DestinationHash,
    relay: TransportId,
    direction: Direction,
    seed: u64,
}

async fn warm_direction(
    writer: &mut OwnedWriteHalf,
    reader: &mut FramedReader<OwnedReadHalf>,
    return_writer: &mut OwnedWriteHalf,
    return_reader: &mut FramedReader<OwnedReadHalf>,
    route: WarmRoute,
) -> io::Result<()> {
    let WarmRoute {
        destination,
        relay,
        direction,
        seed,
    } = route;
    let source = data_frame(destination, relay, direction, 0, 60, seed);
    let expected_hash = PacketHash::of_wire_packet(&source)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "warm data hash"))?;
    write_frame(writer, &source).await?;
    let carried = tokio::time::timeout(Duration::from_secs(5), next_non_announce(reader))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "warm data did not forward"))??;
    validate_carried_data(&carried, destination, direction, 0, seed)?;
    write_frame(return_writer, &proof_frame(expected_hash, direction, 0)).await?;
    let returned = tokio::time::timeout(Duration::from_secs(5), next_non_announce(return_reader))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "warm proof did not return"))??;
    validate_returned_proof(&returned, direction, 0, expected_hash.as_bytes())?;
    Ok(())
}

async fn establish_resource_link(
    writer_a: &mut OwnedWriteHalf,
    reader_a: &mut FramedReader<OwnedReadHalf>,
    writer_b: &mut OwnedWriteHalf,
    reader_b: &mut FramedReader<OwnedReadHalf>,
    destination_b: DestinationHash,
    relay: TransportId,
    requested_mtu: usize,
) -> io::Result<(LinkId, usize)> {
    let initiator = InMemoryNodeIdentity::from_secret_key_bytes(
        &[0x31; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    );
    let responder = InMemoryNodeIdentity::from_secret_key_bytes(
        &[0x42; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    );
    let initiator_encryption: X25519PublicKey = *initiator.encryption_public_key().as_x25519();
    let initiator_signing: Ed25519PublicKey = *initiator.signing_public_key().as_ed25519();
    let mut request = vec![0u8; BROADCAST_MTU];
    let request_len = write_link_request(
        &destination_b,
        Some(relay),
        &initiator_encryption,
        &initiator_signing,
        requested_mtu,
        LinkMode::Aes256Cbc,
        &mut request,
    )
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "link request did not fit"))?;
    request.truncate(request_len);
    write_frame(writer_a, &request).await?;

    let forwarded = tokio::time::timeout(Duration::from_secs(10), next_non_announce(reader_b))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "link request did not forward"))??;
    let (forwarded_header, _) = WirePacketHeader::parse(&forwarded)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "forwarded link request header"))?;
    let parsed = parse_link_request(&forwarded)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "forwarded link request body"))?;
    let valid_request = forwarded_header.propagation == PropagationType::Broadcast
        && forwarded_header.destination_type == DestinationType::Single
        && forwarded_header.packet_type == PacketType::LinkRequest
        && forwarded_header.hops == 1
        && forwarded_header.transport_id.is_none()
        && parsed.destination == destination_b
        && parsed.signalled
        && parsed.mode == LinkMode::Aes256Cbc
        && parsed.mtu > HEADER_MIN_LEN + IFAC_MIN_LEN
        && parsed.mtu <= requested_mtu;
    if !valid_request {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "relay did not perform the exact final-hop link-request transformation",
        ));
    }

    let mut proof = vec![0u8; BROADCAST_MTU];
    let proof_len = write_link_proof(
        &parsed.link_id,
        responder.encryption_public_key().as_x25519(),
        &responder,
        parsed.mtu,
        parsed.mode,
        &mut proof,
    )
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "link proof did not fit"))?;
    proof.truncate(proof_len);
    write_frame(writer_b, &proof).await?;
    let returned = tokio::time::timeout(Duration::from_secs(10), next_non_announce(reader_a))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "link proof did not return"))??;
    let (returned_header, _) = WirePacketHeader::parse(&returned)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "returned link proof header"))?;
    let verified = validate_link_proof(&returned, responder.signing_public_key().as_ed25519())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "returned link proof signature"))?;
    let valid_proof = returned_header.propagation == PropagationType::Broadcast
        && returned_header.destination_type == DestinationType::Link
        && returned_header.packet_type == PacketType::Proof
        && returned_header.hops == 1
        && returned_header.transport_id.is_none()
        && returned_header.context == WireContext::LinkRequestProof
        && verified.link_id == parsed.link_id
        && verified.mtu == parsed.mtu
        && verified.mode == parsed.mode;
    if !valid_proof {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "relay did not return the exact transported-link proof",
        ));
    }
    Ok((parsed.link_id, parsed.mtu))
}

fn validate_resource_frame(
    frame: &[u8],
    link_id: LinkId,
    direction: Direction,
    sequence: u64,
    expected_payload: &[u8],
) -> io::Result<()> {
    let (header, payload) = WirePacketHeader::parse(frame)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "resource frame header"))?;
    let valid_header = header.ifac_flag == IfacFlag::Open
        && header.context_flag == ContextFlag::Unset
        && header.propagation == PropagationType::Broadcast
        && header.destination_type == DestinationType::Link
        && header.packet_type == PacketType::Data
        && header.hops == 1
        && header.transport_id.is_none()
        && LinkId::from_address(header.address) == link_id
        && header.context == WireContext::Resource;
    let valid_payload = parse_resource_payload(payload, expected_payload).is_some_and(
        |(observed_direction, observed_sequence)| {
            observed_direction.id == direction.id && observed_sequence == sequence
        },
    );
    if valid_header && valid_payload {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "relay changed or misrouted a transported resource part",
        ))
    }
}

async fn warm_resource_link(
    writer_a: &mut OwnedWriteHalf,
    reader_a: &mut FramedReader<OwnedReadHalf>,
    writer_b: &mut OwnedWriteHalf,
    reader_b: &mut FramedReader<OwnedReadHalf>,
    link_id: LinkId,
    payload_len: usize,
    seed: u64,
) -> io::Result<()> {
    let template_a = resource_frame_template(link_id, A_TO_B, payload_len, seed);
    let template_b = resource_frame_template(link_id, B_TO_A, payload_len, seed);
    let (_, expected_a) = WirePacketHeader::parse(&template_a)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "A resource template"))?;
    let (_, expected_b) = WirePacketHeader::parse(&template_b)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "B resource template"))?;

    write_frame(writer_a, &resource_frame(&template_a, payload_len, 0)).await?;
    let carried_a = tokio::time::timeout(Duration::from_secs(10), next_non_announce(reader_b))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "A resource warm-up"))??;
    validate_resource_frame(&carried_a, link_id, A_TO_B, 0, expected_a)?;

    write_frame(writer_b, &resource_frame(&template_b, payload_len, 0)).await?;
    let carried_b = tokio::time::timeout(Duration::from_secs(10), next_non_announce(reader_a))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "B resource warm-up"))??;
    validate_resource_frame(&carried_b, link_id, B_TO_A, 0, expected_b)?;
    Ok(())
}

fn validate_carried_data(
    frame: &[u8],
    destination: DestinationHash,
    direction: Direction,
    sequence: u64,
    seed: u64,
) -> io::Result<()> {
    let (header, payload) = WirePacketHeader::parse(frame)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid carried data header"))?;
    let valid_header = header.ifac_flag == IfacFlag::Open
        && header.context_flag == ContextFlag::Unset
        && header.propagation == PropagationType::Broadcast
        && header.destination_type == DestinationType::Single
        && header.packet_type == PacketType::Data
        && header.hops == 1
        && header.transport_id.is_none()
        && DestinationHash::from_address(header.address) == destination
        && header.context == WireContext::None;
    let valid_payload = parse_data_payload(payload, seed)
        .is_some_and(|(d, s)| d.id == direction.id && s == sequence);
    if valid_header && valid_payload {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "relay changed data outside the final-hop header rewrite",
        ))
    }
}

fn validate_returned_proof(
    frame: &[u8],
    direction: Direction,
    sequence: u64,
    expected_hash: &[u8; 32],
) -> io::Result<()> {
    let (header, payload) = WirePacketHeader::parse(frame)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid returned proof header"))?;
    let proof = parse_proof_payload(payload);
    let valid = header.packet_type == PacketType::Proof
        && header.propagation == PropagationType::Broadcast
        && header.transport_id.is_none()
        && header.hops == 1
        && proof.is_some_and(|(d, s, hash)| {
            d.id == direction.id && s == sequence && hash == *expected_hash
        });
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "relay changed or misrouted the returned proof",
        ))
    }
}

#[derive(Clone, Copy, Default)]
struct OutstandingSlot {
    sequence: u64,
    hash: [u8; 32],
    occupied: bool,
}

struct Outstanding {
    slots: Mutex<Vec<OutstandingSlot>>,
    count: AtomicU64,
    slot_errors: AtomicU64,
}

impl Outstanding {
    fn new(capacity: usize) -> Self {
        Self {
            slots: Mutex::new(vec![OutstandingSlot::default(); capacity]),
            count: AtomicU64::new(0),
            slot_errors: AtomicU64::new(0),
        }
    }

    fn insert(&self, sequence: u64, hash: [u8; 32]) -> bool {
        let mut slots = self.slots.lock().expect("outstanding ring");
        let index = sequence as usize % slots.len();
        let slot = &mut slots[index];
        if slot.occupied {
            self.slot_errors.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        *slot = OutstandingSlot {
            sequence,
            hash,
            occupied: true,
        };
        self.count.fetch_add(1, Ordering::Release);
        true
    }

    fn remove(&self, sequence: u64) -> Option<[u8; 32]> {
        let mut slots = self.slots.lock().expect("outstanding ring");
        let index = sequence as usize % slots.len();
        let slot = &mut slots[index];
        if !slot.occupied || slot.sequence != sequence {
            return None;
        }
        slot.occupied = false;
        self.count.fetch_sub(1, Ordering::AcqRel);
        Some(slot.hash)
    }

    fn get(&self, sequence: u64) -> Option<[u8; 32]> {
        let slots = self.slots.lock().expect("outstanding ring");
        let slot = &slots[sequence as usize % slots.len()];
        (slot.occupied && slot.sequence == sequence).then_some(slot.hash)
    }

    fn len(&self) -> usize {
        self.count.load(Ordering::Acquire) as usize
    }

    fn is_empty(&self) -> bool {
        self.count.load(Ordering::Acquire) == 0
    }

    fn slot_errors(&self) -> u64 {
        self.slot_errors.load(Ordering::Relaxed)
    }
}

struct SharedDirection {
    sent: AtomicU64,
    sent_payload_bytes: AtomicU64,
    generator_done: AtomicBool,
    outstanding: Outstanding,
    buffer_pool_misses: AtomicU64,
    changed: Notify,
}

impl SharedDirection {
    fn new(window: usize) -> Self {
        Self {
            sent: AtomicU64::new(0),
            sent_payload_bytes: AtomicU64::new(0),
            generator_done: AtomicBool::new(false),
            outstanding: Outstanding::new(window),
            buffer_pool_misses: AtomicU64::new(0),
            changed: Notify::new(),
        }
    }
}

#[derive(Default)]
struct WriterStats {
    frames: AtomicU64,
    framed_bytes: AtomicU64,
    errors: AtomicU64,
}

#[derive(Clone, Copy)]
enum RecycleBuffer {
    Data,
    Proof,
}

struct OutboundFrame {
    bytes: Vec<u8>,
    recycle: RecycleBuffer,
}

fn buffer_pool(
    count: usize,
    mut make: impl FnMut() -> Vec<u8>,
) -> (mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
    let (send, receive) = mpsc::channel(count);
    for _ in 0..count {
        send.try_send(make()).expect("new buffer pool has capacity");
    }
    (send, receive)
}

fn writer_buffer(max_frame_len: usize) -> Vec<u8> {
    let frame_capacity = max_encoded_len(max_frame_len);
    let capacity = if max_frame_len <= BROADCAST_MTU {
        frame_capacity.max(WRITE_BATCH_BYTES)
    } else {
        frame_capacity
    };
    Vec::with_capacity(capacity)
}

async fn socket_writer<W>(
    mut writer: W,
    mut receive: mpsc::Receiver<OutboundFrame>,
    stats: Arc<WriterStats>,
    shutdown_when_done: bool,
    mut encoded: Vec<u8>,
    data_pool: Option<mpsc::Sender<Vec<u8>>>,
    proof_pool: Option<mpsc::Sender<Vec<u8>>>,
) where
    W: AsyncWrite + Unpin,
{
    let mut pending = None;
    loop {
        let mut frame = if let Some(frame) = pending.take() {
            frame
        } else {
            let Some(frame) = receive.recv().await else {
                break;
            };
            frame
        };
        encoded.clear();
        let mut batch_frames = 0u64;
        let mut encode_failed = false;
        loop {
            let reserved = max_encoded_len(frame.bytes.len());
            if batch_frames > 0 && encoded.len() + reserved > encoded.capacity() {
                pending = Some(frame);
                break;
            }
            if encoded.len() + reserved > encoded.capacity() {
                stats.errors.fetch_add(1, Ordering::Relaxed);
                encode_failed = true;
            } else {
                let offset = encoded.len();
                encoded.resize(offset + reserved, 0);
                match encode(&frame.bytes, &mut encoded[offset..]) {
                    Ok(encoded_len) => {
                        encoded.truncate(offset + encoded_len);
                        batch_frames += 1;
                    }
                    Err(_) => {
                        encoded.truncate(offset);
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                        encode_failed = true;
                    }
                }
            }
            match frame.recycle {
                RecycleBuffer::Data => {
                    if let Some(pool) = &data_pool {
                        let _ = pool.send(frame.bytes).await;
                    }
                }
                RecycleBuffer::Proof => {
                    if let Some(pool) = &proof_pool {
                        let _ = pool.send(frame.bytes).await;
                    }
                }
            }
            if encode_failed {
                break;
            }
            match receive.try_recv() {
                Ok(next) => frame = next,
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }
        if encode_failed {
            break;
        }
        if let Err(_) = writer.write_all(&encoded).await {
            stats.errors.fetch_add(1, Ordering::Relaxed);
            break;
        }
        stats.frames.fetch_add(batch_frames, Ordering::Relaxed);
        stats
            .framed_bytes
            .fetch_add(encoded.len() as u64, Ordering::Relaxed);
    }
    if shutdown_when_done {
        let _ = writer.shutdown().await;
    }
}

struct GeneratorContext {
    destination: DestinationHash,
    relay: TransportId,
    profile: benchmarks::WorkloadProfile,
    payload_template: Arc<Vec<u8>>,
    deadline: tokio::time::Instant,
}

async fn generate_direction(
    send: mpsc::Sender<OutboundFrame>,
    mut buffers: mpsc::Receiver<Vec<u8>>,
    credits: Arc<Semaphore>,
    shared: Arc<SharedDirection>,
    context: GeneratorContext,
) {
    let GeneratorContext {
        destination,
        relay,
        profile,
        payload_template,
        deadline,
    } = context;
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let mut sequence = 1u64;
    loop {
        let permit = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            permit = credits.acquire() => {
                match permit {
                    Ok(permit) => permit,
                    Err(_) => break,
                }
            }
        };
        permit.forget();
        if tokio::time::Instant::now() >= deadline {
            credits.add_permits(1);
            break;
        }
        let mut frame = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                credits.add_permits(1);
                break;
            }
            frame = buffers.recv() => {
                match frame {
                    Some(frame) => frame,
                    None => {
                        shared.buffer_pool_misses.fetch_add(1, Ordering::Relaxed);
                        credits.add_permits(1);
                        break;
                    }
                }
            }
        };
        let len = sizes.next_len();
        prepare_data_frame_from_template(
            &mut frame,
            destination,
            relay,
            sequence,
            len,
            &payload_template,
        );
        let hash = PacketHash::of_wire_packet(&frame).expect("generated data hashes");
        if !shared.outstanding.insert(sequence, *hash.as_bytes()) {
            shared.buffer_pool_misses.fetch_add(1, Ordering::Relaxed);
            credits.add_permits(1);
            break;
        }
        if send
            .send(OutboundFrame {
                bytes: frame,
                recycle: RecycleBuffer::Data,
            })
            .await
            .is_err()
        {
            shared.outstanding.remove(sequence);
            credits.add_permits(1);
            break;
        }
        shared.sent.fetch_add(1, Ordering::Release);
        shared
            .sent_payload_bytes
            .fetch_add(len as u64, Ordering::Relaxed);
        sequence += 1;
    }
    shared.generator_done.store(true, Ordering::Release);
    shared.changed.notify_waiters();
}

#[derive(Default)]
struct ReaderStats {
    carried_data: u64,
    carried_payload_bytes: u64,
    returned_proofs: u64,
    egress_wire_bytes: u64,
    maintenance_announces: u64,
    duplicates: u64,
    corrupt: u64,
    reordered: u64,
    unexpected: u64,
    drain_timeouts: u64,
}

struct ReaderContext {
    side_send: mpsc::Sender<OutboundFrame>,
    proof_buffers: mpsc::Receiver<Vec<u8>>,
    incoming_destination: DestinationHash,
    incoming_direction: Direction,
    incoming: Arc<SharedDirection>,
    local_direction: Direction,
    local: Arc<SharedDirection>,
    local_credits: Arc<Semaphore>,
    incoming_payload_template: Arc<Vec<u8>>,
    drain_timeout: Duration,
}

async fn consume_side(
    mut reader: FramedReader<OwnedReadHalf>,
    mut context: ReaderContext,
    mut frame: Vec<u8>,
) -> ReaderStats {
    let mut stats = ReaderStats::default();
    let mut next_incoming = 1u64;
    let mut drain_started = None;
    loop {
        let incoming_changed = context.incoming.changed.notified();
        let local_changed = context.local.changed.notified();
        tokio::pin!(incoming_changed);
        tokio::pin!(local_changed);
        incoming_changed.as_mut().enable();
        local_changed.as_mut().enable();
        let incoming_empty = context.incoming.outstanding.is_empty();
        let local_empty = context.local.outstanding.is_empty();
        let complete = context.incoming.generator_done.load(Ordering::Acquire)
            && context.local.generator_done.load(Ordering::Acquire)
            && stats.carried_data == context.incoming.sent.load(Ordering::Acquire)
            && stats.returned_proofs == context.local.sent.load(Ordering::Acquire)
            && incoming_empty
            && local_empty;
        if complete {
            break;
        }
        if context.incoming.generator_done.load(Ordering::Acquire)
            && context.local.generator_done.load(Ordering::Acquire)
            && drain_started.is_none()
        {
            drain_started = Some(Instant::now());
        }
        let drain_deadline = drain_started.map(|started| started + context.drain_timeout);
        enum Wake {
            Frame(io::Result<()>),
            StateChanged,
            DrainTimedOut,
        }
        let wake = match drain_deadline {
            Some(deadline) => tokio::select! {
                result = reader.next_into(&mut frame) => Wake::Frame(result),
                () = &mut incoming_changed => Wake::StateChanged,
                () = &mut local_changed => Wake::StateChanged,
                () = tokio::time::sleep_until(deadline.into()) => Wake::DrainTimedOut,
            },
            None => tokio::select! {
                result = reader.next_into(&mut frame) => Wake::Frame(result),
                () = &mut incoming_changed => Wake::StateChanged,
                () = &mut local_changed => Wake::StateChanged,
            },
        };
        match wake {
            Wake::Frame(Ok(())) => {}
            Wake::Frame(Err(_)) => {
                stats.unexpected += 1;
                break;
            }
            Wake::StateChanged => continue,
            Wake::DrainTimedOut => {
                stats.drain_timeouts += 1;
                break;
            }
        }
        let Ok((header, payload)) = WirePacketHeader::parse(&frame) else {
            stats.corrupt += 1;
            context.local_credits.add_permits(1);
            continue;
        };
        match header.packet_type {
            PacketType::Announce => {
                stats.maintenance_announces += 1;
            }
            PacketType::Data => {
                let Some((direction, sequence)) =
                    parse_data_payload_against(payload, &context.incoming_payload_template)
                else {
                    stats.corrupt += 1;
                    continue;
                };
                let valid_header = header.ifac_flag == IfacFlag::Open
                    && header.context_flag == ContextFlag::Unset
                    && header.propagation == PropagationType::Broadcast
                    && header.destination_type == DestinationType::Single
                    && header.hops == 1
                    && header.transport_id.is_none()
                    && DestinationHash::from_address(header.address)
                        == context.incoming_destination
                    && header.context == WireContext::None;
                if !valid_header || direction.id != context.incoming_direction.id {
                    stats.unexpected += 1;
                    continue;
                }
                if sequence < next_incoming {
                    stats.duplicates += 1;
                } else if sequence > next_incoming {
                    stats.reordered += sequence - next_incoming;
                    next_incoming = sequence + 1;
                } else {
                    next_incoming += 1;
                }
                stats.carried_data += 1;
                stats.carried_payload_bytes += payload.len() as u64;
                let Some(expected_hash) = context.incoming.outstanding.get(sequence) else {
                    stats.unexpected += 1;
                    continue;
                };
                let Some(mut proof) = context.proof_buffers.recv().await else {
                    context
                        .incoming
                        .buffer_pool_misses
                        .fetch_add(1, Ordering::Relaxed);
                    stats.unexpected += 1;
                    break;
                };
                prepare_proof_frame(
                    &mut proof,
                    PacketHash::new(expected_hash),
                    direction,
                    sequence,
                );
                if context
                    .side_send
                    .send(OutboundFrame {
                        bytes: proof,
                        recycle: RecycleBuffer::Proof,
                    })
                    .await
                    .is_err()
                {
                    stats.unexpected += 1;
                    break;
                }
            }
            PacketType::Proof => {
                let Some((direction, sequence, hash)) = parse_proof_payload(payload) else {
                    stats.corrupt += 1;
                    context.local_credits.add_permits(1);
                    continue;
                };
                let valid_header = header.propagation == PropagationType::Broadcast
                    && header.transport_id.is_none()
                    && header.hops == 1
                    && direction.id == context.local_direction.id
                    && DestinationHash::from_address(header.address)
                        == PacketHash::new(hash).proof_destination();
                let expected = context.local.outstanding.remove(sequence);
                context.local.changed.notify_waiters();
                if valid_header && expected == Some(hash) {
                    stats.returned_proofs += 1;
                } else if expected.is_none() {
                    stats.duplicates += 1;
                } else {
                    stats.corrupt += 1;
                }
                context.local_credits.add_permits(1);
            }
            PacketType::LinkRequest => {
                stats.unexpected += 1;
            }
        }
    }
    stats.egress_wire_bytes = reader.wire_bytes();
    stats
}

struct ResourceGeneratorContext {
    payload_len: usize,
    deadline: tokio::time::Instant,
}

async fn generate_resource_direction(
    send: mpsc::Sender<OutboundFrame>,
    mut buffers: mpsc::Receiver<Vec<u8>>,
    credits: Arc<Semaphore>,
    shared: Arc<SharedDirection>,
    context: ResourceGeneratorContext,
) {
    let mut sequence = 1u64;
    loop {
        let permit = tokio::select! {
            _ = tokio::time::sleep_until(context.deadline) => break,
            permit = credits.acquire() => {
                match permit {
                    Ok(permit) => permit,
                    Err(_) => break,
                }
            }
        };
        permit.forget();
        if tokio::time::Instant::now() >= context.deadline {
            credits.add_permits(1);
            break;
        }
        let mut frame = tokio::select! {
            _ = tokio::time::sleep_until(context.deadline) => {
                credits.add_permits(1);
                break;
            }
            frame = buffers.recv() => {
                match frame {
                    Some(frame) => frame,
                    None => {
                        shared.buffer_pool_misses.fetch_add(1, Ordering::Relaxed);
                        credits.add_permits(1);
                        break;
                    }
                }
            }
        };
        prepare_resource_frame(&mut frame, context.payload_len, sequence);
        if !shared.outstanding.insert(sequence, [0; 32]) {
            shared.buffer_pool_misses.fetch_add(1, Ordering::Relaxed);
            credits.add_permits(1);
            break;
        }
        if send
            .send(OutboundFrame {
                bytes: frame,
                recycle: RecycleBuffer::Data,
            })
            .await
            .is_err()
        {
            shared.outstanding.remove(sequence);
            credits.add_permits(1);
            break;
        }
        shared.sent.fetch_add(1, Ordering::Release);
        shared
            .sent_payload_bytes
            .fetch_add(context.payload_len as u64, Ordering::Relaxed);
        sequence += 1;
    }
    shared.generator_done.store(true, Ordering::Release);
    shared.changed.notify_waiters();
}

struct ResourceReaderContext {
    link_id: LinkId,
    incoming_direction: Direction,
    incoming_payload: Vec<u8>,
    incoming: Arc<SharedDirection>,
    local: Arc<SharedDirection>,
    incoming_credits: Arc<Semaphore>,
    drain_timeout: Duration,
}

async fn consume_resource_side(
    mut reader: FramedReader<OwnedReadHalf>,
    context: ResourceReaderContext,
    mut frame: Vec<u8>,
) -> ReaderStats {
    let mut stats = ReaderStats::default();
    let mut next_incoming = 1u64;
    let mut drain_started = None;
    loop {
        let incoming_changed = context.incoming.changed.notified();
        let local_changed = context.local.changed.notified();
        tokio::pin!(incoming_changed);
        tokio::pin!(local_changed);
        incoming_changed.as_mut().enable();
        local_changed.as_mut().enable();
        let incoming_empty = context.incoming.outstanding.is_empty();
        let local_empty = context.local.outstanding.is_empty();
        let complete = context.incoming.generator_done.load(Ordering::Acquire)
            && context.local.generator_done.load(Ordering::Acquire)
            && stats.carried_data == context.incoming.sent.load(Ordering::Acquire)
            && incoming_empty
            && local_empty;
        if complete {
            break;
        }
        if context.incoming.generator_done.load(Ordering::Acquire)
            && context.local.generator_done.load(Ordering::Acquire)
            && drain_started.is_none()
        {
            drain_started = Some(Instant::now());
        }
        let drain_deadline = drain_started.map(|started| started + context.drain_timeout);
        enum Wake {
            Frame(io::Result<()>),
            StateChanged,
            DrainTimedOut,
        }
        let wake = match drain_deadline {
            Some(deadline) => tokio::select! {
                result = reader.next_into(&mut frame) => Wake::Frame(result),
                () = &mut incoming_changed => Wake::StateChanged,
                () = &mut local_changed => Wake::StateChanged,
                () = tokio::time::sleep_until(deadline.into()) => Wake::DrainTimedOut,
            },
            None => tokio::select! {
                result = reader.next_into(&mut frame) => Wake::Frame(result),
                () = &mut incoming_changed => Wake::StateChanged,
                () = &mut local_changed => Wake::StateChanged,
            },
        };
        match wake {
            Wake::Frame(Ok(())) => {}
            Wake::Frame(Err(_)) => {
                stats.unexpected += 1;
                break;
            }
            Wake::StateChanged => continue,
            Wake::DrainTimedOut => {
                stats.drain_timeouts += 1;
                break;
            }
        }
        let Ok((header, payload)) = WirePacketHeader::parse(&frame) else {
            stats.corrupt += 1;
            continue;
        };
        if header.packet_type == PacketType::Announce {
            stats.maintenance_announces += 1;
            continue;
        }
        if header.packet_type != PacketType::Data {
            stats.unexpected += 1;
            continue;
        }
        let Some((direction, sequence)) =
            parse_resource_payload(payload, &context.incoming_payload)
        else {
            stats.corrupt += 1;
            continue;
        };
        let valid_header = header.ifac_flag == IfacFlag::Open
            && header.context_flag == ContextFlag::Unset
            && header.propagation == PropagationType::Broadcast
            && header.destination_type == DestinationType::Link
            && header.hops == 1
            && header.transport_id.is_none()
            && LinkId::from_address(header.address) == context.link_id
            && header.context == WireContext::Resource;
        if !valid_header || direction.id != context.incoming_direction.id {
            stats.unexpected += 1;
            continue;
        }
        let outstanding = context.incoming.outstanding.remove(sequence);
        context.incoming.changed.notify_waiters();
        if outstanding.is_none() {
            stats.duplicates += 1;
            continue;
        }
        if sequence < next_incoming {
            stats.duplicates += 1;
        } else if sequence > next_incoming {
            stats.reordered += sequence - next_incoming;
            next_incoming = sequence + 1;
        } else {
            next_incoming += 1;
        }
        stats.carried_data += 1;
        stats.carried_payload_bytes += payload.len() as u64;
        context.incoming_credits.add_permits(1);
    }
    stats.egress_wire_bytes = reader.wire_bytes();
    stats
}

struct ResourceMeasurement {
    write_a: OwnedWriteHalf,
    read_a: FramedReader<OwnedReadHalf>,
    write_b: OwnedWriteHalf,
    read_b: FramedReader<OwnedReadHalf>,
    link_id: LinkId,
    payload_len: usize,
    profile: benchmarks::WorkloadProfile,
    duration: Duration,
    harness_rates: CalibrationRates,
    harness_calibration_ms: u64,
}

async fn run_resource_measurement(measurement: ResourceMeasurement) -> io::Result<()> {
    let ResourceMeasurement {
        write_a,
        mut read_a,
        write_b,
        mut read_b,
        link_id,
        payload_len,
        profile,
        duration,
        harness_rates,
        harness_calibration_ms,
    } = measurement;
    let harness_rate = harness_rates.limiting();
    let template_a = resource_frame_template(link_id, A_TO_B, payload_len, profile.size_seed);
    let template_b = resource_frame_template(link_id, B_TO_A, payload_len, profile.size_seed);
    let (_, expected_a) = WirePacketHeader::parse(&template_a)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "A resource template"))?;
    let (_, expected_b) = WirePacketHeader::parse(&template_b)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "B resource template"))?;
    let expected_a = expected_a.to_vec();
    let expected_b = expected_b.to_vec();

    let (send_a, receive_a) = mpsc::channel(profile.window);
    let (send_b, receive_b) = mpsc::channel(profile.window);
    let (data_pool_a, data_buffers_a) = buffer_pool(profile.window, || template_a.clone());
    let (data_pool_b, data_buffers_b) = buffer_pool(profile.window, || template_b.clone());
    let encoded_a = writer_buffer(template_a.len());
    let encoded_b = writer_buffer(template_b.len());
    let read_frame_a = Vec::with_capacity(template_a.len());
    let read_frame_b = Vec::with_capacity(template_b.len());
    let writer_a_stats = Arc::new(WriterStats::default());
    let writer_b_stats = Arc::new(WriterStats::default());
    let shared_a = Arc::new(SharedDirection::new(profile.window));
    let shared_b = Arc::new(SharedDirection::new(profile.window));
    let credits_a = Arc::new(Semaphore::new(profile.window));
    let credits_b = Arc::new(Semaphore::new(profile.window));
    read_a.reset_wire_bytes();
    read_b.reset_wire_bytes();

    println!("MEASURE_READY");
    let mut command = String::new();
    std::io::stdin().read_line(&mut command)?;
    if command.trim() != "START" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected START",
        ));
    }

    let writer_a = tokio::spawn(socket_writer(
        write_a,
        receive_a,
        writer_a_stats.clone(),
        false,
        encoded_a,
        Some(data_pool_a),
        None,
    ));
    let writer_b = tokio::spawn(socket_writer(
        write_b,
        receive_b,
        writer_b_stats.clone(),
        false,
        encoded_b,
        Some(data_pool_b),
        None,
    ));
    let deadline = tokio::time::Instant::now() + duration;
    let started = Instant::now();

    let generator_a = tokio::spawn(generate_resource_direction(
        send_a.clone(),
        data_buffers_a,
        credits_a.clone(),
        shared_a.clone(),
        ResourceGeneratorContext {
            payload_len,
            deadline,
        },
    ));
    let generator_b = tokio::spawn(generate_resource_direction(
        send_b.clone(),
        data_buffers_b,
        credits_b.clone(),
        shared_b.clone(),
        ResourceGeneratorContext {
            payload_len,
            deadline,
        },
    ));
    let consumer_a = tokio::spawn(consume_resource_side(
        read_a,
        ResourceReaderContext {
            link_id,
            incoming_direction: B_TO_A,
            incoming_payload: expected_b,
            incoming: shared_b.clone(),
            local: shared_a.clone(),
            incoming_credits: credits_b.clone(),
            drain_timeout: Duration::from_millis(profile.drain_timeout_ms),
        },
        read_frame_a,
    ));
    let consumer_b = tokio::spawn(consume_resource_side(
        read_b,
        ResourceReaderContext {
            link_id,
            incoming_direction: A_TO_B,
            incoming_payload: expected_a,
            incoming: shared_a.clone(),
            local: shared_b.clone(),
            incoming_credits: credits_a.clone(),
            drain_timeout: Duration::from_millis(profile.drain_timeout_ms),
        },
        read_frame_b,
    ));
    generator_a.await.expect("A resource generator");
    generator_b.await.expect("B resource generator");
    let reader_a = consumer_a.await.expect("A resource consumer");
    let reader_b = consumer_b.await.expect("B resource consumer");
    drop(send_a);
    drop(send_b);
    writer_a.await.expect("A resource writer");
    writer_b.await.expect("B resource writer");
    let elapsed = started.elapsed();

    let sent_a = shared_a.sent.load(Ordering::Acquire);
    let sent_b = shared_b.sent.load(Ordering::Acquire);
    let sent_bytes_a = shared_a.sent_payload_bytes.load(Ordering::Relaxed);
    let sent_bytes_b = shared_b.sent_payload_bytes.load(Ordering::Relaxed);
    let carried_a = reader_b.carried_data;
    let carried_b = reader_a.carried_data;
    let carried_bytes_a = reader_b.carried_payload_bytes;
    let carried_bytes_b = reader_a.carried_payload_bytes;
    let sent = sent_a + sent_b;
    let carried = carried_a + carried_b;
    let sent_bytes = sent_bytes_a + sent_bytes_b;
    let carried_bytes = carried_bytes_a + carried_bytes_b;
    let seconds = elapsed.as_secs_f64().max(f64::EPSILON);
    let carried_rate = carried_bytes as f64 / seconds;
    let frame_rate = carried as f64 / seconds;
    let ingress_wire_bytes = writer_a_stats.framed_bytes.load(Ordering::Relaxed)
        + writer_b_stats.framed_bytes.load(Ordering::Relaxed);
    let egress_wire_bytes = reader_a.egress_wire_bytes + reader_b.egress_wire_bytes;
    let writer_errors = writer_a_stats.errors.load(Ordering::Relaxed)
        + writer_b_stats.errors.load(Ordering::Relaxed);
    let duplicates = reader_a.duplicates + reader_b.duplicates;
    let corrupt = reader_a.corrupt + reader_b.corrupt;
    let reordered = reader_a.reordered + reader_b.reordered;
    let unexpected = reader_a.unexpected + reader_b.unexpected + writer_errors;
    let drain_timeouts = reader_a.drain_timeouts + reader_b.drain_timeouts;
    let maintenance_announces = reader_a.maintenance_announces + reader_b.maintenance_announces;
    let outstanding = shared_a.outstanding.len() + shared_b.outstanding.len();
    let buffer_pool_misses = shared_a.buffer_pool_misses.load(Ordering::Relaxed)
        + shared_b.buffer_pool_misses.load(Ordering::Relaxed);
    let slot_errors = shared_a.outstanding.slot_errors() + shared_b.outstanding.slot_errors();
    let available_credits = credits_a.available_permits() + credits_b.available_permits();
    let credit_leaks = profile
        .window
        .saturating_mul(2)
        .saturating_sub(available_credits);
    let missing = sent.saturating_sub(carried);
    let timed_out_frames = if drain_timeouts > 0 {
        outstanding as u64
    } else {
        0
    };
    let harness_headroom = harness_rate >= carried_rate * 1.25;

    println!("MEASURE_DONE");
    println!(
        "RESULT build={} sent={} carried={} proofs=0 sent_a_to_b={} carried_a_to_b={} \
         sent_b_to_a={} carried_b_to_a={} sent_payload_bytes={} carried_payload_bytes={} \
         sent_payload_bytes_a_to_b={} carried_payload_bytes_a_to_b={} \
         sent_payload_bytes_b_to_a={} carried_payload_bytes_b_to_a={} elapsed_ms={} \
         carried_payload_bytes_per_sec={carried_rate:.1} forwarded_frames_per_sec={frame_rate:.1} \
         ingress_wire_bytes_per_sec={:.1} egress_wire_bytes_per_sec={:.1} \
         harness_source_payload_bytes_per_sec={:.1} \
         harness_sink_payload_bytes_per_sec={:.1} \
         harness_carried_payload_bytes_per_sec={harness_rate:.1} \
         harness_calibration_ms={harness_calibration_ms} harness_headroom={} \
         missing={} duplicates={} corrupt={} reordered={} unexpected={} timed_out_frames={} \
         drain_timeouts={} outstanding={} maintenance_announces={} negotiated_link_mtu_bytes={} \
         resource_payload_bytes_per_frame={} buffer_pool_misses={} credit_leaks={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        sent,
        carried,
        sent_a,
        carried_a,
        sent_b,
        carried_b,
        sent_bytes,
        carried_bytes,
        sent_bytes_a,
        carried_bytes_a,
        sent_bytes_b,
        carried_bytes_b,
        elapsed.as_millis(),
        ingress_wire_bytes as f64 / seconds,
        egress_wire_bytes as f64 / seconds,
        harness_rates.source,
        harness_rates.sink,
        u8::from(harness_headroom),
        missing,
        duplicates,
        corrupt,
        reordered,
        unexpected + slot_errors,
        timed_out_frames,
        drain_timeouts,
        outstanding,
        maintenance_announces,
        payload_len + HEADER_MIN_LEN + IFAC_MIN_LEN,
        payload_len,
        buffer_pool_misses,
        credit_leaks,
    );
    Ok(())
}

async fn calibration_generate(
    send: mpsc::Sender<OutboundFrame>,
    mut buffers: mpsc::Receiver<Vec<u8>>,
    profile: &benchmarks::WorkloadProfile,
    direction: Direction,
    duration: Duration,
    resource_payload_len: Option<usize>,
    raw_payload_template: Arc<Vec<u8>>,
) -> u64 {
    let destination = DestinationHash::new([direction.id.wrapping_add(1); 16]);
    let relay = TransportId::new([0x77; 16]);
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let deadline = tokio::time::Instant::now() + duration;
    let mut sequence = 1u64;
    let mut payload_bytes = 0u64;
    while tokio::time::Instant::now() < deadline {
        let Some(mut frame) = buffers.recv().await else {
            break;
        };
        let len = if let Some(payload_len) = resource_payload_len {
            prepare_resource_frame(&mut frame, payload_len, sequence);
            payload_len
        } else {
            let len = sizes.next_len();
            prepare_data_frame_from_template(
                &mut frame,
                destination,
                relay,
                sequence,
                len,
                &raw_payload_template,
            );
            let _ = PacketHash::of_wire_packet(&frame)
                .expect("calibration source hashes generated data");
            len
        };
        if send
            .send(OutboundFrame {
                bytes: frame,
                recycle: RecycleBuffer::Data,
            })
            .await
            .is_err()
        {
            break;
        }
        payload_bytes += len as u64;
        sequence += 1;
    }
    payload_bytes
}

fn calibration_return_sink(mut stream: std::net::TcpStream) -> io::Result<u64> {
    stream.set_nonblocking(false)?;
    let mut buffer = [0u8; READ_CHUNK];
    let mut wire_bytes = 0u64;
    loop {
        let received = std::io::Read::read(&mut stream, &mut buffer)?;
        if received == 0 {
            return Ok(wire_bytes);
        }
        wire_bytes += received as u64;
    }
}

fn calibration_feed(
    mut stream: std::net::TcpStream,
    corpus: Arc<CalibrationCorpus>,
    duration: Duration,
) -> io::Result<(u64, u64)> {
    stream.set_nonblocking(false)?;
    let deadline = Instant::now() + duration;
    let mut payload_bytes = 0u64;
    let mut wire_bytes = 0u64;
    while Instant::now() < deadline {
        std::io::Write::write_all(&mut stream, &corpus.encoded)?;
        payload_bytes += corpus.payload_bytes;
        wire_bytes += corpus.encoded.len() as u64;
    }
    stream.shutdown(std::net::Shutdown::Write)?;
    Ok((payload_bytes, wire_bytes))
}

struct CalibrationCorpus {
    encoded: Vec<u8>,
    payload_bytes: u64,
    hashes: Vec<[u8; 32]>,
}

fn calibration_corpus(
    profile: &benchmarks::WorkloadProfile,
    direction: Direction,
    resource: Option<(LinkId, usize)>,
) -> CalibrationCorpus {
    let count = profile.window.max(1);
    let destination = DestinationHash::new([direction.id.wrapping_add(1); 16]);
    let relay = TransportId::new([0x77; 16]);
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let resource_template = resource.map(|(link_id, payload_len)| {
        (
            resource_frame_template(link_id, direction, payload_len, profile.size_seed),
            payload_len,
        )
    });
    let raw_template = payload_template(
        direction,
        profile.payload_max.max(profile.payload_len).max(17),
        profile.size_seed,
    );
    let mut corpus = CalibrationCorpus {
        encoded: Vec::new(),
        payload_bytes: 0,
        hashes: Vec::with_capacity(count),
    };
    for sequence in 1..=count {
        let mut frame = Vec::with_capacity(FRAME_CAP);
        let payload_len = if let Some((template, payload_len)) = &resource_template {
            frame.extend_from_slice(template);
            prepare_resource_frame(&mut frame, *payload_len, sequence as u64);
            *payload_len
        } else {
            let payload_len = sizes.next_len();
            prepare_data_frame_from_template(
                &mut frame,
                destination,
                relay,
                sequence as u64,
                payload_len,
                &raw_template,
            );
            payload_len
        };
        if resource_template.is_none() {
            corpus.hashes.push(
                *PacketHash::of_wire_packet(&frame)
                    .expect("calibration corpus data hashes")
                    .as_bytes(),
            );
        }
        let offset = corpus.encoded.len();
        corpus
            .encoded
            .resize(offset + max_encoded_len(frame.len()), 0);
        let len =
            encode(&frame, &mut corpus.encoded[offset..]).expect("calibration corpus encodes");
        corpus.encoded.truncate(offset + len);
        corpus.payload_bytes += payload_len as u64;
    }
    corpus
}

async fn calibration_consume(
    read: OwnedReadHalf,
    return_send: mpsc::Sender<OutboundFrame>,
    mut proof_buffers: mpsc::Receiver<Vec<u8>>,
    direction: Direction,
    raw_payload_template: Arc<Vec<u8>>,
    resource: Option<(LinkId, Vec<u8>)>,
    expected_hashes: Arc<Vec<[u8; 32]>>,
    sequence_count: u64,
) -> io::Result<(u64, u64, Duration)> {
    let started = Instant::now();
    let mut reader = FramedReader::new(read);
    let mut frame = Vec::with_capacity(FRAME_CAP);
    let mut expected_sequence = 1u64;
    let mut payload_bytes = 0u64;
    loop {
        match reader.next_into(&mut frame).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
        let (header, payload) = WirePacketHeader::parse(&frame)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "calibration frame header"))?;
        if let Some((link_id, expected_payload)) = &resource {
            let Some((observed_direction, sequence)) =
                parse_resource_payload(payload, expected_payload)
            else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "calibration resource payload",
                ));
            };
            let valid = header.packet_type == PacketType::Data
                && header.destination_type == DestinationType::Link
                && LinkId::from_address(header.address) == *link_id
                && header.context == WireContext::Resource
                && observed_direction.id == direction.id
                && sequence == expected_sequence;
            if !valid {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "calibration resource sequence or header",
                ));
            }
        } else {
            let Some((observed_direction, sequence)) =
                parse_data_payload_against(payload, &raw_payload_template)
            else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "calibration data payload",
                ));
            };
            let valid = header.packet_type == PacketType::Data
                && header.destination_type == DestinationType::Single
                && header.propagation == PropagationType::Transport
                && header.transport_id.is_some()
                && observed_direction.id == direction.id
                && sequence == expected_sequence;
            if !valid {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "calibration data sequence or header",
                ));
            }
            let expected_hash = expected_hashes
                .get(sequence.saturating_sub(1) as usize)
                .copied()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "calibration expected hash")
                })?;
            let mut proof = proof_buffers.recv().await.ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "calibration proof pool closed")
            })?;
            prepare_proof_frame(
                &mut proof,
                PacketHash::new(expected_hash),
                direction,
                sequence,
            );
            return_send
                .send(OutboundFrame {
                    bytes: proof,
                    recycle: RecycleBuffer::Proof,
                })
                .await
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "calibration proof writer closed")
                })?;
        }
        payload_bytes += payload.len() as u64;
        expected_sequence = if expected_sequence == sequence_count {
            1
        } else {
            expected_sequence + 1
        };
    }
    Ok((payload_bytes, reader.wire_bytes(), started.elapsed()))
}

#[derive(Clone, Copy)]
struct CalibrationRates {
    source: f64,
    sink: f64,
}

impl CalibrationRates {
    fn limiting(self) -> f64 {
        self.source.min(self.sink)
    }
}

async fn calibration_connections() -> io::Result<((TcpStream, TcpStream), (TcpStream, TcpStream))> {
    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let (client_a, client_b, accepted_a, accepted_b) = tokio::join!(
        TcpStream::connect(listener_a.local_addr()?),
        TcpStream::connect(listener_b.local_addr()?),
        listener_a.accept(),
        listener_b.accept(),
    );
    let client_a = client_a?;
    let client_b = client_b?;
    client_a.set_nodelay(true)?;
    client_b.set_nodelay(true)?;
    let (server_a, _) = accepted_a?;
    let (server_b, _) = accepted_b?;
    server_a.set_nodelay(true)?;
    server_b.set_nodelay(true)?;
    Ok(((client_a, client_b), (server_a, server_b)))
}

async fn calibrate_source(
    profile: benchmarks::WorkloadProfile,
    duration: Duration,
    resource: Option<(LinkId, usize)>,
) -> io::Result<f64> {
    let ((client_a, client_b), (server_a, server_b)) = calibration_connections().await?;
    let (_, write_a) = client_a.into_split();
    let (_, write_b) = client_b.into_split();
    let server_a = server_a.into_std()?;
    let server_b = server_b.into_std()?;
    let profile_a = profile.clone();
    let profile_b = profile.clone();
    let raw_template_len = profile.payload_max.max(profile.payload_len).max(17);
    let raw_template_a = Arc::new(payload_template(
        A_TO_B,
        raw_template_len,
        profile.size_seed,
    ));
    let raw_template_b = Arc::new(payload_template(
        B_TO_A,
        raw_template_len,
        profile.size_seed,
    ));
    let queue = if resource.is_some() {
        profile.window
    } else {
        WRITER_QUEUE
    };
    let (template_a, template_b, resource_payload_len) = resource.map_or_else(
        || {
            (
                Vec::with_capacity(BROADCAST_MTU),
                Vec::with_capacity(BROADCAST_MTU),
                None,
            )
        },
        |(link_id, payload_len)| {
            let template_a =
                resource_frame_template(link_id, A_TO_B, payload_len, profile.size_seed);
            let template_b =
                resource_frame_template(link_id, B_TO_A, payload_len, profile.size_seed);
            (template_a, template_b, Some(payload_len))
        },
    );
    let (send_a, receive_a) = mpsc::channel(queue);
    let (send_b, receive_b) = mpsc::channel(queue);
    let (pool_a, buffers_a) = buffer_pool(profile.window, || {
        if resource_payload_len.is_some() {
            template_a.clone()
        } else {
            Vec::with_capacity(BROADCAST_MTU)
        }
    });
    let (pool_b, buffers_b) = buffer_pool(profile.window, || {
        if resource_payload_len.is_some() {
            template_b.clone()
        } else {
            Vec::with_capacity(BROADCAST_MTU)
        }
    });
    let writer_a_stats = Arc::new(WriterStats::default());
    let writer_b_stats = Arc::new(WriterStats::default());
    let writer_a = tokio::spawn(socket_writer(
        write_a,
        receive_a,
        writer_a_stats.clone(),
        true,
        writer_buffer(template_a.len()),
        Some(pool_a),
        None,
    ));
    let writer_b = tokio::spawn(socket_writer(
        write_b,
        receive_b,
        writer_b_stats.clone(),
        true,
        writer_buffer(template_b.len()),
        Some(pool_b),
        None,
    ));
    let sink_a = std::thread::spawn(move || calibration_return_sink(server_a));
    let sink_b = std::thread::spawn(move || calibration_return_sink(server_b));
    let generator_a = tokio::spawn(async move {
        calibration_generate(
            send_a,
            buffers_a,
            &profile_a,
            A_TO_B,
            duration,
            resource_payload_len,
            raw_template_a,
        )
        .await
    });
    let generator_b = tokio::spawn(async move {
        calibration_generate(
            send_b,
            buffers_b,
            &profile_b,
            B_TO_A,
            duration,
            resource_payload_len,
            raw_template_b,
        )
        .await
    });
    let join_error = |error| io::Error::other(format!("calibration task: {error}"));
    let (sent_a, sent_b) = tokio::join!(generator_a, generator_b);
    let sent = sent_a.map_err(join_error)? + sent_b.map_err(join_error)?;
    let (writer_a, writer_b) = tokio::join!(writer_a, writer_b);
    writer_a.map_err(join_error)?;
    writer_b.map_err(join_error)?;
    let thread_error = |_| io::Error::other("calibration peer thread panicked");
    let received_wire =
        sink_a.join().map_err(thread_error)?? + sink_b.join().map_err(thread_error)??;
    let written_wire = writer_a_stats.framed_bytes.load(Ordering::Relaxed)
        + writer_b_stats.framed_bytes.load(Ordering::Relaxed);
    let writer_errors = writer_a_stats.errors.load(Ordering::Relaxed)
        + writer_b_stats.errors.load(Ordering::Relaxed);
    if writer_errors != 0 {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            format!("calibration writers failed {writer_errors} time(s)"),
        ));
    }
    if written_wire != received_wire {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("calibration source TCP mismatch: written={written_wire} read={received_wire}"),
        ));
    }
    let seconds = duration.as_secs_f64().max(f64::EPSILON);
    Ok(sent as f64 / seconds)
}

async fn calibrate_sink(
    profile: benchmarks::WorkloadProfile,
    duration: Duration,
    resource: Option<(LinkId, usize)>,
) -> io::Result<f64> {
    let ((client_a, client_b), (server_a, server_b)) = calibration_connections().await?;
    let client_a = client_a.into_std()?;
    let client_b = client_b.into_std()?;
    let return_read_a = client_a.try_clone()?;
    let return_read_b = client_b.try_clone()?;
    let (read_a, return_write_a) = server_a.into_split();
    let (read_b, return_write_b) = server_b.into_split();
    let raw_template_len = profile.payload_max.max(profile.payload_len).max(17);
    let raw_template_a = Arc::new(payload_template(
        A_TO_B,
        raw_template_len,
        profile.size_seed,
    ));
    let raw_template_b = Arc::new(payload_template(
        B_TO_A,
        raw_template_len,
        profile.size_seed,
    ));
    let corpus_a = Arc::new(calibration_corpus(&profile, A_TO_B, resource));
    let corpus_b = Arc::new(calibration_corpus(&profile, B_TO_A, resource));
    let expected_hashes_a = Arc::new(corpus_a.hashes.clone());
    let expected_hashes_b = Arc::new(corpus_b.hashes.clone());
    let (expected_a, expected_b) = resource.map_or((None, None), |(link_id, payload_len)| {
        let template_a = resource_frame_template(link_id, A_TO_B, payload_len, profile.size_seed);
        let template_b = resource_frame_template(link_id, B_TO_A, payload_len, profile.size_seed);
        let (_, payload_a) =
            WirePacketHeader::parse(&template_a).expect("calibration A resource template");
        let (_, payload_b) =
            WirePacketHeader::parse(&template_b).expect("calibration B resource template");
        (
            Some((link_id, payload_a.to_vec())),
            Some((link_id, payload_b.to_vec())),
        )
    });
    let (return_send_a, return_receive_a) = mpsc::channel(WRITER_QUEUE);
    let (return_send_b, return_receive_b) = mpsc::channel(WRITER_QUEUE);
    let (proof_pool_a, proof_buffers_a) =
        buffer_pool(profile.window, || Vec::with_capacity(BROADCAST_MTU));
    let (proof_pool_b, proof_buffers_b) =
        buffer_pool(profile.window, || Vec::with_capacity(BROADCAST_MTU));
    let writer_a_stats = Arc::new(WriterStats::default());
    let writer_b_stats = Arc::new(WriterStats::default());
    let writer_a = tokio::spawn(socket_writer(
        return_write_a,
        return_receive_a,
        writer_a_stats.clone(),
        true,
        writer_buffer(BROADCAST_MTU),
        None,
        Some(proof_pool_a),
    ));
    let writer_b = tokio::spawn(socket_writer(
        return_write_b,
        return_receive_b,
        writer_b_stats.clone(),
        true,
        writer_buffer(BROADCAST_MTU),
        None,
        Some(proof_pool_b),
    ));
    let consumer_a = tokio::spawn(calibration_consume(
        read_a,
        return_send_a,
        proof_buffers_a,
        A_TO_B,
        raw_template_a,
        expected_a,
        expected_hashes_a,
        profile.window as u64,
    ));
    let consumer_b = tokio::spawn(calibration_consume(
        read_b,
        return_send_b,
        proof_buffers_b,
        B_TO_A,
        raw_template_b,
        expected_b,
        expected_hashes_b,
        profile.window as u64,
    ));
    let return_sink_a = std::thread::spawn(move || calibration_return_sink(return_read_a));
    let return_sink_b = std::thread::spawn(move || calibration_return_sink(return_read_b));
    let feeder_a = std::thread::spawn(move || calibration_feed(client_a, corpus_a, duration));
    let feeder_b = std::thread::spawn(move || calibration_feed(client_b, corpus_b, duration));
    let join_error = |error| io::Error::other(format!("calibration task: {error}"));
    let thread_error = |_| io::Error::other("calibration peer thread panicked");
    let (sent_payload_a, sent_wire_a) = feeder_a.join().map_err(thread_error)??;
    let (sent_payload_b, sent_wire_b) = feeder_b.join().map_err(thread_error)??;
    let sent_payload = sent_payload_a + sent_payload_b;
    let sent_wire = sent_wire_a + sent_wire_b;
    let (received_a, received_b) = tokio::join!(consumer_a, consumer_b);
    let (received_payload_a, received_wire_a, elapsed_a) = received_a.map_err(join_error)??;
    let (received_payload_b, received_wire_b, elapsed_b) = received_b.map_err(join_error)??;
    let received_payload = received_payload_a + received_payload_b;
    let received_wire = received_wire_a + received_wire_b;
    let (writer_a, writer_b) = tokio::join!(writer_a, writer_b);
    writer_a.map_err(join_error)?;
    writer_b.map_err(join_error)?;
    let returned_wire = return_sink_a.join().map_err(thread_error)??
        + return_sink_b.join().map_err(thread_error)??;
    let written_return_wire = writer_a_stats.framed_bytes.load(Ordering::Relaxed)
        + writer_b_stats.framed_bytes.load(Ordering::Relaxed);
    let writer_errors = writer_a_stats.errors.load(Ordering::Relaxed)
        + writer_b_stats.errors.load(Ordering::Relaxed);
    if writer_errors != 0 || written_return_wire != returned_wire {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "calibration return mismatch: errors={writer_errors} \
                 written={written_return_wire} read={returned_wire}"
            ),
        ));
    }
    if sent_payload != received_payload || sent_wire != received_wire {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "calibration sink mismatch: payload sent={sent_payload} received={received_payload}; \
                 TCP sent={sent_wire} received={received_wire}"
            ),
        ));
    }
    if resource.is_none() && returned_wire == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "calibration proof return path carried no bytes",
        ));
    }
    let seconds = elapsed_a.max(elapsed_b).as_secs_f64().max(f64::EPSILON);
    Ok(received_payload as f64 / seconds)
}

async fn calibrate(
    profile: benchmarks::WorkloadProfile,
    smoke: bool,
    resource: Option<(LinkId, usize)>,
) -> io::Result<CalibrationRates> {
    let duration = if smoke {
        Duration::from_millis(SMOKE_CALIBRATION_MILLIS)
    } else {
        Duration::from_secs(CALIBRATION_SECONDS)
    };
    // Run the source and sink halves independently so the synthetic peer cannot
    // consume the same loopback CPU budget as the driver component being qualified.
    let source = calibrate_source(profile.clone(), duration, resource).await?;
    let sink = calibrate_sink(profile, duration, resource).await?;
    Ok(CalibrationRates { source, sink })
}

async fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let usage =
        "usage: raw_transport_driver <manifest.json> wire-driver <side-a>><side-b> [duration-ms]";
    let manifest_path = args.next().expect(usage);
    assert_eq!(args.next().as_deref(), Some("wire-driver"), "{usage}");
    let addresses = args.next().expect(usage);
    let duration_override = args.next().map(|value| {
        value
            .parse::<u64>()
            .expect("duration override is milliseconds")
    });
    let (addr_a, addr_b) = addresses
        .split_once('>')
        .expect("wire-driver address is <side-a>><side-b>");
    let manifest: ScenarioManifest = serde_json::from_str(&std::fs::read_to_string(manifest_path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    assert!(
        manifest.name.is_transport(),
        "raw transport driver requires a transport scenario"
    );
    let duration = Duration::from_millis(duration_override.unwrap_or(manifest.profile.duration_ms));
    let smoke = std::env::var_os("BENCHMARK_SMOKE").is_some();

    let side_a = TcpStream::connect(addr_a).await?;
    let side_b = TcpStream::connect(addr_b).await?;
    side_a.set_nodelay(true)?;
    side_b.set_nodelay(true)?;
    let (read_a, mut write_a) = side_a.into_split();
    let (read_b, mut write_b) = side_b.into_split();
    let mut read_a = FramedReader::new(read_a);
    let mut read_b = FramedReader::new(read_b);
    println!("READY role=wire-driver slug={DRIVER_SLUG}");

    let harness_calibration_ms = if smoke {
        SMOKE_CALIBRATION_MILLIS
    } else {
        CALIBRATION_SECONDS * 1_000
    };

    let (destination_a, announce_a) = make_announce(0x31);
    let (destination_b, announce_b) = make_announce(0x42);
    write_frame(&mut write_a, &announce_a).await?;
    write_frame(&mut write_b, &announce_b).await?;
    let relay_from_a = relayed_announce(&mut read_a, destination_b).await?;
    let relay_from_b = relayed_announce(&mut read_b, destination_a).await?;
    if relay_from_a != relay_from_b {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "relay announced two transport identities",
        ));
    }
    let relay = relay_from_a;

    if manifest.name.is_transport_resource() {
        let (link_id, negotiated_mtu) = establish_resource_link(
            &mut write_a,
            &mut read_a,
            &mut write_b,
            &mut read_b,
            destination_b,
            relay,
            manifest.profile.transport_link_mtu,
        )
        .await?;
        let payload_len = negotiated_mtu - HEADER_MIN_LEN - IFAC_MIN_LEN;
        warm_resource_link(
            &mut write_a,
            &mut read_a,
            &mut write_b,
            &mut read_b,
            link_id,
            payload_len,
            manifest.profile.size_seed,
        )
        .await?;
        let harness_rates = calibrate(
            manifest.profile.clone(),
            smoke,
            Some((link_id, payload_len)),
        )
        .await?;
        let harness_rate = harness_rates.limiting();
        println!(
            "HARNESS source_payload_bytes_per_sec={:.1} sink_payload_bytes_per_sec={:.1} \
             carried_payload_bytes_per_sec={harness_rate:.1} calibration_ms={harness_calibration_ms}",
            harness_rates.source,
            harness_rates.sink,
        );
        return run_resource_measurement(ResourceMeasurement {
            write_a,
            read_a,
            write_b,
            read_b,
            link_id,
            payload_len,
            profile: manifest.profile,
            duration,
            harness_rates,
            harness_calibration_ms,
        })
        .await;
    }

    let harness_rates = calibrate(manifest.profile.clone(), smoke, None).await?;
    let harness_rate = harness_rates.limiting();
    println!(
        "HARNESS source_payload_bytes_per_sec={:.1} sink_payload_bytes_per_sec={:.1} \
         carried_payload_bytes_per_sec={harness_rate:.1} calibration_ms={harness_calibration_ms}",
        harness_rates.source, harness_rates.sink,
    );
    warm_direction(
        &mut write_a,
        &mut read_b,
        &mut write_b,
        &mut read_a,
        WarmRoute {
            destination: destination_b,
            relay,
            direction: A_TO_B,
            seed: manifest.profile.size_seed,
        },
    )
    .await?;
    warm_direction(
        &mut write_b,
        &mut read_a,
        &mut write_a,
        &mut read_b,
        WarmRoute {
            destination: destination_a,
            relay,
            direction: B_TO_A,
            seed: manifest.profile.size_seed,
        },
    )
    .await?;

    let window = manifest.profile.window;
    let max_payload_len = manifest
        .profile
        .payload_max
        .max(manifest.profile.payload_len);
    let payload_template_a = Arc::new(payload_template(
        A_TO_B,
        max_payload_len,
        manifest.profile.size_seed,
    ));
    let payload_template_b = Arc::new(payload_template(
        B_TO_A,
        max_payload_len,
        manifest.profile.size_seed,
    ));
    let (send_a, receive_a) = mpsc::channel(WRITER_QUEUE);
    let (send_b, receive_b) = mpsc::channel(WRITER_QUEUE);
    let (data_pool_a, data_buffers_a) = buffer_pool(window, || Vec::with_capacity(BROADCAST_MTU));
    let (data_pool_b, data_buffers_b) = buffer_pool(window, || Vec::with_capacity(BROADCAST_MTU));
    let (proof_pool_a, proof_buffers_a) = buffer_pool(window, || Vec::with_capacity(BROADCAST_MTU));
    let (proof_pool_b, proof_buffers_b) = buffer_pool(window, || Vec::with_capacity(BROADCAST_MTU));
    let encoded_a = writer_buffer(BROADCAST_MTU);
    let encoded_b = writer_buffer(BROADCAST_MTU);
    let read_frame_a = Vec::with_capacity(BROADCAST_MTU);
    let read_frame_b = Vec::with_capacity(BROADCAST_MTU);
    let writer_a_stats = Arc::new(WriterStats::default());
    let writer_b_stats = Arc::new(WriterStats::default());
    let shared_a = Arc::new(SharedDirection::new(window));
    let shared_b = Arc::new(SharedDirection::new(window));
    let credits_a = Arc::new(Semaphore::new(window));
    let credits_b = Arc::new(Semaphore::new(window));
    read_a.reset_wire_bytes();
    read_b.reset_wire_bytes();

    println!("MEASURE_READY");
    let mut command = String::new();
    std::io::stdin().read_line(&mut command)?;
    if command.trim() != "START" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected START",
        ));
    }

    let writer_a = tokio::spawn(socket_writer(
        write_a,
        receive_a,
        writer_a_stats.clone(),
        false,
        encoded_a,
        Some(data_pool_a),
        Some(proof_pool_a),
    ));
    let writer_b = tokio::spawn(socket_writer(
        write_b,
        receive_b,
        writer_b_stats.clone(),
        false,
        encoded_b,
        Some(data_pool_b),
        Some(proof_pool_b),
    ));

    let deadline = tokio::time::Instant::now() + duration;
    let started = Instant::now();

    let generator_a = tokio::spawn(generate_direction(
        send_a.clone(),
        data_buffers_a,
        credits_a.clone(),
        shared_a.clone(),
        GeneratorContext {
            destination: destination_b,
            relay,
            profile: manifest.profile.clone(),
            payload_template: payload_template_a.clone(),
            deadline,
        },
    ));
    let generator_b = tokio::spawn(generate_direction(
        send_b.clone(),
        data_buffers_b,
        credits_b.clone(),
        shared_b.clone(),
        GeneratorContext {
            destination: destination_a,
            relay,
            profile: manifest.profile.clone(),
            payload_template: payload_template_b.clone(),
            deadline,
        },
    ));
    let consumer_a = tokio::spawn(consume_side(
        read_a,
        ReaderContext {
            side_send: send_a.clone(),
            proof_buffers: proof_buffers_a,
            incoming_destination: destination_a,
            incoming_direction: B_TO_A,
            incoming: shared_b.clone(),
            local_direction: A_TO_B,
            local: shared_a.clone(),
            local_credits: credits_a.clone(),
            incoming_payload_template: payload_template_b,
            drain_timeout: Duration::from_millis(manifest.profile.drain_timeout_ms),
        },
        read_frame_a,
    ));
    let consumer_b = tokio::spawn(consume_side(
        read_b,
        ReaderContext {
            side_send: send_b.clone(),
            proof_buffers: proof_buffers_b,
            incoming_destination: destination_b,
            incoming_direction: A_TO_B,
            incoming: shared_a.clone(),
            local_direction: B_TO_A,
            local: shared_b.clone(),
            local_credits: credits_b.clone(),
            incoming_payload_template: payload_template_a,
            drain_timeout: Duration::from_millis(manifest.profile.drain_timeout_ms),
        },
        read_frame_b,
    ));
    drop(send_a);
    drop(send_b);

    generator_a.await.expect("A generator");
    generator_b.await.expect("B generator");
    let reader_a = consumer_a.await.expect("A consumer");
    let reader_b = consumer_b.await.expect("B consumer");
    writer_a.await.expect("A writer");
    writer_b.await.expect("B writer");
    let elapsed = started.elapsed();

    let sent_a = shared_a.sent.load(Ordering::Acquire);
    let sent_b = shared_b.sent.load(Ordering::Acquire);
    let sent_bytes_a = shared_a.sent_payload_bytes.load(Ordering::Relaxed);
    let sent_bytes_b = shared_b.sent_payload_bytes.load(Ordering::Relaxed);
    let carried_a = reader_b.carried_data;
    let carried_b = reader_a.carried_data;
    let carried_bytes_a = reader_b.carried_payload_bytes;
    let carried_bytes_b = reader_a.carried_payload_bytes;
    let sent = sent_a + sent_b;
    let carried = carried_a + carried_b;
    let sent_bytes = sent_bytes_a + sent_bytes_b;
    let carried_bytes = carried_bytes_a + carried_bytes_b;
    let proofs = reader_a.returned_proofs + reader_b.returned_proofs;
    let seconds = elapsed.as_secs_f64().max(f64::EPSILON);
    let carried_rate = carried_bytes as f64 / seconds;
    let frame_rate = carried as f64 / seconds;
    let ingress_wire_bytes = writer_a_stats.framed_bytes.load(Ordering::Relaxed)
        + writer_b_stats.framed_bytes.load(Ordering::Relaxed);
    let egress_wire_bytes = reader_a.egress_wire_bytes + reader_b.egress_wire_bytes;
    let writer_errors = writer_a_stats.errors.load(Ordering::Relaxed)
        + writer_b_stats.errors.load(Ordering::Relaxed);
    let duplicates = reader_a.duplicates + reader_b.duplicates;
    let corrupt = reader_a.corrupt + reader_b.corrupt;
    let reordered = reader_a.reordered + reader_b.reordered;
    let unexpected = reader_a.unexpected + reader_b.unexpected + writer_errors;
    let drain_timeouts = reader_a.drain_timeouts + reader_b.drain_timeouts;
    let maintenance_announces = reader_a.maintenance_announces + reader_b.maintenance_announces;
    let outstanding = shared_a.outstanding.len() + shared_b.outstanding.len();
    let buffer_pool_misses = shared_a.buffer_pool_misses.load(Ordering::Relaxed)
        + shared_b.buffer_pool_misses.load(Ordering::Relaxed);
    let slot_errors = shared_a.outstanding.slot_errors() + shared_b.outstanding.slot_errors();
    let available_credits = credits_a.available_permits() + credits_b.available_permits();
    let credit_leaks = window.saturating_mul(2).saturating_sub(available_credits);
    let missing = sent.saturating_sub(carried);
    let timed_out_frames = if drain_timeouts > 0 {
        outstanding as u64
    } else {
        0
    };
    let harness_headroom = harness_rate >= carried_rate * 1.25;

    println!("MEASURE_DONE");
    println!(
        "RESULT build={} sent={} carried={} proofs={} sent_a_to_b={} carried_a_to_b={} \
         sent_b_to_a={} carried_b_to_a={} sent_payload_bytes={} carried_payload_bytes={} \
         sent_payload_bytes_a_to_b={} carried_payload_bytes_a_to_b={} \
         sent_payload_bytes_b_to_a={} carried_payload_bytes_b_to_a={} elapsed_ms={} \
         carried_payload_bytes_per_sec={carried_rate:.1} forwarded_frames_per_sec={frame_rate:.1} \
         ingress_wire_bytes_per_sec={:.1} egress_wire_bytes_per_sec={:.1} \
         harness_source_payload_bytes_per_sec={:.1} \
         harness_sink_payload_bytes_per_sec={:.1} \
         harness_carried_payload_bytes_per_sec={harness_rate:.1} \
         harness_calibration_ms={harness_calibration_ms} harness_headroom={} \
         missing={} duplicates={} corrupt={} reordered={} unexpected={} timed_out_frames={} \
         drain_timeouts={} outstanding={} maintenance_announces={} buffer_pool_misses={} \
         credit_leaks={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        sent,
        carried,
        proofs,
        sent_a,
        carried_a,
        sent_b,
        carried_b,
        sent_bytes,
        carried_bytes,
        sent_bytes_a,
        carried_bytes_a,
        sent_bytes_b,
        carried_bytes_b,
        elapsed.as_millis(),
        ingress_wire_bytes as f64 / seconds,
        egress_wire_bytes as f64 / seconds,
        harness_rates.source,
        harness_rates.sink,
        u8::from(harness_headroom),
        missing,
        duplicates,
        corrupt,
        reordered,
        unexpected + slot_errors,
        timed_out_frames,
        drain_timeouts,
        outstanding,
        maintenance_announces,
        buffer_pool_misses,
        credit_leaks,
    );
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("raw transport driver failed: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn deterministic_frame_sizes_are_repeatable_and_cover_the_declared_range() {
        let sequence = || {
            let mut sizes = SizeSequence::new(benchmarks::DEFAULT_SIZE_SEED, 60, 420, 0);
            (0..2_000).map(|_| sizes.next_len()).collect::<Vec<_>>()
        };
        let first = sequence();
        assert_eq!(first, sequence());
        assert!(first.iter().all(|size| (60..=420).contains(size)));
        assert_eq!(first.iter().copied().min(), Some(60));
        assert_eq!(first.iter().copied().max(), Some(420));
    }

    #[test]
    fn deterministic_payloads_are_unique_and_self_validating() {
        let first = payload_for(A_TO_B, 1, 60, benchmarks::DEFAULT_SIZE_SEED);
        let second = payload_for(A_TO_B, 2, 60, benchmarks::DEFAULT_SIZE_SEED);
        assert_ne!(first, second);
        assert_eq!(
            parse_data_payload(&first, benchmarks::DEFAULT_SIZE_SEED)
                .map(|(direction, sequence)| (direction.id, sequence)),
            Some((A_TO_B.id, 1))
        );
    }

    #[test]
    fn generated_transport_frames_fit_the_rns_mtu_and_hash_uniquely() {
        let destination = DestinationHash::new([0x22; 16]);
        let relay = TransportId::new([0x33; 16]);
        let frames = (1..=1_024)
            .map(|sequence| {
                data_frame(
                    destination,
                    relay,
                    A_TO_B,
                    sequence,
                    420,
                    benchmarks::DEFAULT_SIZE_SEED,
                )
            })
            .collect::<Vec<_>>();
        let first = &frames[0];
        assert!(first.len() <= BROADCAST_MTU);
        let hashes = frames
            .iter()
            .map(|frame| *PacketHash::of_wire_packet(frame).unwrap().as_bytes())
            .collect::<BTreeSet<_>>();
        assert_eq!(hashes.len(), frames.len());
        let (header, _) = WirePacketHeader::parse(first).unwrap();
        assert_eq!(header.propagation, PropagationType::Transport);
        assert_eq!(header.destination_type, DestinationType::Single);
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(header.hops, 0);
        assert_eq!(header.transport_id, Some(relay));
    }

    #[test]
    fn hdlc_round_trip_preserves_transport_frame() {
        let frame = data_frame(
            DestinationHash::new([0x44; 16]),
            TransportId::new([0x55; 16]),
            B_TO_A,
            9,
            300,
            benchmarks::DEFAULT_SIZE_SEED,
        );
        let mut encoded = vec![0u8; max_encoded_len(frame.len())];
        let len = encode(&frame, &mut encoded).unwrap();
        let mut decoder = RnsSerialDecoder::<FRAME_CAP>::new();
        let mut decoded = Vec::new();
        decoder.feed_slice(&encoded[..len], |candidate| decoded = candidate.to_vec());
        assert_eq!(decoded, frame);
    }

    #[tokio::test]
    async fn framed_reader_reuses_buffers_and_counts_actual_tcp_bytes() {
        use tokio::io::AsyncWriteExt as _;

        let first = data_frame(
            DestinationHash::new([0x11; 16]),
            TransportId::new([0x22; 16]),
            A_TO_B,
            1,
            60,
            benchmarks::DEFAULT_SIZE_SEED,
        );
        let second = data_frame(
            DestinationHash::new([0x33; 16]),
            TransportId::new([0x44; 16]),
            B_TO_A,
            2,
            420,
            benchmarks::DEFAULT_SIZE_SEED,
        );
        let mut encoded = vec![0; max_encoded_len(first.len()) + max_encoded_len(second.len())];
        let first_len = encode(&first, &mut encoded).unwrap();
        let second_len = encode(&second, &mut encoded[first_len..]).unwrap();
        let total = first_len + second_len;
        let (mut write, read) = tokio::io::duplex(total * 2);
        write.write_all(&encoded[..total]).await.unwrap();
        write.shutdown().await.unwrap();

        let mut reader = FramedReader::new(read);
        let mut frame = Vec::with_capacity(FRAME_CAP);
        reader.next_into(&mut frame).await.unwrap();
        assert_eq!(frame, first);
        let capacity = frame.capacity();
        reader.next_into(&mut frame).await.unwrap();
        assert_eq!(frame, second);
        assert_eq!(frame.capacity(), capacity);
        assert_eq!(reader.wire_bytes(), total as u64);
    }

    #[tokio::test]
    async fn framed_reader_recovers_after_empty_and_oversized_frames() {
        let valid = data_frame(
            DestinationHash::new([0x33; 16]),
            TransportId::new([0x44; 16]),
            A_TO_B,
            9,
            120,
            benchmarks::DEFAULT_SIZE_SEED,
        );
        let mut encoded = vec![0; max_encoded_len(valid.len())];
        let valid_len = encode(&valid, &mut encoded).unwrap();
        encoded.truncate(valid_len);
        let mut wire = Vec::with_capacity(FRAME_CAP + encoded.len() + 8);
        wire.extend_from_slice(&[0x7e, 0x7e]);
        wire.push(0x7e);
        wire.resize(wire.len() + FRAME_CAP + 1, 0x01);
        wire.push(0x7e);
        wire.extend_from_slice(&encoded);
        let expected_wire = wire.len() as u64;
        let (mut write, read) = tokio::io::duplex(wire.len() + 1);
        write.write_all(&wire).await.unwrap();
        write.shutdown().await.unwrap();

        let mut reader = FramedReader::new(read);
        let mut frame = Vec::with_capacity(FRAME_CAP);
        let error = reader.next_into(&mut frame).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        reader.next_into(&mut frame).await.unwrap();
        assert_eq!(frame, valid);
        assert_eq!(reader.wire_bytes(), expected_wire);
    }

    #[tokio::test]
    async fn socket_writer_batches_exactly_and_recycles_both_buffer_classes() {
        let data = data_frame(
            DestinationHash::new([0x55; 16]),
            TransportId::new([0x66; 16]),
            A_TO_B,
            11,
            120,
            benchmarks::DEFAULT_SIZE_SEED,
        );
        let hash = PacketHash::of_wire_packet(&data).unwrap();
        let proof = proof_frame(hash, A_TO_B, 11);
        let expected_data = data.clone();
        let expected_proof = proof.clone();
        let (send, receive) = mpsc::channel(2);
        send.send(OutboundFrame {
            bytes: data,
            recycle: RecycleBuffer::Data,
        })
        .await
        .unwrap();
        send.send(OutboundFrame {
            bytes: proof,
            recycle: RecycleBuffer::Proof,
        })
        .await
        .unwrap();
        drop(send);
        let (data_pool, mut data_buffers) = mpsc::channel(1);
        let (proof_pool, mut proof_buffers) = mpsc::channel(1);
        let stats = Arc::new(WriterStats::default());
        let (write, read) = tokio::io::duplex(WRITE_BATCH_BYTES * 2);
        let writer = tokio::spawn(socket_writer(
            write,
            receive,
            stats.clone(),
            true,
            writer_buffer(BROADCAST_MTU),
            Some(data_pool),
            Some(proof_pool),
        ));
        let mut reader = FramedReader::new(read);
        let mut frame = Vec::with_capacity(BROADCAST_MTU);
        reader.next_into(&mut frame).await.unwrap();
        assert_eq!(frame, expected_data);
        reader.next_into(&mut frame).await.unwrap();
        assert_eq!(frame, expected_proof);
        writer.await.unwrap();
        assert!(data_buffers.recv().await.is_some());
        assert!(proof_buffers.recv().await.is_some());
        assert_eq!(stats.frames.load(Ordering::Relaxed), 2);
        assert_eq!(
            stats.framed_bytes.load(Ordering::Relaxed),
            reader.wire_bytes()
        );
        assert_eq!(stats.errors.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn socket_writer_recycles_a_frame_when_tcp_write_fails() {
        let frame = data_frame(
            DestinationHash::new([0x77; 16]),
            TransportId::new([0x88; 16]),
            B_TO_A,
            12,
            120,
            benchmarks::DEFAULT_SIZE_SEED,
        );
        let (send, receive) = mpsc::channel(1);
        send.send(OutboundFrame {
            bytes: frame,
            recycle: RecycleBuffer::Data,
        })
        .await
        .unwrap();
        drop(send);
        let (pool, mut buffers) = mpsc::channel(1);
        let stats = Arc::new(WriterStats::default());
        let (write, read) = tokio::io::duplex(1024);
        drop(read);
        socket_writer(
            write,
            receive,
            stats.clone(),
            true,
            writer_buffer(BROADCAST_MTU),
            Some(pool),
            None,
        )
        .await;
        assert!(buffers.recv().await.is_some());
        assert_eq!(stats.frames.load(Ordering::Relaxed), 0);
        assert_eq!(stats.errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn outstanding_ring_detects_duplicates_and_reuses_sequence_slots() {
        let outstanding = Outstanding::new(2);
        assert!(outstanding.insert(1, [1; 32]));
        assert_eq!(outstanding.get(1), Some([1; 32]));
        assert!(!outstanding.insert(3, [3; 32]));
        assert_eq!(outstanding.slot_errors(), 1);
        assert_eq!(outstanding.remove(1), Some([1; 32]));
        assert_eq!(outstanding.get(1), None);
        assert!(outstanding.insert(3, [3; 32]));
        assert_eq!(outstanding.remove(3), Some([3; 32]));
        assert!(outstanding.is_empty());
    }

    #[test]
    fn calibration_uses_whichever_driver_half_is_slower() {
        assert_eq!(
            CalibrationRates {
                source: 2_000.0,
                sink: 1_500.0,
            }
            .limiting(),
            1_500.0
        );
        assert_eq!(
            CalibrationRates {
                source: 1_250.0,
                sink: 3_000.0,
            }
            .limiting(),
            1_250.0
        );
    }

    #[test]
    fn warmed_driver_encode_decode_generate_validate_path_does_not_allocate() {
        let destination = DestinationHash::new([0x51; 16]);
        let relay = TransportId::new([0x52; 16]);
        let mut frame = Vec::with_capacity(BROADCAST_MTU);
        let mut encoded = vec![0; max_encoded_len(BROADCAST_MTU)];
        let payload_template = payload_template(A_TO_B, 420, benchmarks::DEFAULT_SIZE_SEED);
        prepare_data_frame_from_template(&mut frame, destination, relay, 1, 420, &payload_template);
        let mut scanner = RnsSerialScanner::new();
        let mut decoded = CappedFrame::new();
        let mut proof = Vec::with_capacity(BROADCAST_MTU);
        let frame_ptr = frame.as_ptr();
        let encoded_ptr = encoded.as_ptr();

        let payload_len = 524_288 - HEADER_MIN_LEN - IFAC_MIN_LEN;
        let template = resource_frame_template(
            LinkId::new([0x53; 16]),
            B_TO_A,
            payload_len,
            benchmarks::DEFAULT_SIZE_SEED,
        );
        let mut resource = template.clone();
        let resource_ptr = resource.as_ptr();
        let (_, expected_resource_payload) = WirePacketHeader::parse(&template).unwrap();
        let mut resource_encoded = vec![0; max_encoded_len(resource.len())];
        let mut resource_scanner = RnsSerialScanner::new();
        let mut decoded_resource = CappedFrame::new();

        let allocations = allocation_gate::count(|| {
            for sequence in 2..=4_096 {
                let payload_len = 60 + sequence as usize % 361;
                prepare_data_frame_from_template(
                    &mut frame,
                    destination,
                    relay,
                    sequence,
                    payload_len,
                    &payload_template,
                );
                let expected_hash = PacketHash::of_wire_packet(&frame).unwrap();
                let encoded_len = encode(&frame, &mut encoded).unwrap();
                let mut offset = 0;
                assert_eq!(
                    scanner.next_frame_into(&encoded[..encoded_len], &mut offset, &mut decoded,),
                    Ok(Some(frame.len()))
                );
                let (header, payload) = WirePacketHeader::parse(&decoded.bytes).unwrap();
                assert_eq!(header.packet_type, PacketType::Data);
                assert_eq!(
                    parse_data_payload_against(payload, &payload_template)
                        .map(|(_, observed)| observed),
                    Some(sequence)
                );
                prepare_proof_frame(&mut proof, expected_hash, A_TO_B, sequence);
                assert!(encode(&proof, &mut encoded).unwrap() > proof.len());
                assert_eq!(frame.as_ptr(), frame_ptr);
                assert_eq!(encoded.as_ptr(), encoded_ptr);
            }

            for sequence in 1..=128 {
                prepare_resource_frame(&mut resource, payload_len, sequence);
                let encoded_len = encode(&resource, &mut resource_encoded).unwrap();
                let mut offset = 0;
                assert_eq!(
                    resource_scanner.next_frame_into(
                        &resource_encoded[..encoded_len],
                        &mut offset,
                        &mut decoded_resource,
                    ),
                    Ok(Some(resource.len()))
                );
                let (_, payload) = WirePacketHeader::parse(&decoded_resource.bytes).unwrap();
                assert_eq!(
                    parse_resource_payload(payload, expected_resource_payload)
                        .map(|(_, observed)| observed),
                    Some(sequence)
                );
                assert_eq!(resource.as_ptr(), resource_ptr);
            }
        });
        assert_eq!(
            allocations, 0,
            "warmed driver hot path allocated {allocations} time(s)"
        );
    }

    #[test]
    fn final_hop_validation_rejects_unstripped_transport_headers() {
        let destination = DestinationHash::new([0x66; 16]);
        let transport = data_frame(
            destination,
            TransportId::new([0x77; 16]),
            A_TO_B,
            1,
            60,
            benchmarks::DEFAULT_SIZE_SEED,
        );
        assert!(validate_carried_data(
            &transport,
            destination,
            A_TO_B,
            1,
            benchmarks::DEFAULT_SIZE_SEED,
        )
        .is_err());

        let (source_header, payload) = WirePacketHeader::parse(&transport).unwrap();
        let final_header = WirePacketHeader {
            ifac_flag: IfacFlag::Open,
            context_flag: ContextFlag::Unset,
            propagation: PropagationType::Broadcast,
            destination_type: source_header.destination_type,
            packet_type: source_header.packet_type,
            hops: 1,
            transport_id: None,
            address: source_header.address,
            context: source_header.context,
        };
        let mut final_hop = vec![0u8; BROADCAST_MTU];
        let header_len = final_header.write(&mut final_hop).unwrap();
        final_hop[header_len..header_len + payload.len()].copy_from_slice(payload);
        final_hop.truncate(header_len + payload.len());
        validate_carried_data(
            &final_hop,
            destination,
            A_TO_B,
            1,
            benchmarks::DEFAULT_SIZE_SEED,
        )
        .expect("the exact final-hop rewrite is accepted");
        assert_eq!(
            PacketHash::of_wire_packet(&transport).unwrap(),
            PacketHash::of_wire_packet(&final_hop).unwrap(),
            "transport header removal and hop increment preserve packet identity"
        );
    }

    #[test]
    fn resource_frames_use_the_negotiated_payload_ceiling_and_validate_exactly() {
        let link_id = LinkId::new([0x88; 16]);
        let mtu = 8_192;
        let payload_len = mtu - HEADER_MIN_LEN - IFAC_MIN_LEN;
        let source =
            resource_frame_template(link_id, A_TO_B, payload_len, benchmarks::DEFAULT_SIZE_SEED);
        let source = resource_frame(&source, payload_len, 7);
        assert_eq!(source.len(), mtu - IFAC_MIN_LEN);
        let (_, expected_payload) = WirePacketHeader::parse(&source).unwrap();
        assert_eq!(
            parse_resource_payload(expected_payload, expected_payload)
                .map(|(direction, sequence)| (direction.id, sequence)),
            Some((A_TO_B.id, 7))
        );
        assert!(
            validate_resource_frame(&source, link_id, A_TO_B, 7, expected_payload).is_err(),
            "the source-side hop count is not a forwarded frame"
        );

        let (header, payload) = WirePacketHeader::parse(&source).unwrap();
        let forwarded_header = WirePacketHeader { hops: 1, ..header };
        let mut forwarded = vec![0u8; mtu];
        let header_len = forwarded_header.write(&mut forwarded).unwrap();
        forwarded[header_len..header_len + payload.len()].copy_from_slice(payload);
        forwarded.truncate(header_len + payload.len());
        validate_resource_frame(&forwarded, link_id, A_TO_B, 7, expected_payload)
            .expect("the exact transported-link switch is valid");

        let last = forwarded.len() - 1;
        forwarded[last] ^= 1;
        assert!(
            validate_resource_frame(&forwarded, link_id, A_TO_B, 7, expected_payload).is_err(),
            "payload corruption is detected"
        );
    }

    #[test]
    fn maximum_mtu_resource_frame_round_trips_through_hdlc() {
        let link_id = LinkId::new([0x99; 16]);
        let payload_len = MAX_LINK_MTU - HEADER_MIN_LEN - IFAC_MIN_LEN;
        let template =
            resource_frame_template(link_id, B_TO_A, payload_len, benchmarks::DEFAULT_SIZE_SEED);
        let frame = resource_frame(&template, payload_len, 42);
        let mut encoded = vec![0u8; max_encoded_len(frame.len())];
        let len = encode(&frame, &mut encoded).unwrap();
        let mut decoder = Box::new(RnsSerialDecoder::<FRAME_CAP>::new());
        let mut decoded = Vec::new();
        decoder.feed_slice(&encoded[..len], |candidate| decoded = candidate.to_vec());
        assert_eq!(decoded, frame);
    }
}
