//! The route-lookup crossover: linear-scan columns vs. the Lemire-indexed columns,
//! `index_of` for a present key (hit) and an absent key (miss), across table sizes.
//! Wall-clock on purpose — the small-N regime is decided by cache locality, which an
//! instruction count cannot see. These are host numbers; the on-device S3 crossover
//! (tiny caches, PSRAM) will sit lower, but the shape transfers.

use std::hint::black_box;
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion};

use personal_rns::engine::InstantMillis;
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::routes::{
    route_index_buckets, FixedArrayRouteTable, FixedIndexedRouteTable, RouteEntry, RouteEvidenceId,
    RouteTable,
};
use personal_rns::routing::{NextHop, RouteResponsiveness};
use personal_rns::wire::DestinationHash;

fn dest_n(n: u32) -> DestinationHash {
    let key = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&key.to_be_bytes());
    b[8..12].copy_from_slice(&n.to_be_bytes());
    DestinationHash::new(b)
}

fn evidence(n: u32) -> RouteEvidenceId {
    RouteEvidenceId::new(n + 1).unwrap()
}

fn route_row() -> RouteEntry {
    RouteEntry {
        hops: 1,
        learned_at: InstantMillis(0),
        last_route_activity_at: InstantMillis(0),
        responsiveness: RouteResponsiveness::Responsive,
        receiving_interface: InterfaceId::new([0u8; 8]),
        next_hop: NextHop::Direct,
    }
}

fn bench_mode<const N: usize, const B: usize>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    lookup: DestinationHash,
) {
    let mut linear = FixedArrayRouteTable::<N>::default();
    let mut indexed = FixedIndexedRouteTable::<N, B>::default();
    for i in 0..N as u32 {
        linear.push(dest_n(i), evidence(i), route_row()).unwrap();
        indexed.push(dest_n(i), evidence(i), route_row()).unwrap();
    }

    group.bench_with_input(BenchmarkId::new("linear", N), &N, |b, _| {
        b.iter(|| black_box(linear.index_of(black_box(&lookup))))
    });
    group.bench_with_input(BenchmarkId::new("indexed", N), &N, |b, _| {
        b.iter(|| black_box(indexed.index_of(black_box(&lookup))))
    });
}

fn route_lookup(c: &mut Criterion) {
    let mut hit = c.benchmark_group("route_lookup_hit");
    bench_mode::<16, { route_index_buckets(16) }>(&mut hit, dest_n(8));
    bench_mode::<32, { route_index_buckets(32) }>(&mut hit, dest_n(16));
    bench_mode::<64, { route_index_buckets(64) }>(&mut hit, dest_n(32));
    bench_mode::<128, { route_index_buckets(128) }>(&mut hit, dest_n(64));
    bench_mode::<256, { route_index_buckets(256) }>(&mut hit, dest_n(128));
    bench_mode::<1024, { route_index_buckets(1024) }>(&mut hit, dest_n(512));
    hit.finish();

    let mut miss = c.benchmark_group("route_lookup_miss");
    bench_mode::<16, { route_index_buckets(16) }>(&mut miss, dest_n(1_000_016));
    bench_mode::<32, { route_index_buckets(32) }>(&mut miss, dest_n(1_000_032));
    bench_mode::<64, { route_index_buckets(64) }>(&mut miss, dest_n(1_000_064));
    bench_mode::<128, { route_index_buckets(128) }>(&mut miss, dest_n(1_000_128));
    bench_mode::<256, { route_index_buckets(256) }>(&mut miss, dest_n(1_000_256));
    bench_mode::<1024, { route_index_buckets(1024) }>(&mut miss, dest_n(1_001_024));
    miss.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_millis(1500))
        .warm_up_time(Duration::from_millis(400));
    targets = route_lookup
}
criterion_main!(benches);
