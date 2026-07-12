use std::hint::black_box;

use iai_callgrind::{
    library_benchmark, library_benchmark_group, main, Callgrind, LibraryBenchmarkConfig,
};
use personal_rns::crypto::{
    ed25519_public_key, ed25519_sign, ed25519_verify, token_open, token_seal,
    x25519_diffie_hellman, x25519_public_key, Ed25519PublicKey, Ed25519SecretKey, Ed25519Signature,
    TokenKey, X25519PublicKey, X25519SecretKey,
};
use personal_rns::identity::ENCRYPTION_IV_LEN;

use benchmarks::microscope::{Cycle, Forward};
use personal_rns::engine::{CommandId, InstantMillis};
use personal_rns::interfaces::rns_serial_framing;
use personal_rns::routing::dedup::{HeapPacketHashHistory, PacketHash, PacketHashHistory};
use personal_rns::routing::links::channel::table::impls::HeapChannelTable;
use personal_rns::routing::links::channel::table::{ChannelTable, OutstandingSend, TxOutcome};
use personal_rns::routing::links::channel::{ChannelSequence, MessageType};
use personal_rns::routing::links::LinkId;

const PAYLOAD_LEN: usize = 300;

#[library_benchmark]
fn ed25519_sign_300b() {
    let signer = Ed25519SecretKey::new([0x42; 32]);
    let message = [0xAB_u8; 32];
    black_box(ed25519_sign(black_box(&signer), black_box(&message)));
}

fn verify_inputs() -> (Ed25519PublicKey, [u8; 32], Ed25519Signature) {
    let signer = Ed25519SecretKey::new([0x42; 32]);
    let message = [0xAB_u8; 32];
    let signature = ed25519_sign(&signer, &message);
    (ed25519_public_key(&signer), message, signature)
}

#[library_benchmark]
#[bench::authentic(setup = verify_inputs)]
fn ed25519_verify_300b(input: (Ed25519PublicKey, [u8; 32], Ed25519Signature)) {
    let (verifier, message, signature) = input;
    black_box(
        ed25519_verify(
            black_box(&verifier),
            black_box(&message),
            black_box(&signature),
        )
        .expect("authentic"),
    );
}

#[library_benchmark]
fn x25519_pubkey() {
    let ours = X25519SecretKey::new([0x11; 32]);
    black_box(x25519_public_key(black_box(&ours)));
}

fn dh_inputs() -> (X25519SecretKey, X25519PublicKey) {
    let ours = X25519SecretKey::new([0x11; 32]);
    let theirs = x25519_public_key(&X25519SecretKey::new([0x33; 32]));
    (ours, theirs)
}

#[library_benchmark]
#[bench::pair(setup = dh_inputs)]
fn x25519_dh(input: (X25519SecretKey, X25519PublicKey)) {
    let (ours, theirs) = input;
    black_box(x25519_diffie_hellman(black_box(&ours), black_box(&theirs)));
}

#[library_benchmark]
fn token_seal_300b() {
    let key = TokenKey::from_derived(&[0x5A_u8; 64]).expect("64-byte derived key");
    let iv = [0x77_u8; ENCRYPTION_IV_LEN];
    let plaintext = [0xAB_u8; PAYLOAD_LEN];
    let mut out = [0u8; 512];
    black_box(
        token_seal(
            black_box(&key),
            black_box(&iv),
            black_box(&plaintext),
            &mut out,
        )
        .expect("seals"),
    );
}

fn sealed_token() -> Vec<u8> {
    let key = TokenKey::from_derived(&[0x5A_u8; 64]).expect("64-byte derived key");
    let iv = [0x77_u8; ENCRYPTION_IV_LEN];
    let plaintext = [0xAB_u8; PAYLOAD_LEN];
    let mut sealed = [0u8; 512];
    let n = token_seal(&key, &iv, &plaintext, &mut sealed).expect("seals");
    sealed[..n].to_vec()
}

#[library_benchmark]
#[bench::b300(setup = sealed_token)]
fn token_open_300b(sealed: Vec<u8>) {
    let key = TokenKey::from_derived(&[0x5A_u8; 64]).expect("64-byte derived key");
    let mut out = [0u8; 512];
    black_box(token_open(black_box(&key), black_box(&sealed), &mut out).expect("opens"));
}

fn fresh_cycle() -> Cycle {
    Cycle::new()
}

fn sealed_cycle() -> Cycle {
    let mut cycle = Cycle::new();
    cycle.seal();
    cycle
}

fn delivered_cycle() -> Cycle {
    let mut cycle = Cycle::new();
    cycle.seal();
    cycle.deliver_prove();
    cycle
}

#[library_benchmark]
#[bench::single(setup = fresh_cycle)]
fn cycle_roundtrip(mut cycle: Cycle) -> Cycle {
    cycle.seal();
    cycle.deliver_prove();
    cycle.settle();
    cycle
}

#[library_benchmark]
#[bench::single(setup = fresh_cycle)]
fn cycle_seal(mut cycle: Cycle) -> Cycle {
    cycle.seal();
    cycle
}

#[library_benchmark]
#[bench::single(setup = sealed_cycle)]
fn cycle_deliver_prove(mut cycle: Cycle) -> Cycle {
    cycle.deliver_prove();
    cycle
}

#[library_benchmark]
#[bench::single(setup = delivered_cycle)]
fn cycle_settle(mut cycle: Cycle) -> Cycle {
    cycle.settle();
    cycle
}

const DEDUP_BATCH: usize = 8192;

fn dedup_hashes() -> Vec<PacketHash> {
    let mut out = Vec::with_capacity(DEDUP_BATCH);
    let mut state = 0x1234_5678_9ABC_DEF0_u64;
    for _ in 0..DEDUP_BATCH {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut word = state;
            word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            word ^= word >> 31;
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out.push(PacketHash::new(bytes));
    }
    out
}

#[library_benchmark]
#[bench::fresh(setup = dedup_hashes)]
fn dedup_remember_fresh(hashes: Vec<PacketHash>) {
    let mut history = HeapPacketHashHistory::default();
    for hash in hashes {
        black_box(history.remember(black_box(hash)));
    }
    black_box(history);
}

const FRAMING_ITERS: usize = 1024;

fn framing_payload() -> Vec<u8> {
    let mut out = Vec::with_capacity(PAYLOAD_LEN);
    let mut state = 0xC0FF_EE00_1234_5678_u64;
    while out.len() < PAYLOAD_LEN {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        for byte in state.to_le_bytes() {
            if out.len() < PAYLOAD_LEN {
                out.push(byte);
            }
        }
    }
    out
}

#[library_benchmark]
#[bench::p300(setup = framing_payload)]
fn framing_encode(payload: Vec<u8>) {
    let mut out = [0u8; rns_serial_framing::max_encoded_len(PAYLOAD_LEN)];
    for _ in 0..FRAMING_ITERS {
        let n =
            rns_serial_framing::encode(black_box(&payload), black_box(&mut out)).expect("encodes");
        black_box(n);
    }
}

fn framed_payload() -> Vec<u8> {
    let payload = framing_payload();
    let mut framed = vec![0u8; rns_serial_framing::max_encoded_len(PAYLOAD_LEN)];
    let n = rns_serial_framing::encode(&payload, &mut framed).expect("encodes");
    framed.truncate(n);
    framed
}

#[library_benchmark]
#[bench::p300(setup = framed_payload)]
fn framing_decode(framed: Vec<u8>) {
    let mut decoded = 0usize;
    for _ in 0..FRAMING_ITERS {
        let mut decoder = rns_serial_framing::RnsSerialDecoder::<512>::new();
        decoder.feed_slice(black_box(&framed), |frame| {
            decoded += frame.len();
        });
    }
    black_box(decoded);
}

const CHANNEL_COUNT: usize = 16;
const IN_FLIGHT: usize = 48;

fn full_window_channels() -> HeapChannelTable {
    let mut table = HeapChannelTable::default();
    for c in 0..CHANNEL_COUNT {
        let index = table
            .ensure(&LinkId::new([c as u8 + 1; 16]))
            .expect("heap table never fills");
        for sub in 0..IN_FLIGHT {
            let sent_at = InstantMillis(1_000 + (sub as u64) * 10 + c as u64);
            let outcome = table.push_outstanding(
                index,
                OutstandingSend {
                    packet_hash: PacketHash::new([sub as u8 + 1; 32]),
                    command_id: CommandId((c * IN_FLIGHT + sub) as u64),
                    sequence: ChannelSequence(sub as u16),
                    message_type: MessageType(0x07),
                    body: b"channel deadline stretch bench body",
                    iv: [sub as u8; 16],
                    sent_at,
                    timeout_at: InstantMillis(sent_at.0 + 9_000),
                },
            );
            assert_eq!(outcome, TxOutcome::Tracked);
        }
    }
    table
}

#[library_benchmark]
#[bench::full_window(setup = full_window_channels)]
fn channel_deadline_stretch(mut table: HeapChannelTable) {
    for index in 0..CHANNEL_COUNT {
        for sub in 0..IN_FLIGHT {
            let current = table.outstanding_timeout_at(index, sub);
            table.set_outstanding_timeout_at(
                black_box(index),
                black_box(sub),
                InstantMillis(current.0 + 250),
            );
        }
        black_box(table.earliest_tx_timeout_at());
    }
    black_box(&table);
}

const FORWARD_BATCH: usize = 64;

fn forward_batch() -> (Forward, Vec<Vec<u8>>) {
    let mut forward = Forward::new();
    let frames = forward.seal_many(FORWARD_BATCH);
    (forward, frames)
}

#[library_benchmark]
#[bench::batch(setup = forward_batch)]
fn relay_forward(input: (Forward, Vec<Vec<u8>>)) {
    let (mut forward, mut frames) = input;
    for frame in frames.iter_mut() {
        black_box(forward.forward_frame(black_box(frame)));
    }
    black_box(&forward);
}

library_benchmark_group!(
    name = primitives;
    benchmarks =
        ed25519_sign_300b,
        ed25519_verify_300b,
        x25519_pubkey,
        x25519_dh,
        token_seal_300b,
        token_open_300b
);

library_benchmark_group!(
    name = engine_cycle;
    benchmarks = cycle_roundtrip, cycle_seal, cycle_deliver_prove, cycle_settle
);

library_benchmark_group!(
    name = dedup;
    benchmarks = dedup_remember_fresh
);

library_benchmark_group!(
    name = framing;
    benchmarks = framing_encode, framing_decode
);

library_benchmark_group!(
    name = forwarding;
    benchmarks = relay_forward
);

library_benchmark_group!(
    name = channel;
    benchmarks = channel_deadline_stretch
);

main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--cache-sim=yes", "--branch-sim=yes"]));
    library_benchmark_groups = primitives, engine_cycle, dedup, framing, forwarding, channel
);
