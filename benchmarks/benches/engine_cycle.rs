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
use personal_rns::identity::ENCRYPTION_IV_LEN;

use benchmarks::microscope::{Cycle, PAYLOAD_LEN};

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
