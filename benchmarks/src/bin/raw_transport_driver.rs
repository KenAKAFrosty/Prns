use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use benchmarks::{ScenarioManifest, SizeSequence};
use personal_rns::crypto::{Ed25519PublicKey, X25519PublicKey};
use personal_rns::engine::InstantMillis;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::interfaces::rns_serial_framing::{encode, max_encoded_len, RnsSerialDecoder};
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
    TransportId, WireContext, WirePacketHeader, BROADCAST_MTU, HEADER_MIN_LEN, IFAC_MIN_LEN,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Semaphore};

const DRIVER_SLUG: &str = "benchmark-wire-driver";
const DATA_MAGIC: &[u8; 8] = b"PRNSRAW1";
const PROOF_MAGIC: &[u8; 8] = b"PRNSPRF1";
const RESOURCE_MAGIC: &[u8; 8] = b"PRNSRES1";
const FRAME_CAP: usize = MAX_LINK_MTU;
const READ_CHUNK: usize = 16 * 1024;
const WRITER_QUEUE: usize = 1024;
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
    decoder: Box<RnsSerialDecoder<FRAME_CAP>>,
    pending: VecDeque<Vec<u8>>,
    read_buf: [u8; READ_CHUNK],
}

impl<R: AsyncRead + Unpin> FramedReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            decoder: Box::new(RnsSerialDecoder::new()),
            pending: VecDeque::new(),
            read_buf: [0; READ_CHUNK],
        }
    }

    async fn next(&mut self) -> io::Result<Vec<u8>> {
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return Ok(frame);
            }
            let read = self.inner.read(&mut self.read_buf).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "relay TCP stream closed",
                ));
            }
            self.decoder.feed_slice(&self.read_buf[..read], |frame| {
                if !frame.is_empty() {
                    self.pending.push_back(frame.to_vec());
                }
            });
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

fn payload_for(direction: Direction, sequence: u64, len: usize, seed: u64) -> Vec<u8> {
    assert!(len >= 17, "raw transport payload carries its identity");
    let mut payload = vec![0u8; len];
    payload[..8].copy_from_slice(DATA_MAGIC);
    payload[8] = direction.id;
    payload[9..17].copy_from_slice(&sequence.to_be_bytes());
    let mut state = seed ^ direction.seed_xor ^ sequence.wrapping_mul(0x9E37_79B9_7F4A_7C15);
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
    (payload == payload_for(direction, sequence, payload.len(), seed))
        .then_some((direction, sequence))
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

fn resource_frame(template: &[u8], payload_len: usize, sequence: u64) -> Vec<u8> {
    let mut frame = template.to_vec();
    let payload_offset = frame.len() - payload_len;
    frame[payload_offset + 9..payload_offset + 17].copy_from_slice(&sequence.to_be_bytes());
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

fn data_frame(
    destination: DestinationHash,
    relay: TransportId,
    direction: Direction,
    sequence: u64,
    payload_len: usize,
    seed: u64,
) -> Vec<u8> {
    let payload = payload_for(direction, sequence, payload_len, seed);
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
    let mut frame = vec![0u8; BROADCAST_MTU];
    let header_len = header.write(&mut frame).expect("data header fits");
    frame[header_len..header_len + payload.len()].copy_from_slice(&payload);
    frame.truncate(header_len + payload.len());
    frame
}

fn proof_frame(packet_hash: PacketHash, direction: Direction, sequence: u64) -> Vec<u8> {
    let mut payload = Vec::with_capacity(49);
    payload.extend_from_slice(PROOF_MAGIC);
    payload.push(direction.id);
    payload.extend_from_slice(&sequence.to_be_bytes());
    payload.extend_from_slice(packet_hash.as_bytes());
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
    let mut frame = vec![0u8; BROADCAST_MTU];
    let header_len = header.write(&mut frame).expect("proof header fits");
    frame[header_len..header_len + payload.len()].copy_from_slice(&payload);
    frame.truncate(header_len + payload.len());
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
        loop {
            let frame = reader.next().await?;
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
    loop {
        let frame = reader.next().await?;
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

#[derive(Default)]
struct SharedDirection {
    sent: AtomicU64,
    sent_payload_bytes: AtomicU64,
    generator_done: AtomicBool,
    outstanding: Mutex<HashMap<u64, [u8; 32]>>,
}

#[derive(Default)]
struct WriterStats {
    frames: AtomicU64,
    framed_bytes: AtomicU64,
    errors: AtomicU64,
}

async fn socket_writer(
    mut writer: OwnedWriteHalf,
    mut receive: mpsc::Receiver<Vec<u8>>,
    stats: Arc<WriterStats>,
    shutdown_when_done: bool,
) {
    while let Some(frame) = receive.recv().await {
        match write_frame(&mut writer, &frame).await {
            Ok(framed) => {
                stats.frames.fetch_add(1, Ordering::Relaxed);
                stats
                    .framed_bytes
                    .fetch_add(framed as u64, Ordering::Relaxed);
            }
            Err(_) => {
                stats.errors.fetch_add(1, Ordering::Relaxed);
                break;
            }
        }
    }
    if shutdown_when_done {
        let _ = writer.shutdown().await;
    }
}

struct GeneratorContext {
    destination: DestinationHash,
    relay: TransportId,
    direction: Direction,
    profile: benchmarks::WorkloadProfile,
    deadline: tokio::time::Instant,
}

async fn generate_direction(
    send: mpsc::Sender<Vec<u8>>,
    credits: Arc<Semaphore>,
    shared: Arc<SharedDirection>,
    context: GeneratorContext,
) {
    let GeneratorContext {
        destination,
        relay,
        direction,
        profile,
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
            permit = credits.clone().acquire_owned() => {
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
        let len = sizes.next_len();
        let frame = data_frame(
            destination,
            relay,
            direction,
            sequence,
            len,
            profile.size_seed,
        );
        let hash = PacketHash::of_wire_packet(&frame).expect("generated data hashes");
        shared
            .outstanding
            .lock()
            .expect("outstanding map")
            .insert(sequence, *hash.as_bytes());
        if send.send(frame).await.is_err() {
            shared
                .outstanding
                .lock()
                .expect("outstanding map")
                .remove(&sequence);
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
    side_send: mpsc::Sender<Vec<u8>>,
    incoming_destination: DestinationHash,
    incoming_direction: Direction,
    incoming: Arc<SharedDirection>,
    local_direction: Direction,
    local: Arc<SharedDirection>,
    local_credits: Arc<Semaphore>,
    seed: u64,
    drain_timeout: Duration,
}

async fn consume_side(
    mut reader: FramedReader<OwnedReadHalf>,
    context: ReaderContext,
) -> ReaderStats {
    let mut stats = ReaderStats::default();
    let mut next_incoming = 1u64;
    let mut drain_started = None;
    loop {
        let incoming_empty = context
            .incoming
            .outstanding
            .lock()
            .expect("incoming outstanding map")
            .is_empty();
        let local_empty = context
            .local
            .outstanding
            .lock()
            .expect("local outstanding map")
            .is_empty();
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
        if drain_started.is_some_and(|started| started.elapsed() >= context.drain_timeout) {
            stats.drain_timeouts += 1;
            break;
        }

        let frame = match tokio::time::timeout(Duration::from_millis(100), reader.next()).await {
            Ok(Ok(frame)) => frame,
            Ok(Err(_)) => {
                stats.unexpected += 1;
                break;
            }
            Err(_) => continue,
        };
        let mut encoded = vec![0u8; max_encoded_len(frame.len())];
        stats.egress_wire_bytes += encode(&frame, &mut encoded)
            .expect("a received frame re-encodes for wire accounting")
            as u64;
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
                let hash = PacketHash::of_wire_packet(&frame).expect("parsed data hashes");
                let Some((direction, sequence)) = parse_data_payload(payload, context.seed) else {
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
                if context
                    .side_send
                    .send(proof_frame(hash, direction, sequence))
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
                let expected = context
                    .local
                    .outstanding
                    .lock()
                    .expect("outstanding map")
                    .remove(&sequence);
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
    stats
}

struct ResourceGeneratorContext {
    frame_template: Arc<Vec<u8>>,
    payload_len: usize,
    deadline: tokio::time::Instant,
}

async fn generate_resource_direction(
    send: mpsc::Sender<Vec<u8>>,
    credits: Arc<Semaphore>,
    shared: Arc<SharedDirection>,
    context: ResourceGeneratorContext,
) {
    let mut sequence = 1u64;
    loop {
        let permit = tokio::select! {
            _ = tokio::time::sleep_until(context.deadline) => break,
            permit = credits.clone().acquire_owned() => {
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
        let frame = resource_frame(&context.frame_template, context.payload_len, sequence);
        shared
            .outstanding
            .lock()
            .expect("outstanding map")
            .insert(sequence, [0; 32]);
        if send.send(frame).await.is_err() {
            shared
                .outstanding
                .lock()
                .expect("outstanding map")
                .remove(&sequence);
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
) -> ReaderStats {
    let mut stats = ReaderStats::default();
    let mut next_incoming = 1u64;
    let mut drain_started = None;
    let mut encoded = Vec::new();
    loop {
        let incoming_empty = context
            .incoming
            .outstanding
            .lock()
            .expect("incoming outstanding map")
            .is_empty();
        let local_empty = context
            .local
            .outstanding
            .lock()
            .expect("local outstanding map")
            .is_empty();
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
        if drain_started.is_some_and(|started| started.elapsed() >= context.drain_timeout) {
            stats.drain_timeouts += 1;
            break;
        }

        let frame = match tokio::time::timeout(Duration::from_millis(100), reader.next()).await {
            Ok(Ok(frame)) => frame,
            Ok(Err(_)) => {
                stats.unexpected += 1;
                break;
            }
            Err(_) => continue,
        };
        encoded.resize(max_encoded_len(frame.len()), 0);
        stats.egress_wire_bytes += encode(&frame, &mut encoded)
            .expect("a received frame re-encodes for wire accounting")
            as u64;
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
        let outstanding = context
            .incoming
            .outstanding
            .lock()
            .expect("outstanding map")
            .remove(&sequence);
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
    harness_rate: f64,
    harness_calibration_ms: u64,
}

async fn run_resource_measurement(measurement: ResourceMeasurement) -> io::Result<()> {
    println!("MEASURE_READY");
    let mut command = String::new();
    std::io::stdin().read_line(&mut command)?;
    if command.trim() != "START" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected START",
        ));
    }

    let ResourceMeasurement {
        write_a,
        read_a,
        write_b,
        read_b,
        link_id,
        payload_len,
        profile,
        duration,
        harness_rate,
        harness_calibration_ms,
    } = measurement;
    let template_a = Arc::new(resource_frame_template(
        link_id,
        A_TO_B,
        payload_len,
        profile.size_seed,
    ));
    let template_b = Arc::new(resource_frame_template(
        link_id,
        B_TO_A,
        payload_len,
        profile.size_seed,
    ));
    let (_, expected_a) = WirePacketHeader::parse(&template_a)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "A resource template"))?;
    let (_, expected_b) = WirePacketHeader::parse(&template_b)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "B resource template"))?;
    let expected_a = expected_a.to_vec();
    let expected_b = expected_b.to_vec();

    let (send_a, receive_a) = mpsc::channel(profile.window);
    let (send_b, receive_b) = mpsc::channel(profile.window);
    let writer_a_stats = Arc::new(WriterStats::default());
    let writer_b_stats = Arc::new(WriterStats::default());
    let writer_a = tokio::spawn(socket_writer(
        write_a,
        receive_a,
        writer_a_stats.clone(),
        false,
    ));
    let writer_b = tokio::spawn(socket_writer(
        write_b,
        receive_b,
        writer_b_stats.clone(),
        false,
    ));
    let shared_a = Arc::new(SharedDirection::default());
    let shared_b = Arc::new(SharedDirection::default());
    let credits_a = Arc::new(Semaphore::new(profile.window));
    let credits_b = Arc::new(Semaphore::new(profile.window));
    let deadline = tokio::time::Instant::now() + duration;
    let started = Instant::now();

    let generator_a = tokio::spawn(generate_resource_direction(
        send_a.clone(),
        credits_a.clone(),
        shared_a.clone(),
        ResourceGeneratorContext {
            frame_template: template_a,
            payload_len,
            deadline,
        },
    ));
    let generator_b = tokio::spawn(generate_resource_direction(
        send_b.clone(),
        credits_b.clone(),
        shared_b.clone(),
        ResourceGeneratorContext {
            frame_template: template_b,
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
            incoming_credits: credits_b,
            drain_timeout: Duration::from_millis(profile.drain_timeout_ms),
        },
    ));
    let consumer_b = tokio::spawn(consume_resource_side(
        read_b,
        ResourceReaderContext {
            link_id,
            incoming_direction: A_TO_B,
            incoming_payload: expected_a,
            incoming: shared_a.clone(),
            local: shared_b.clone(),
            incoming_credits: credits_a,
            drain_timeout: Duration::from_millis(profile.drain_timeout_ms),
        },
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
    let outstanding = shared_a.outstanding.lock().expect("A outstanding").len()
        + shared_b.outstanding.lock().expect("B outstanding").len();
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
         harness_carried_payload_bytes_per_sec={harness_rate:.1} \
         harness_calibration_ms={harness_calibration_ms} harness_headroom={} \
         missing={} duplicates={} corrupt={} reordered={} unexpected={} timed_out_frames={} \
         drain_timeouts={} outstanding={} maintenance_announces={} negotiated_link_mtu_bytes={} \
         resource_payload_bytes_per_frame={}",
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
        u8::from(harness_headroom),
        missing,
        duplicates,
        corrupt,
        reordered,
        unexpected,
        timed_out_frames,
        drain_timeouts,
        outstanding,
        maintenance_announces,
        payload_len + HEADER_MIN_LEN + IFAC_MIN_LEN,
        payload_len,
    );
    Ok(())
}

async fn calibration_generate(
    send: mpsc::Sender<Vec<u8>>,
    profile: &benchmarks::WorkloadProfile,
    direction: Direction,
    duration: Duration,
    resource: Option<(Arc<Vec<u8>>, usize)>,
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
        let (frame, len) = if let Some((template, payload_len)) = &resource {
            (
                resource_frame(template, *payload_len, sequence),
                *payload_len,
            )
        } else {
            let len = sizes.next_len();
            (
                data_frame(
                    destination,
                    relay,
                    direction,
                    sequence,
                    len,
                    profile.size_seed,
                ),
                len,
            )
        };
        if send.send(frame).await.is_err() {
            break;
        }
        payload_bytes += len as u64;
        sequence += 1;
    }
    payload_bytes
}

fn calibration_sink(read: &mut std::net::TcpStream) -> io::Result<u64> {
    use std::io::Read as _;

    let mut buffer = [0u8; 64 * 1024];
    let mut wire_bytes = 0u64;
    loop {
        let received = read.read(&mut buffer)?;
        if received == 0 {
            return Ok(wire_bytes);
        }
        wire_bytes += received as u64;
    }
}

async fn calibrate(
    profile: benchmarks::WorkloadProfile,
    smoke: bool,
    resource: Option<(LinkId, usize)>,
) -> io::Result<f64> {
    let duration = if smoke {
        Duration::from_millis(SMOKE_CALIBRATION_MILLIS)
    } else {
        Duration::from_secs(CALIBRATION_SECONDS)
    };
    // Connect each synthetic source directly to its opposite sink. Running both
    // one-way TCP paths concurrently is the relay-free equivalent of the live
    // bidirectional workload, without inserting a copy loop that becomes a third
    // implementation under test.
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
    let (unused_client_read_a, write_a) = client_a.into_split();
    let (unused_client_read_b, write_b) = client_b.into_split();
    drop(unused_client_read_a);
    drop(unused_client_read_b);
    let sinks = tokio::task::spawn_blocking(move || {
        let mut server_a = server_a.into_std()?;
        let mut server_b = server_b.into_std()?;
        server_a.set_nonblocking(false)?;
        server_b.set_nonblocking(false)?;
        std::thread::scope(|scope| {
            let sink_a = scope.spawn(|| calibration_sink(&mut server_a));
            let sink_b = scope.spawn(|| calibration_sink(&mut server_b));
            Ok::<(u64, u64), io::Error>((
                sink_a.join().expect("calibration A sink thread")?,
                sink_b.join().expect("calibration B sink thread")?,
            ))
        })
    });
    let profile_a = profile.clone();
    let profile_b = profile.clone();
    let queue = if resource.is_some() {
        profile.window
    } else {
        WRITER_QUEUE
    };
    let (resource_a, resource_b) = resource.map_or((None, None), |(link_id, payload_len)| {
        (
            Some((
                Arc::new(resource_frame_template(
                    link_id,
                    A_TO_B,
                    payload_len,
                    profile.size_seed,
                )),
                payload_len,
            )),
            Some((
                Arc::new(resource_frame_template(
                    link_id,
                    B_TO_A,
                    payload_len,
                    profile.size_seed,
                )),
                payload_len,
            )),
        )
    });
    let (send_a, receive_a) = mpsc::channel(queue);
    let (send_b, receive_b) = mpsc::channel(queue);
    let writer_a_stats = Arc::new(WriterStats::default());
    let writer_b_stats = Arc::new(WriterStats::default());
    let writer_a = tokio::spawn(socket_writer(
        write_a,
        receive_a,
        writer_a_stats.clone(),
        true,
    ));
    let writer_b = tokio::spawn(socket_writer(
        write_b,
        receive_b,
        writer_b_stats.clone(),
        true,
    ));
    let generator_a = tokio::spawn(async move {
        calibration_generate(send_a, &profile_a, A_TO_B, duration, resource_a).await
    });
    let generator_b = tokio::spawn(async move {
        calibration_generate(send_b, &profile_b, B_TO_A, duration, resource_b).await
    });
    let join_error = |error| io::Error::other(format!("calibration task: {error}"));
    let (sent_a, sent_b) = tokio::join!(generator_a, generator_b);
    let sent = sent_a.map_err(join_error)? + sent_b.map_err(join_error)?;
    let (writer_a, writer_b) = tokio::join!(writer_a, writer_b);
    writer_a.map_err(join_error)?;
    writer_b.map_err(join_error)?;
    let (received_a, received_b) = sinks
        .await
        .map_err(|error| io::Error::other(format!("calibration sinks: {error}")))??;
    let received_wire = received_a + received_b;
    let writer_errors = writer_a_stats.errors.load(Ordering::Relaxed)
        + writer_b_stats.errors.load(Ordering::Relaxed);
    if writer_errors != 0 {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            format!("calibration writers failed {writer_errors} time(s)"),
        ));
    }
    let written_wire = writer_a_stats.framed_bytes.load(Ordering::Relaxed)
        + writer_b_stats.framed_bytes.load(Ordering::Relaxed);
    if written_wire != received_wire {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("calibration lost TCP bytes: written={written_wire} received={received_wire}"),
        ));
    }
    Ok(sent as f64 / duration.as_secs_f64().max(f64::EPSILON))
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
        let harness_rate = calibrate(
            manifest.profile.clone(),
            smoke,
            Some((link_id, payload_len)),
        )
        .await?;
        println!(
            "HARNESS carried_payload_bytes_per_sec={harness_rate:.1} calibration_ms={harness_calibration_ms}"
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
            harness_rate,
            harness_calibration_ms,
        })
        .await;
    }

    let harness_rate = calibrate(manifest.profile.clone(), smoke, None).await?;
    println!(
        "HARNESS carried_payload_bytes_per_sec={harness_rate:.1} calibration_ms={harness_calibration_ms}"
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

    println!("MEASURE_READY");
    let mut command = String::new();
    std::io::stdin().read_line(&mut command)?;
    if command.trim() != "START" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected START",
        ));
    }

    let (send_a, receive_a) = mpsc::channel(WRITER_QUEUE);
    let (send_b, receive_b) = mpsc::channel(WRITER_QUEUE);
    let writer_a_stats = Arc::new(WriterStats::default());
    let writer_b_stats = Arc::new(WriterStats::default());
    let writer_a = tokio::spawn(socket_writer(
        write_a,
        receive_a,
        writer_a_stats.clone(),
        false,
    ));
    let writer_b = tokio::spawn(socket_writer(
        write_b,
        receive_b,
        writer_b_stats.clone(),
        false,
    ));

    let shared_a = Arc::new(SharedDirection::default());
    let shared_b = Arc::new(SharedDirection::default());
    let credits_a = Arc::new(Semaphore::new(manifest.profile.window));
    let credits_b = Arc::new(Semaphore::new(manifest.profile.window));
    let deadline = tokio::time::Instant::now() + duration;
    let started = Instant::now();

    let generator_a = tokio::spawn(generate_direction(
        send_a.clone(),
        credits_a.clone(),
        shared_a.clone(),
        GeneratorContext {
            destination: destination_b,
            relay,
            direction: A_TO_B,
            profile: manifest.profile.clone(),
            deadline,
        },
    ));
    let generator_b = tokio::spawn(generate_direction(
        send_b.clone(),
        credits_b.clone(),
        shared_b.clone(),
        GeneratorContext {
            destination: destination_a,
            relay,
            direction: B_TO_A,
            profile: manifest.profile.clone(),
            deadline,
        },
    ));
    let consumer_a = tokio::spawn(consume_side(
        read_a,
        ReaderContext {
            side_send: send_a.clone(),
            incoming_destination: destination_a,
            incoming_direction: B_TO_A,
            incoming: shared_b.clone(),
            local_direction: A_TO_B,
            local: shared_a.clone(),
            local_credits: credits_a,
            seed: manifest.profile.size_seed,
            drain_timeout: Duration::from_millis(manifest.profile.drain_timeout_ms),
        },
    ));
    let consumer_b = tokio::spawn(consume_side(
        read_b,
        ReaderContext {
            side_send: send_b.clone(),
            incoming_destination: destination_b,
            incoming_direction: A_TO_B,
            incoming: shared_a.clone(),
            local_direction: B_TO_A,
            local: shared_b.clone(),
            local_credits: credits_b,
            seed: manifest.profile.size_seed,
            drain_timeout: Duration::from_millis(manifest.profile.drain_timeout_ms),
        },
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
    let outstanding = shared_a.outstanding.lock().expect("A outstanding").len()
        + shared_b.outstanding.lock().expect("B outstanding").len();
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
         harness_carried_payload_bytes_per_sec={harness_rate:.1} \
         harness_calibration_ms={harness_calibration_ms} harness_headroom={} \
         missing={} duplicates={} corrupt={} reordered={} unexpected={} timed_out_frames={} \
         drain_timeouts={} outstanding={} maintenance_announces={}",
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
        u8::from(harness_headroom),
        missing,
        duplicates,
        corrupt,
        reordered,
        unexpected,
        timed_out_frames,
        drain_timeouts,
        outstanding,
        maintenance_announces,
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
