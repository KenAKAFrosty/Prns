use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use personal_rns::engine::InstantMillis;
use personal_rns::routing::{LinearRouteExpiryIndex, RoaringRouteExpiryIndex, RouteExpiryIndex};

const ROUTE_COUNTS: [usize; 3] = [10_000, 100_000, 1_000_000];
const ROUTE_LIFETIMES: [u64; 8] = [
    6 * 60 * 60 * 1_000,
    24 * 60 * 60 * 1_000,
    7 * 24 * 60 * 60 * 1_000,
    7 * 24 * 60 * 60 * 1_000,
    6 * 60 * 60 * 1_000,
    24 * 60 * 60 * 1_000,
    7 * 24 * 60 * 60 * 1_000,
    7 * 24 * 60 * 60 * 1_000,
];

struct RouteRows {
    learned_at: Vec<u64>,
    last_relayed_at: Vec<u64>,
    interface_slots: Vec<u8>,
}

impl RouteRows {
    fn new(count: usize) -> Self {
        let spread = 7 * 24 * 60 * 60 * 1_000u64;
        let mut learned_at = Vec::with_capacity(count);
        let mut last_relayed_at = Vec::with_capacity(count);
        let mut interface_slots = Vec::with_capacity(count);
        for row in 0..count {
            let mixed = mix(row as u64);
            let activity = mix(mixed ^ 0xA076_1D64_78BD_642F);
            let learned = mixed % spread;
            learned_at.push(learned);
            last_relayed_at.push(if activity & 3 == 0 {
                learned.saturating_add((activity >> 17) % (2 * 60 * 60 * 1_000))
            } else {
                0
            });
            interface_slots.push((mixed as u8) & 7);
        }
        Self {
            learned_at,
            last_relayed_at,
            interface_slots,
        }
    }

    fn len(&self) -> usize {
        self.learned_at.len()
    }

    fn expiry(&self, row: usize) -> InstantMillis {
        let active = self.learned_at[row].max(self.last_relayed_at[row]);
        InstantMillis(active.saturating_add(ROUTE_LIFETIMES[self.interface_slots[row] as usize]))
    }
}

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn build_index<I: RouteExpiryIndex>(rows: &RouteRows) -> I {
    let index = I::default();
    for row in 0..rows.len() {
        index.insert(row, rows.expiry(row));
    }
    index
}

fn route_expiry(c: &mut Criterion) {
    let mut exact = c.benchmark_group("route_expiry_exact_next");
    for count in ROUTE_COUNTS {
        let rows = RouteRows::new(count);
        let linear = build_index::<LinearRouteExpiryIndex>(&rows);
        let roaring = build_index::<RoaringRouteExpiryIndex>(&rows);
        exact.bench_with_input(BenchmarkId::new("linear", count), &count, |b, _| {
            b.iter(|| {
                black_box(linear.earliest_exact(rows.len(), |row| rows.expiry(black_box(row))))
            })
        });
        exact.bench_with_input(BenchmarkId::new("roaring_5m", count), &count, |b, _| {
            b.iter(|| {
                black_box(roaring.earliest_exact(rows.len(), |row| rows.expiry(black_box(row))))
            })
        });
    }
    exact.finish();

    let mut update = c.benchmark_group("route_expiry_relay_update");
    for count in ROUTE_COUNTS {
        let mut rows = RouteRows::new(count);
        let roaring = build_index::<RoaringRouteExpiryIndex>(&rows);
        let mut iteration = 0usize;
        update.bench_with_input(BenchmarkId::new("roaring_5m", count), &count, |b, _| {
            b.iter(|| {
                let row = iteration % rows.len();
                iteration = iteration.wrapping_add(1);
                rows.last_relayed_at[row] = rows.last_relayed_at[row].wrapping_add(300_001);
                roaring.update(row, rows.expiry(row));
                black_box(rows.last_relayed_at[row])
            })
        });
    }
    update.finish();

    let mut rebuild = c.benchmark_group("route_expiry_rebuild");
    for count in ROUTE_COUNTS {
        let rows = RouteRows::new(count);
        let roaring = build_index::<RoaringRouteExpiryIndex>(&rows);
        rebuild.bench_with_input(BenchmarkId::new("roaring_5m", count), &count, |b, _| {
            b.iter(|| {
                roaring.invalidate();
                black_box(roaring.earliest_exact(rows.len(), |row| rows.expiry(row)))
            })
        });
    }
    rebuild.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_millis(1500))
        .warm_up_time(Duration::from_millis(400));
    targets = route_expiry
}
criterion_main!(benches);
