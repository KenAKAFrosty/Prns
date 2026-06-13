//! The Prns-only microscope under the scenario numbers: two engines driven directly —
//! fixed identities, fixed clock, deterministic entropy, zero I/O — splitting one
//! SINGLE's life into its three acts (initiator seals, responder delivers and proves,
//! initiator verifies and settles), with raw-primitive anchors beneath them so each
//! stage's curve/cipher floor is visible. This is the control baseline optimization
//! work measures against: run `cargo bench` here before and after touching the path.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use pprof::criterion::{Output, PProfProfiler};
use personal_rns::crypto::{
    ed25519_public_key, ed25519_sign, ed25519_verify, token_open, token_seal,
    x25519_diffie_hellman, x25519_public_key, Ed25519SecretKey, TokenKey, X25519SecretKey,
};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Directive, EngineCommand,
    EngineReaction, EngineState, InstantMillis, IssuedCommand, Journaled, RatchetPolicy,
    SendSingle, SendSinglePayload, Settlement,
};
use personal_rns::identity::{Zeroizing, ENCRYPTION_IV_LEN, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{InboundPacket, InterfaceConfig, InterfaceId};
use personal_rns::reactor::interfaces::tcp::core as tcp_core;
use personal_rns::routing::announce::defaults::JitterSeed;
use personal_rns::routing::delivery::Delivery;
use personal_rns::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;

const WIRE: InterfaceId = InterfaceId::new([0xC7; 16]);
const NOW: InstantMillis = InstantMillis(1_000);
const JITTER: JitterSeed = JitterSeed(7);
const PAYLOAD_LEN: usize = 300;

/// Deterministic entropy (splitmix64): every run pulls the identical stream, so a
/// measured difference between runs is the code, never the keys.
struct Splitmix(u64);

impl Splitmix {
    fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(8) {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut word = self.0;
            word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            word ^= word >> 31;
            chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
        }
    }
}

/// One complete SINGLE round trip held as three replayable stages over two live
/// engines. Every receipt settles within its own cycle, so engine state stays
/// bounded no matter how many iterations the harness runs.
struct Cycle {
    initiator: EngineState<GrowableHeap>,
    responder: EngineState<GrowableHeap>,
    initiator_entropy: Splitmix,
    responder_entropy: Splitmix,
    interfaces: Vec<InterfaceConfig>,
    destination: DestinationHash,
    payload: [u8; PAYLOAD_LEN],
    next_id: u64,
    sealed: Vec<u8>,
    proof: Vec<u8>,
}

impl Cycle {
    fn new() -> Self {
        let mut responder =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x11; IDENTITY_SECRET_KEY_LEN]));
        let responder_identity = responder.held_identity_hashes()[0];
        let destination = responder
            .register_single_destination(
                &responder_identity,
                "bench",
                &["cycle"],
                b"",
                ProofStrategy::ProveAll,
                RatchetPolicy::NoRatchets,
            )
            .expect("registers the bench destination");
        let initiator =
            EngineState::<GrowableHeap>::new(Zeroizing::new([0x22; IDENTITY_SECRET_KEY_LEN]));
        let interfaces = vec![tcp_core::descriptor(WIRE, tcp_core::TCP_BITRATE_GUESS_BPS)];

        let mut cycle = Self {
            initiator,
            responder,
            initiator_entropy: Splitmix(1),
            responder_entropy: Splitmix(2),
            interfaces,
            destination,
            payload: [0xAB; PAYLOAD_LEN],
            next_id: 1,
            sealed: Vec::with_capacity(1024),
            proof: Vec::with_capacity(1024),
        };

        let mut announce = Vec::with_capacity(1024);
        let issued = IssuedCommand {
            id: CommandId(0),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };
        cycle.responder.ingest_command_into(
            issued,
            &cycle.interfaces,
            NOW,
            &mut |bytes| cycle.responder_entropy.fill(bytes),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    announce.extend_from_slice(bytes);
                }
            },
        );
        assert!(!announce.is_empty(), "responder emitted its announce");

        let mut heard = false;
        cycle.initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: &mut announce,
            },
            JITTER,
            &cycle.interfaces,
            NOW,
            &mut |bytes| cycle.initiator_entropy.fill(bytes),
            &mut |_| true,
            &mut |reaction| {
                if matches!(
                    reaction,
                    EngineReaction::Journaled(Journaled::AnnounceHeard { .. })
                ) {
                    heard = true;
                }
            },
        );
        assert!(heard, "initiator learned the destination");
        cycle
    }

    /// Act one, on the initiator: ephemeral X25519 keygen + DH, HKDF, token seal,
    /// receipt registration — everything `SendSingle` costs before the wire.
    fn seal(&mut self) {
        let issued = IssuedCommand {
            id: CommandId(self.next_id),
            command: EngineCommand::SendSingle(SendSingle {
                destination: self.destination,
                payload: SendSinglePayload::from_slice(&self.payload).expect("payload fits"),
            }),
        };
        self.next_id += 1;
        let Self {
            initiator,
            initiator_entropy,
            interfaces,
            sealed,
            ..
        } = self;
        sealed.clear();
        initiator.ingest_command_into(
            issued,
            interfaces,
            NOW,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::Send { bytes, .. }) = reaction {
                    sealed.extend_from_slice(bytes);
                }
            },
        );
        assert!(!self.sealed.is_empty(), "send sealed a frame");
    }

    /// Act two, on the responder: in-place DH + HKDF + token open, then the implicit
    /// proof — an Ed25519 sign — back the way the packet came.
    fn deliver_prove(&mut self) {
        let mut delivered = false;
        let Self {
            responder,
            responder_entropy,
            interfaces,
            sealed,
            proof,
            ..
        } = self;
        proof.clear();
        responder.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: sealed,
            },
            JITTER,
            interfaces,
            NOW,
            &mut |bytes| responder_entropy.fill(bytes),
            &mut |_| true,
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::Delivered(Delivery::Single(_))) => {
                    delivered = true;
                }
                EngineReaction::Directive(Directive::Send { bytes, .. }) => {
                    proof.extend_from_slice(bytes);
                }
                _ => {}
            },
        );
        assert!(delivered, "responder delivered the single");
        assert!(!self.proof.is_empty(), "responder proved the single");
    }

    /// Act three, on the initiator: Ed25519 verify against the announced identity,
    /// and the receipt settles `Delivered`.
    fn settle(&mut self) {
        let mut proof = core::mem::take(&mut self.proof);
        self.settle_frame(&mut proof);
        self.proof = proof;
    }

    fn settle_frame(&mut self, proof: &mut [u8]) {
        let mut settled = false;
        let Self {
            initiator,
            initiator_entropy,
            interfaces,
            ..
        } = self;
        initiator.ingest_packet_into(
            InboundPacket {
                arrived_at: NOW,
                source_interface: WIRE,
                bytes: proof,
            },
            JITTER,
            interfaces,
            NOW,
            &mut |bytes| initiator_entropy.fill(bytes),
            &mut |_| true,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled {
                    settlement: Settlement::SendSingle(Ok(_)),
                    ..
                }) = reaction
                {
                    settled = true;
                }
            },
        );
        assert!(settled, "proof verified and the receipt settled");
    }
}

fn single_cycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_cycle");
    group.throughput(Throughput::Elements(1));

    group.bench_function("roundtrip", |b| {
        let mut cycle = Cycle::new();
        b.iter(|| {
            cycle.seal();
            cycle.deliver_prove();
            cycle.settle();
        });
    });

    group.bench_function("initiator_seal", |b| {
        let mut cycle = Cycle::new();
        b.iter_custom(|iters| {
            let mut in_stage = Duration::ZERO;
            for _ in 0..iters {
                let begun = Instant::now();
                cycle.seal();
                in_stage += begun.elapsed();
                cycle.deliver_prove();
                cycle.settle();
            }
            in_stage
        });
    });

    group.bench_function("responder_deliver_prove", |b| {
        let mut cycle = Cycle::new();
        b.iter_custom(|iters| {
            let mut in_stage = Duration::ZERO;
            for _ in 0..iters {
                cycle.seal();
                let begun = Instant::now();
                cycle.deliver_prove();
                in_stage += begun.elapsed();
                cycle.settle();
            }
            in_stage
        });
    });

    group.bench_function("initiator_verify_settle", |b| {
        let mut cycle = Cycle::new();
        b.iter_custom(|iters| {
            let mut in_stage = Duration::ZERO;
            for _ in 0..iters {
                cycle.seal();
                cycle.deliver_prove();
                let begun = Instant::now();
                cycle.settle();
                in_stage += begun.elapsed();
            }
            in_stage
        });
    });
    group.finish();
}

/// The settle stage with `depth` receipts outstanding — the live initiator's true
/// position, where window-deep traffic keeps the receipt table populated. An implicit
/// proof names no row, so the engine trial-verifies until one matches (reference
/// parity); what this group measures is how many full verifies that trial order costs.
fn settle_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("settle_depth");
    for depth in [1usize, 8, 16] {
        group.throughput(Throughput::Elements(depth as u64));
        group.bench_function(BenchmarkId::from_parameter(depth), |b| {
            let mut cycle = Cycle::new();
            b.iter_custom(|iters| {
                let mut in_stage = Duration::ZERO;
                for _ in 0..iters {
                    let mut proofs: Vec<Vec<u8>> = Vec::with_capacity(depth);
                    for _ in 0..depth {
                        cycle.seal();
                        cycle.deliver_prove();
                        proofs.push(cycle.proof.clone());
                    }
                    let begun = Instant::now();
                    for proof in &mut proofs {
                        cycle.settle_frame(proof);
                    }
                    in_stage += begun.elapsed();
                }
                in_stage
            })
        });
    }
    group.finish();
}

fn primitives(c: &mut Criterion) {
    let mut group = c.benchmark_group("primitives");

    let signer = Ed25519SecretKey::new([0x42; 32]);
    let verifier = ed25519_public_key(&signer);
    let message = [0xAB_u8; 32];
    let signature = ed25519_sign(&signer, &message);
    group.bench_function("ed25519_sign", |b| {
        b.iter(|| ed25519_sign(black_box(&signer), black_box(&message)))
    });
    group.bench_function("ed25519_verify", |b| {
        b.iter(|| {
            ed25519_verify(
                black_box(&verifier),
                black_box(&message),
                black_box(&signature),
            )
            .expect("authentic")
        })
    });

    let ours = X25519SecretKey::new([0x11; 32]);
    let theirs = x25519_public_key(&X25519SecretKey::new([0x33; 32]));
    group.bench_function("x25519_public_key", |b| {
        b.iter(|| x25519_public_key(black_box(&ours)))
    });
    group.bench_function("x25519_diffie_hellman", |b| {
        b.iter(|| x25519_diffie_hellman(black_box(&ours), black_box(&theirs)))
    });

    let derived = [0x5A_u8; 64];
    let key = TokenKey::from_derived(&derived).expect("64-byte derived key");
    let iv = [0x77_u8; ENCRYPTION_IV_LEN];
    let plaintext = [0xAB_u8; PAYLOAD_LEN];
    let mut sealed = [0u8; 512];
    let sealed_len = token_seal(&key, &iv, &plaintext, &mut sealed).expect("seals");
    group.bench_function("token_seal_300B", |b| {
        let mut out = [0u8; 512];
        b.iter(|| {
            token_seal(
                black_box(&key),
                black_box(&iv),
                black_box(&plaintext),
                &mut out,
            )
            .expect("seals")
        })
    });
    group.bench_function("token_open_300B", |b| {
        let mut out = [0u8; 512];
        b.iter(|| {
            token_open(black_box(&key), black_box(&sealed[..sealed_len]), &mut out).expect("opens")
        })
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = single_cycle, settle_depth, primitives
}
criterion_main!(benches);
