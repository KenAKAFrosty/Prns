use super::*;
use core::hint::black_box;
use core::mem::size_of;
use std::collections::BTreeMap;
use std::time::Instant;

fn deadlines(values: &[Option<u64>]) -> Vec<Option<InstantMillis>> {
    values
        .iter()
        .map(|value| value.map(InstantMillis))
        .collect()
}

#[test]
fn exact_queries_only_walk_the_first_populated_bucket() {
    let mut index = HeapTemporalIndex::<300_000>::default();
    let values = deadlines(&[
        Some(599_000),
        None,
        Some(301_000),
        Some(300_500),
        Some(900_000),
    ]);
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }
    assert_eq!(
        index.earliest_exact(values.len(), |row| values[row]),
        Some(InstantMillis(300_500))
    );
    assert_eq!(
        index.first_due(values.len(), InstantMillis(300_750), |row| values[row]),
        Some(3)
    );
    assert_eq!(
        index.first_due(values.len(), InstantMillis(300_499), |row| values[row]),
        None
    );
}

#[test]
fn optional_updates_and_swap_removals_keep_row_ids_aligned() {
    let mut index = HeapTemporalIndex::<1_000>::default();
    let mut values = deadlines(&[Some(1_000), None, Some(3_000), Some(4_000)]);
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }

    values[0] = None;
    index.update(0, None);
    values[1] = Some(InstantMillis(2_000));
    index.update(1, values[1]);
    assert_eq!(
        index.earliest_exact(values.len(), |row| values[row]),
        Some(InstantMillis(2_000))
    );

    let last = values.len() - 1;
    values.swap_remove(1);
    index.swap_remove(1, last);
    assert_eq!(
        index.earliest_exact(values.len(), |row| values[row]),
        Some(InstantMillis(3_000))
    );
}

#[test]
fn invalidation_rebuilds_and_unrepresentable_deadlines_scan_exactly() {
    let mut index = HeapTemporalIndex::<1>::default();
    let mut values = deadlines(&[Some(100), Some(200)]);
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }
    values[1] = Some(InstantMillis(1));
    index.invalidate();
    assert_eq!(
        index.earliest_exact(values.len(), |row| values[row]),
        Some(InstantMillis(1))
    );

    values[0] = Some(InstantMillis(u64::from(u32::MAX) + 1));
    index.invalidate();
    assert_eq!(
        index.earliest_exact(values.len(), |row| values[row]),
        Some(InstantMillis(1))
    );
}

#[test]
fn dense_due_sets_choose_the_scan_path() {
    let mut index = HeapTemporalIndex::<1_000>::default();
    let values = vec![Some(InstantMillis(1)); 5_000];
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }
    assert!(index.prefers_linear_cull(values.len(), InstantMillis(1)));
    index.invalidate();
    assert!(index.prefers_linear_cull(values.len(), InstantMillis(0)));
}

#[test]
fn matching_queries_continue_across_due_buckets() {
    let mut index = HeapTemporalIndex::<1_000>::default();
    let values = deadlines(&[Some(100), Some(1_100), Some(2_100)]);
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }
    assert_eq!(
        index.first_due_matching(
            values.len(),
            InstantMillis(2_500),
            |row| values[row],
            |row| row == 2,
        ),
        Some(2)
    );
}

#[test]
fn randomized_mutations_match_a_linear_oracle() {
    let mut index = HeapTemporalIndex::<1_000>::default();
    let mut values = (0..1_000u64)
        .map(|row| (row % 5 != 0).then(|| InstantMillis(mix(row) % 2_000_000_000)))
        .collect::<Vec<_>>();
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }

    for step in 0..10_000u64 {
        let mixed = mix(step + 10_000);
        match mixed % 5 {
            0 if !values.is_empty() => {
                let row = mixed as usize % values.len();
                values[row] = (mixed & 8 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                index.update(row, values[row]);
            }
            1 if values.len() > 1 => {
                let row = mixed as usize % values.len();
                let last = values.len() - 1;
                values.swap_remove(row);
                index.swap_remove(row, last);
            }
            2 => {
                let deadline = (mixed & 16 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                index.insert(values.len(), deadline);
                values.push(deadline);
            }
            3 => index.invalidate(),
            _ => {}
        }

        assert_eq!(
            index.earliest_exact(values.len(), |row| values[row]),
            values.iter().flatten().copied().min()
        );
        let now = InstantMillis(mix(mixed + 1) % 2_000_000_000);
        let candidate = index.first_due(values.len(), now, |row| values[row]);
        assert_eq!(
            candidate.is_some(),
            values.iter().flatten().any(|deadline| *deadline <= now)
        );
        if let Some(row) = candidate {
            assert!(values[row].is_some_and(|deadline| deadline <= now));
        }
    }
}

#[test]
fn randomized_deadline_heap_mutations_match_a_linear_oracle() {
    let mut index = HeapDeadlineIndex::default();
    let mut values = (0..1_000u64)
        .map(|row| (row % 5 != 0).then(|| InstantMillis(mix(row) % 2_000_000_000)))
        .collect::<Vec<_>>();
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline, |row| values[row]);
    }

    for step in 0..10_000u64 {
        let mixed = mix(step + 30_000);
        match mixed % 4 {
            0 if !values.is_empty() => {
                let row = mixed as usize % values.len();
                values[row] = (mixed & 8 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                index.update(row, values[row], |row| values[row]);
            }
            1 if values.len() > 1 => {
                let row = mixed as usize % values.len();
                let last = values.len() - 1;
                index.swap_remove(row, last, |row| values[row]);
                values.swap_remove(row);
            }
            2 => {
                let deadline = (mixed & 16 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                values.push(deadline);
                index.insert(values.len() - 1, deadline, |row| values[row]);
            }
            _ => index.invalidate(),
        }

        assert_eq!(
            index.earliest_exact(values.len(), |row| values[row]),
            values.iter().flatten().copied().min()
        );
        let now = InstantMillis(mix(mixed + 1) % 2_000_000_000);
        let candidate = index.first_due(values.len(), now, |row| values[row]);
        assert_eq!(
            candidate.is_some(),
            values.iter().flatten().any(|deadline| *deadline <= now)
        );
        if let Some(row) = candidate {
            assert!(values[row].is_some_and(|deadline| deadline <= now));
        }
        for (position, row) in index.heap.iter().copied().enumerate() {
            assert_eq!(index.positions[row as usize], position as u32);
            if position > 0 {
                let parent = index.heap[(position - 1) / 2] as usize;
                assert!(values[parent] <= values[row as usize]);
            }
        }
    }
}

#[test]
fn deadline_heap_selects_linear_culls_only_for_dense_due_sets() {
    let mut index = HeapDeadlineIndex::default();
    let dense = vec![Some(InstantMillis(1)); 100_000];
    for (row, deadline) in dense.iter().copied().enumerate() {
        index.insert(row, deadline, |row| dense[row]);
    }
    assert!(index.prefers_linear_cull(dense.len(), InstantMillis(1), |row| dense[row]));
    assert!(!index.prefers_linear_cull(dense.len(), InstantMillis(0), |row| dense[row]));

    let sparse = (0..100_000)
        .map(|row| Some(InstantMillis((row >= 1_000) as u64 + 1)))
        .collect::<Vec<_>>();
    index.invalidate();
    assert!(!index.prefers_linear_cull(sparse.len(), InstantMillis(1), |row| sparse[row]));
}

#[derive(Clone, Copy)]
struct WheelLocation {
    bucket: u64,
    position: u32,
}

struct BucketedDeadlineWheel<const QUANTUM_MS: u64> {
    buckets: BTreeMap<u64, Vec<u32>>,
    locations: Vec<Option<WheelLocation>>,
}

impl<const QUANTUM_MS: u64> BucketedDeadlineWheel<QUANTUM_MS> {
    fn new() -> Self {
        Self {
            buckets: BTreeMap::new(),
            locations: Vec::new(),
        }
    }

    fn bucket(deadline: InstantMillis) -> u64 {
        deadline.0 / QUANTUM_MS
    }

    fn insert(&mut self, row: usize, deadline: Option<InstantMillis>) {
        self.locations.push(None);
        let Some(deadline) = deadline else {
            return;
        };
        let bucket = Self::bucket(deadline);
        let rows = self.buckets.entry(bucket).or_default();
        let position = rows.len() as u32;
        rows.push(row as u32);
        self.locations[row] = Some(WheelLocation { bucket, position });
    }

    fn remove(&mut self, row: usize) {
        let Some(location) = self.locations[row].take() else {
            return;
        };
        let empty = {
            let rows = self.buckets.get_mut(&location.bucket).unwrap();
            let position = location.position as usize;
            let moved = (position + 1 < rows.len()).then(|| *rows.last().unwrap());
            rows.swap_remove(position);
            if let Some(moved) = moved {
                self.locations[moved as usize].as_mut().unwrap().position = location.position;
            }
            rows.is_empty()
        };
        if empty {
            self.buckets.remove(&location.bucket);
        }
    }

    fn update(&mut self, row: usize, deadline: Option<InstantMillis>) {
        let next_bucket = deadline.map(Self::bucket);
        if self.locations[row].map(|location| location.bucket) == next_bucket {
            return;
        }
        self.remove(row);
        let Some(bucket) = next_bucket else {
            return;
        };
        let rows = self.buckets.entry(bucket).or_default();
        let position = rows.len() as u32;
        rows.push(row as u32);
        self.locations[row] = Some(WheelLocation { bucket, position });
    }

    fn swap_remove(&mut self, removed: usize, last: usize) {
        self.remove(removed);
        if removed != last {
            let moved = self.locations[last];
            self.locations[removed] = moved;
            if let Some(moved) = moved {
                self.buckets.get_mut(&moved.bucket).unwrap()[moved.position as usize] =
                    removed as u32;
            }
        }
        self.locations.pop();
    }

    fn earliest(&self, values: &[Option<InstantMillis>]) -> Option<InstantMillis> {
        self.buckets
            .first_key_value()
            .and_then(|(_, rows)| rows.iter().filter_map(|row| values[*row as usize]).min())
    }

    fn storage_bytes(&self) -> usize {
        self.locations.capacity() * size_of::<Option<WheelLocation>>()
            + self
                .buckets
                .values()
                .map(|rows| rows.capacity() * size_of::<u32>())
                .sum::<usize>()
    }
}

#[test]
fn randomized_bucketed_wheel_mutations_match_a_linear_oracle() {
    let mut index = BucketedDeadlineWheel::<100>::new();
    let mut values = (0..1_000u64)
        .map(|row| (row % 5 != 0).then(|| InstantMillis(mix(row) % 2_000_000_000)))
        .collect::<Vec<_>>();
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }

    for step in 0..10_000u64 {
        let mixed = mix(step + 50_000);
        match mixed % 3 {
            0 if !values.is_empty() => {
                let row = mixed as usize % values.len();
                values[row] = (mixed & 8 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                index.update(row, values[row]);
            }
            1 if values.len() > 1 => {
                let row = mixed as usize % values.len();
                let last = values.len() - 1;
                index.swap_remove(row, last);
                values.swap_remove(row);
            }
            _ => {
                let deadline = (mixed & 16 != 0).then(|| InstantMillis(mix(mixed) % 2_000_000_000));
                values.push(deadline);
                index.insert(values.len() - 1, deadline);
            }
        }
        assert_eq!(
            index.earliest(&values),
            values.iter().flatten().copied().min()
        );
    }
}

fn profile_values(rows: usize, horizon_ms: u64) -> Vec<Option<InstantMillis>> {
    (0..rows)
        .map(|row| {
            let mixed = mix(row as u64);
            Some(InstantMillis(1_000_000 + mixed % horizon_ms))
        })
        .collect()
}

fn profile_roaring<const QUANTUM_MS: u64>(
    label: &str,
    rows: usize,
    horizon_ms: u64,
    query_iterations: usize,
    mutation_iterations: usize,
) {
    let mut values = profile_values(rows, horizon_ms);
    let build_started = Instant::now();
    let mut index = HeapTemporalIndex::<QUANTUM_MS>::default();
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }
    let build = build_started.elapsed();

    let query_started = Instant::now();
    for _ in 0..query_iterations {
        black_box(index.earliest_exact(values.len(), |row| values[row]));
    }
    let query = query_started.elapsed().as_nanos() as f64 / query_iterations as f64;

    let update_started = Instant::now();
    for step in 0..mutation_iterations {
        let row = mix(step as u64) as usize % values.len();
        let previous = values[row].unwrap();
        let bucket = previous.0 / QUANTUM_MS;
        let deadline = InstantMillis(
            bucket
                .saturating_add((step & 1) as u64)
                .saturating_mul(QUANTUM_MS)
                .saturating_add(QUANTUM_MS / 2),
        );
        values[row] = Some(deadline);
        index.update(row, Some(deadline));
    }
    let update = update_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;

    let churn_started = Instant::now();
    for step in 0..mutation_iterations {
        let removed = mix((step + mutation_iterations) as u64) as usize % values.len();
        let last = values.len() - 1;
        values.swap_remove(removed);
        index.swap_remove(removed, last);
        let deadline = Some(InstantMillis(
            1_000_000 + mix((step + rows) as u64) % horizon_ms,
        ));
        index.insert(values.len(), deadline);
        values.push(deadline);
    }
    let churn = churn_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;
    let storage = index.keys.len() as usize * size_of::<u64>()
        + index.row_buckets.capacity() * size_of::<Option<TemporalBucket>>();

    eprintln!(
        "{label} rows={rows} q={QUANTUM_MS} build_ms={:.3} exact_ns={query:.1} update_ns={update:.1} churn_ns={churn:.1} bytes_upper={storage}",
        build.as_secs_f64() * 1_000.0,
    );
}

fn profile_heap(
    label: &str,
    rows: usize,
    horizon_ms: u64,
    query_iterations: usize,
    mutation_iterations: usize,
) {
    let mut values = profile_values(rows, horizon_ms);
    let build_started = Instant::now();
    let mut index = HeapDeadlineIndex::invalid();
    black_box(index.earliest_exact(values.len(), |row| values[row]));
    let build = build_started.elapsed();

    let query_started = Instant::now();
    for _ in 0..query_iterations {
        black_box(index.earliest_exact(values.len(), |row| values[row]));
    }
    let query = query_started.elapsed().as_nanos() as f64 / query_iterations as f64;

    let update_started = Instant::now();
    for step in 0..mutation_iterations {
        let row = mix(step as u64) as usize % values.len();
        let previous = values[row];
        let deadline = InstantMillis(previous.unwrap().0.saturating_add(if step & 1 == 0 {
            1
        } else {
            horizon_ms / 2
        }));
        values[row] = Some(deadline);
        index.update(row, Some(deadline), |row| values[row]);
    }
    let update = update_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;

    let churn_started = Instant::now();
    for step in 0..mutation_iterations {
        let removed = mix((step + mutation_iterations) as u64) as usize % values.len();
        let last = values.len() - 1;
        index.swap_remove(removed, last, |row| values[row]);
        values.swap_remove(removed);
        let deadline = Some(InstantMillis(
            1_000_000 + mix((step + rows) as u64) % horizon_ms,
        ));
        values.push(deadline);
        index.insert(values.len() - 1, deadline, |row| values[row]);
    }
    let churn = churn_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;

    eprintln!(
        "{label} rows={rows} heap build_ms={:.3} exact_ns={query:.1} update_ns={update:.1} churn_ns={churn:.1} bytes={}",
        build.as_secs_f64() * 1_000.0,
        index.storage_bytes(),
    );
}

fn profile_wheel<const QUANTUM_MS: u64>(
    label: &str,
    rows: usize,
    horizon_ms: u64,
    query_iterations: usize,
    mutation_iterations: usize,
) {
    let mut values = profile_values(rows, horizon_ms);
    let build_started = Instant::now();
    let mut index = BucketedDeadlineWheel::<QUANTUM_MS>::new();
    for (row, deadline) in values.iter().copied().enumerate() {
        index.insert(row, deadline);
    }
    let build = build_started.elapsed();

    let query_started = Instant::now();
    for _ in 0..query_iterations {
        black_box(index.earliest(&values));
    }
    let query = query_started.elapsed().as_nanos() as f64 / query_iterations as f64;

    let update_started = Instant::now();
    for step in 0..mutation_iterations {
        let row = mix(step as u64) as usize % values.len();
        let previous = values[row].unwrap();
        let bucket = previous.0 / QUANTUM_MS;
        let deadline = InstantMillis(
            bucket
                .saturating_add((step & 1) as u64)
                .saturating_mul(QUANTUM_MS)
                .saturating_add(QUANTUM_MS / 2),
        );
        values[row] = Some(deadline);
        index.update(row, Some(deadline));
    }
    let update = update_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;

    let churn_started = Instant::now();
    for step in 0..mutation_iterations {
        let removed = mix((step + mutation_iterations) as u64) as usize % values.len();
        let last = values.len() - 1;
        index.swap_remove(removed, last);
        values.swap_remove(removed);
        let deadline = Some(InstantMillis(
            1_000_000 + mix((step + rows) as u64) % horizon_ms,
        ));
        values.push(deadline);
        index.insert(values.len() - 1, deadline);
    }
    let churn = churn_started.elapsed().as_nanos() as f64 / mutation_iterations as f64;

    eprintln!(
        "{label} rows={rows} wheel_q={QUANTUM_MS} build_ms={:.3} exact_ns={query:.1} update_ns={update:.1} churn_ns={churn:.1} bytes_lower={}",
        build.as_secs_f64() * 1_000.0,
        index.storage_bytes(),
    );
}

fn profile_wheels(
    label: &str,
    rows: usize,
    horizon_ms: u64,
    query_iterations: usize,
    mutation_iterations: usize,
    quanta: [u64; 3],
) {
    match quanta {
        [10, 50, 100] => {
            profile_wheel::<10>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
            profile_wheel::<50>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
            profile_wheel::<100>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
        }
        [100, 250, 1_000] => {
            profile_wheel::<100>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
            profile_wheel::<250>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
            profile_wheel::<1_000>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
        }
        [100, 1_000, 5_000] => {
            profile_wheel::<100>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
            profile_wheel::<1_000>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
            profile_wheel::<5_000>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
        }
        [1_000, 5_000, 30_000] => {
            profile_wheel::<1_000>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
            profile_wheel::<5_000>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
            profile_wheel::<30_000>(
                label,
                rows,
                horizon_ms,
                query_iterations,
                mutation_iterations,
            );
        }
        _ => unreachable!(),
    }
}

fn profile_family(label: &str, horizon_ms: u64, quanta: [u64; 3]) {
    for rows in [10_000, 100_000, 1_000_000] {
        let query_iterations = if rows < 100_000 { 2_000 } else { 200 };
        let mutation_iterations = if rows < 1_000_000 { 20_000 } else { 50_000 };
        match quanta {
            [10, 50, 100] => {
                profile_roaring::<10>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
                profile_roaring::<50>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
                profile_roaring::<100>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
            }
            [100, 250, 1_000] => {
                profile_roaring::<100>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
                profile_roaring::<250>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
                profile_roaring::<1_000>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
            }
            [1_000, 5_000, 30_000] => {
                profile_roaring::<1_000>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
                profile_roaring::<5_000>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
                profile_roaring::<30_000>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
            }
            [100, 1_000, 5_000] => {
                profile_roaring::<100>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
                profile_roaring::<1_000>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
                profile_roaring::<5_000>(
                    label,
                    rows,
                    horizon_ms,
                    query_iterations,
                    mutation_iterations,
                );
            }
            _ => unreachable!(),
        }
        profile_heap(
            label,
            rows,
            horizon_ms,
            query_iterations,
            mutation_iterations,
        );
        profile_wheels(
            label,
            rows,
            horizon_ms,
            query_iterations,
            mutation_iterations,
            quanta,
        );
    }
}

fn profile_dense_cull(rows: usize, due_percent: u64) {
    let now = InstantMillis(2_000_000);
    let values = (0..rows)
        .map(|row| {
            let mixed = mix(row as u64);
            Some(if mixed % 100 < due_percent {
                InstantMillis(now.0 - mixed % 100_000)
            } else {
                InstantMillis(now.0 + 1 + mixed % 100_000)
            })
        })
        .collect::<Vec<_>>();

    let mut heap_values = values.clone();
    let mut index = HeapDeadlineIndex::invalid();
    black_box(index.earliest_exact(heap_values.len(), |row| heap_values[row]));
    let heap_started = Instant::now();
    let mut heap_removed = 0;
    while let Some(row) = index.first_due(heap_values.len(), now, |row| heap_values[row]) {
        let last = heap_values.len() - 1;
        index.swap_remove(row, last, |row| heap_values[row]);
        heap_values.swap_remove(row);
        heap_removed += 1;
    }
    let heap = heap_started.elapsed();

    let mut scan_values = values.clone();
    let scan_started = Instant::now();
    let mut scan_removed = 0;
    let mut row = 0;
    while row < scan_values.len() {
        if scan_values[row].is_some_and(|deadline| deadline <= now) {
            scan_values.swap_remove(row);
            scan_removed += 1;
        } else {
            row += 1;
        }
    }
    let scan = scan_started.elapsed();

    let mut hybrid_values = values;
    let mut hybrid_index = HeapDeadlineIndex::invalid();
    black_box(hybrid_index.earliest_exact(hybrid_values.len(), |row| hybrid_values[row]));
    let hybrid_started = Instant::now();
    hybrid_index.invalidate();
    let mut hybrid_removed = 0;
    let mut row = 0;
    while row < hybrid_values.len() {
        if hybrid_values[row].is_some_and(|deadline| deadline <= now) {
            let last = hybrid_values.len() - 1;
            hybrid_index.swap_remove(row, last, |row| hybrid_values[row]);
            hybrid_values.swap_remove(row);
            hybrid_removed += 1;
        } else {
            row += 1;
        }
    }
    black_box(hybrid_index.earliest_exact(hybrid_values.len(), |row| hybrid_values[row]));
    let hybrid = hybrid_started.elapsed();

    assert_eq!(heap_removed, scan_removed);
    assert_eq!(heap_removed, hybrid_removed);
    eprintln!(
        "dense rows={rows} due_percent={due_percent} removed={heap_removed} heap_ms={:.3} scan_ms={:.3} scan_rebuild_ms={:.3}",
        heap.as_secs_f64() * 1_000.0,
        scan.as_secs_f64() * 1_000.0,
        hybrid.as_secs_f64() * 1_000.0,
    );
}

#[test]
#[ignore]
fn profile_tranche_one_candidates() {
    profile_family("reverse", 480_000, [1_000, 5_000, 30_000]);
    profile_family("transported", 900_000, [1_000, 5_000, 30_000]);
    profile_family("local", 60_000, [100, 250, 1_000]);
}

#[test]
#[ignore]
fn profile_tranche_two_candidates() {
    profile_family("scheduled", 5_500, [10, 50, 100]);
    profile_family("channels", 60_000, [100, 250, 1_000]);
    profile_family("path", 20_000, [100, 1_000, 5_000]);
}

#[test]
#[ignore]
fn profile_dense_cull_candidates() {
    for rows in [10_000, 100_000, 1_000_000] {
        for due_percent in [1, 5, 10, 25, 100] {
            profile_dense_cull(rows, due_percent);
        }
    }
}

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
