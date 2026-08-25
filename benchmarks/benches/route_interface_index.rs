use std::hint::black_box;
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion};

use personal_rns::engine::InstantMillis;
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::routes::{
    LinearHeapRouteTable, RoaringHeapRouteTable, RouteEntry, RouteEvidenceId, RouteTable,
};
use personal_rns::routing::{NextHop, RouteResponsiveness};
use personal_rns::wire::DestinationHash;

fn destination(row: u32) -> DestinationHash {
    let key = (row as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&key.to_be_bytes());
    bytes[8..12].copy_from_slice(&row.to_be_bytes());
    DestinationHash::new(bytes)
}

fn evidence(row: u32) -> RouteEvidenceId {
    RouteEvidenceId::new(row + 1).unwrap()
}

fn route(receiving_interface: InterfaceId) -> RouteEntry {
    RouteEntry {
        hops: 1,
        learned_at: InstantMillis(0),
        last_route_activity_at: InstantMillis(0),
        responsiveness: RouteResponsiveness::Responsive,
        receiving_interface,
        next_hop: NextHop::Direct,
    }
}

fn heap_routes<T: RouteTable + Default>(routes: usize, matching_every: Option<usize>) -> T {
    let mut table = T::default();
    for row in 0..routes {
        let receiving_interface = match matching_every {
            Some(every) if row.is_multiple_of(every) => InterfaceId::new([0xA1; 8]),
            Some(_) | None => InterfaceId::new([0xC3; 8]),
        };
        table
            .push(
                destination(row as u32),
                evidence(row as u32),
                route(receiving_interface),
            )
            .unwrap();
    }
    table
}

fn bench_repoint_no_match(group: &mut BenchmarkGroup<'_, WallTime>, routes: usize) {
    let mut linear = heap_routes::<LinearHeapRouteTable>(routes, None);
    let mut roaring = heap_routes::<RoaringHeapRouteTable>(routes, None);
    let previous = InterfaceId::new([0xE1; 8]);
    let current = InterfaceId::new([0xE2; 8]);
    let populated = InterfaceId::new([0xC3; 8]);
    assert_eq!(linear.route_count_via(populated), routes);
    assert_eq!(roaring.route_count_via(populated), routes);
    assert_eq!(linear.route_count_via(previous), 0);
    assert_eq!(roaring.route_count_via(previous), 0);

    group.bench_with_input(BenchmarkId::new("linear", routes), &routes, |b, _| {
        b.iter(|| {
            black_box(linear.repoint_receiving_interface(
                black_box(previous),
                black_box(current),
                black_box(InstantMillis(1)),
            ))
        })
    });
    group.bench_with_input(BenchmarkId::new("roaring", routes), &routes, |b, _| {
        b.iter(|| {
            black_box(roaring.repoint_receiving_interface(
                black_box(previous),
                black_box(current),
                black_box(InstantMillis(1)),
            ))
        })
    });
}

fn bench_repoint_sparse(group: &mut BenchmarkGroup<'_, WallTime>, routes: usize) {
    let mut linear = heap_routes::<LinearHeapRouteTable>(routes, Some(1_024));
    let mut roaring = heap_routes::<RoaringHeapRouteTable>(routes, Some(1_024));
    let mut linear_previous = InterfaceId::new([0xA1; 8]);
    let mut linear_current = InterfaceId::new([0xB2; 8]);
    let mut roaring_previous = linear_previous;
    let mut roaring_current = linear_current;
    let expected = routes.div_ceil(1_024);
    assert_eq!(linear.route_count_via(linear_previous), expected);
    assert_eq!(roaring.route_count_via(roaring_previous), expected);

    group.bench_with_input(BenchmarkId::new("linear", routes), &routes, |b, _| {
        b.iter(|| {
            let moved = linear.repoint_receiving_interface(
                black_box(linear_previous),
                black_box(linear_current),
                black_box(InstantMillis(1)),
            );
            core::mem::swap(&mut linear_previous, &mut linear_current);
            black_box(moved)
        })
    });
    group.bench_with_input(BenchmarkId::new("roaring", routes), &routes, |b, _| {
        b.iter(|| {
            let moved = roaring.repoint_receiving_interface(
                black_box(roaring_previous),
                black_box(roaring_current),
                black_box(InstantMillis(1)),
            );
            core::mem::swap(&mut roaring_previous, &mut roaring_current);
            black_box(moved)
        })
    });
}

fn route_repoint(c: &mut Criterion) {
    let mut no_match = c.benchmark_group("route_repoint_no_match");
    for routes in [1_000, 100_000, 1_000_000] {
        bench_repoint_no_match(&mut no_match, routes);
    }
    no_match.finish();

    let mut sparse = c.benchmark_group("route_repoint_sparse");
    for routes in [1_000, 100_000, 1_000_000] {
        bench_repoint_sparse(&mut sparse, routes);
    }
    sparse.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_millis(1500))
        .warm_up_time(Duration::from_millis(400));
    targets = route_repoint
}
criterion_main!(benches);
