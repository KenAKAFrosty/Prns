//! Throughput + latency axis (criterion): time the engine ingesting a batch of
//! distinct announces. Same `ingest_and_settle` scenario the memory bins use — only
//! the measurement backend differs (criterion's timer here, dhat there).
//!
//! Run: `cargo bench`

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use benchmarks::{announce_fixtures, ingest_and_settle};
use personal_rns::routing::storage::GrowableHeap;

fn ingest(c: &mut Criterion) {
    let announces = announce_fixtures(256);
    c.bench_function("ingest_256_announces/growable-heap", |b| {
        b.iter_batched(
            || announces.clone(),
            |mut packets| ingest_and_settle::<GrowableHeap>(&mut packets, 0),
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, ingest);
criterion_main!(benches);
