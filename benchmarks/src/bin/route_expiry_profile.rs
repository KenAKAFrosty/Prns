use std::hint::black_box;
use std::time::Instant;

use personal_rns::engine::InstantMillis;
use personal_rns::routing::{LinearRouteExpiryIndex, RoaringRouteExpiryIndex, RouteExpiryIndex};

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

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

#[derive(Clone)]
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

    fn swap_remove(&mut self, row: usize) {
        self.learned_at.swap_remove(row);
        self.last_relayed_at.swap_remove(row);
        self.interface_slots.swap_remove(row);
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

fn exact_ns<I: RouteExpiryIndex>(index: &I, rows: &RouteRows, iterations: usize) -> f64 {
    let begun = Instant::now();
    let mut sink = 0u64;
    for _ in 0..iterations {
        sink = sink.wrapping_add(
            black_box(index)
                .earliest_exact(black_box(rows.len()), |row| {
                    black_box(rows).expiry(black_box(row))
                })
                .map_or(0, |deadline| deadline.0),
        );
    }
    black_box(sink);
    begun.elapsed().as_secs_f64() * 1e9 / iterations as f64
}

fn linear_cull(mut rows: RouteRows, now: InstantMillis) -> (usize, f64) {
    let begun = Instant::now();
    let mut culled = 0;
    let mut row = 0;
    while row < rows.len() {
        if rows.expiry(row) <= now {
            rows.swap_remove(row);
            culled += 1;
        } else {
            row += 1;
        }
    }
    black_box((0..rows.len()).map(|row| rows.expiry(row)).min());
    (culled, begun.elapsed().as_secs_f64() * 1e3)
}

fn indexed_cull(
    mut rows: RouteRows,
    index: RoaringRouteExpiryIndex,
    now: InstantMillis,
) -> (usize, f64) {
    let begun = Instant::now();
    let mut culled = 0;
    if index.prefers_linear_cull(rows.len(), now) {
        index.invalidate();
        let mut row = 0;
        while row < rows.len() {
            if rows.expiry(row) <= now {
                let last = rows.len() - 1;
                rows.swap_remove(row);
                index.swap_remove(row, last);
                culled += 1;
            } else {
                row += 1;
            }
        }
    } else {
        while let Some(row) =
            index.first_expired(rows.len(), now, |candidate| rows.expiry(candidate))
        {
            let last = rows.len() - 1;
            rows.swap_remove(row);
            index.swap_remove(row, last);
            culled += 1;
        }
    }
    black_box(index.earliest_exact(rows.len(), |row| rows.expiry(row)));
    (culled, begun.elapsed().as_secs_f64() * 1e3)
}

fn profile(count: usize) {
    let rows = RouteRows::new(count);
    let first_bucket = (0..rows.len())
        .map(|row| rows.expiry(row).0 / personal_rns::routing::ROUTE_EXPIRY_QUANTUM_MS)
        .min()
        .unwrap_or(0);
    let first_bucket_candidates = (0..rows.len())
        .filter(|&row| {
            rows.expiry(row).0 / personal_rns::routing::ROUTE_EXPIRY_QUANTUM_MS == first_bucket
        })
        .count();
    let linear = LinearRouteExpiryIndex;
    let linear_ns = exact_ns(&linear, &rows, 50);

    #[cfg(feature = "dhat-heap")]
    let before = dhat::HeapStats::get();
    let build_begun = Instant::now();
    let roaring = build_index::<RoaringRouteExpiryIndex>(&rows);
    let build_ms = build_begun.elapsed().as_secs_f64() * 1e3;
    #[cfg(feature = "dhat-heap")]
    let after = dhat::HeapStats::get();

    let roaring_ns = exact_ns(&roaring, &rows, 20_000);
    let now = InstantMillis(8 * 24 * 60 * 60 * 1_000);
    let (linear_culled, linear_cull_ms) = linear_cull(rows.clone(), now);
    let (indexed_culled, indexed_cull_ms) = indexed_cull(rows, roaring, now);

    assert_eq!(linear_culled, indexed_culled);
    println!(
        "{count:>9} routes  exact linear {linear_ns:>10.1} ns  roaring {roaring_ns:>8.1} ns  first bucket {first_bucket_candidates:>4}  build {build_ms:>8.2} ms"
    );
    println!(
        "           cull+rearm {indexed_culled:>9}  linear {linear_cull_ms:>8.2} ms  roaring {indexed_cull_ms:>8.2} ms"
    );
    #[cfg(feature = "dhat-heap")]
    println!(
        "                 heap {:>9} bytes / {:>7} live allocations  build {:>7} allocations / {:>9} bytes",
        after.curr_bytes - before.curr_bytes,
        after.curr_blocks - before.curr_blocks,
        after.total_blocks - before.total_blocks,
        after.total_bytes - before.total_bytes,
    );
}

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::builder().testing().build();

    for count in ROUTE_COUNTS {
        profile(count);
    }
}
