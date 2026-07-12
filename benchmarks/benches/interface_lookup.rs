//! The interface-lookup crossover: the bare linear view vs. the heap reactor's
//! indexed roster, `descriptor_for` for a present id (hit) and an absent id (miss),
//! across fleet sizes. A busy TCP server holds an interface per client and ingress
//! runs this lookup per packet, so the flat regime is the one that matters at scale.

use std::hint::black_box;
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion};

use personal_rns::interfaces::pipe::core as pipe_core;
use personal_rns::interfaces::{
    AttachedInterfaces, IndexedAttachedInterfaces, InterfaceDescriptor, InterfaceId,
};

fn iface_n(n: u32) -> InterfaceId {
    let mut id = [0x07u8, 0, 0, 0, 0, 0, 0, 0];
    id[4..].copy_from_slice(&n.to_be_bytes());
    InterfaceId::new(id)
}

fn bench_size(group: &mut BenchmarkGroup<'_, WallTime>, n: u32, lookup: InterfaceId) {
    let descriptors: Vec<InterfaceDescriptor> =
        (0..n).map(|i| pipe_core::descriptor(iface_n(i))).collect();
    let indexed = IndexedAttachedInterfaces::from(descriptors.clone());

    group.bench_with_input(BenchmarkId::new("linear", n), &n, |b, _| {
        let view = AttachedInterfaces::new(&descriptors);
        b.iter(|| black_box(view.descriptor_for(black_box(lookup))))
    });
    group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
        let view = indexed.view();
        b.iter(|| black_box(view.descriptor_for(black_box(lookup))))
    });
}

fn interface_lookup(c: &mut Criterion) {
    let mut hit = c.benchmark_group("interface_lookup_hit");
    for n in [4u32, 16, 64, 256, 1024] {
        bench_size(&mut hit, n, iface_n(n / 2));
    }
    hit.finish();

    let mut miss = c.benchmark_group("interface_lookup_miss");
    for n in [4u32, 16, 64, 256, 1024] {
        bench_size(&mut miss, n, iface_n(1_000_000 + n));
    }
    miss.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_millis(1500))
        .warm_up_time(Duration::from_millis(400));
    targets = interface_lookup
}
criterion_main!(benches);
