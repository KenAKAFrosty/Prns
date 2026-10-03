#![allow(clippy::unwrap_used)]

use super::*;
use std::{
    convert::Infallible,
    sync::atomic::{AtomicUsize, Ordering},
};

fn source(output: &mut [u8]) -> Result<(), Infallible> {
    output.fill(0x57);
    Ok(())
}

fn owner() -> TokioHandleEntropy {
    TokioHandleEntropy::from_sources(RuntimeEntropy::try_new(source).unwrap(), source)
}

#[test]
fn cloned_owners_continue_one_stream_without_copying_state() {
    let owner = owner();
    let clone = owner.clone();
    let mut reference = RuntimeEntropy::try_new(source).unwrap();
    let mut actual = [0; 128];
    let mut expected = [0; 128];
    owner.fill(&mut actual[..64]);
    clone.fill(&mut actual[64..]);
    reference.fill_random(&mut expected[..64]);
    reference.fill_random(&mut expected[64..]);
    assert_eq!(actual, expected);
    assert!(Arc::ptr_eq(&owner.owner, &clone.owner));
}

#[test]
fn unrelated_owners_do_not_consume_each_others_stream() {
    let first = owner();
    let second = owner();
    assert!(!Arc::ptr_eq(&first.owner, &second.owner));
    first.fill(&mut [0; 64]);
    let mut actual = [0; 64];
    let mut expected = [0; 64];
    second.fill(&mut actual);
    RuntimeEntropy::try_new(source)
        .unwrap()
        .fill_random(&mut expected);
    assert_eq!(actual, expected);
}

#[test]
fn clones_cross_threads_without_rewinding_or_duplicate_blocks() {
    let owner = owner();
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let owner = owner.clone();
            std::thread::spawn(move || {
                let mut bytes = [0; 64];
                owner.fill(&mut bytes);
                bytes
            })
        })
        .collect();
    let mut actual: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    let mut reference = RuntimeEntropy::try_new(source).unwrap();
    let mut expected = vec![[0; 64]; 8];
    for bytes in &mut expected {
        reference.fill_random(bytes);
    }
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn handoff_preserves_position_and_path_reads_do_not_advance_the_stream() {
    let mut stream = RuntimeEntropy::try_new(source).unwrap();
    let mut reference = RuntimeEntropy::try_new(source).unwrap();
    stream.fill_random(&mut [0; 73]);
    reference.fill_random(&mut [0; 73]);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let owner = TokioHandleEntropy::from_sources(stream, move |output: &mut [u8]| {
        observed.fetch_add(1, Ordering::Relaxed);
        output.fill(0xA3);
        Ok::<(), Infallible>(())
    });
    let mut path = [0; 16];
    owner.clone().try_fill_path_id(&mut path).unwrap();
    let mut actual = [0; 128];
    let mut expected = [0; 128];
    owner.fill(&mut actual);
    reference.fill_random(&mut expected);
    assert_eq!(
        (path, calls.load(Ordering::Relaxed), actual),
        ([0xA3; 16], 1, expected)
    );
}

#[test]
fn failed_reseed_and_later_recovery_preserve_the_core_stream() {
    fn scripted(calls: Arc<AtomicUsize>) -> impl EntropySource<Error = ()> {
        move |output: &mut [u8]| {
            let call = calls.fetch_add(1, Ordering::Relaxed) + 1;
            if call == 2 {
                return Err(());
            }
            output.fill(0x57);
            Ok(())
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let owner = TokioHandleEntropy::from_sources(
        RuntimeEntropy::try_new(scripted(Arc::clone(&calls))).unwrap(),
        source,
    );
    let mut reference = RuntimeEntropy::try_new(scripted(Arc::new(AtomicUsize::new(0)))).unwrap();
    owner.fill(&mut []);
    let mut actual = vec![0; 2 * 64 * 1024 + 64];
    let mut expected = vec![0; actual.len()];
    owner.fill(&mut actual);
    reference.fill_random(&mut expected);
    assert_eq!((actual, calls.load(Ordering::Relaxed)), (expected, 3));
}

#[test]
fn path_source_failure_does_not_poison_or_advance_the_runtime_stream() {
    let mut calls = 0;
    let owner = TokioHandleEntropy::from_sources(
        RuntimeEntropy::try_new(source).unwrap(),
        move |output: &mut [u8]| {
            calls += 1;
            output.fill(0xA3);
            if calls == 1 {
                Err(())
            } else {
                Ok(())
            }
        },
    );
    let mut path = [0; 16];
    assert_eq!(
        owner.try_fill_path_id(&mut path),
        Err(PathEntropyError::SourceUnavailable)
    );
    assert_eq!(owner.clone().try_fill_path_id(&mut path), Ok(()));
    let mut actual = [0; 64];
    let mut expected = [0; 64];
    owner.fill(&mut actual);
    RuntimeEntropy::try_new(source)
        .unwrap()
        .fill_random(&mut expected);
    assert_eq!((path, actual), ([0xA3; 16], expected));
}

#[test]
#[allow(clippy::panic)]
fn poisoned_ownership_refuses_both_consumers_without_output() {
    let owner = TokioHandleEntropy::from_sources(
        RuntimeEntropy::try_new(source).unwrap(),
        |_: &mut [u8]| -> Result<(), ()> { panic!("injected source panic") },
    );
    let clone = owner.clone();
    assert!(
        std::thread::spawn(move || clone.try_fill_path_id(&mut [0; 16]))
            .join()
            .is_err()
    );
    let mut output = [0xA5; 64];
    assert_eq!(
        owner.try_fill_path_id(&mut output),
        Err(PathEntropyError::OwnerPoisoned)
    );
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.fill(&mut output))).is_err()
    );
    assert_eq!(output, [0xA5; 64]);
}

#[test]
#[ignore = "local ownership timing probe, not a publication or fleet benchmark"]
fn compare_concrete_and_erased_ownership_cost() {
    use std::{hint::black_box, time::Instant};
    const FILLS: usize = 200_000;
    let previous = Arc::new(Mutex::new(
        RuntimeEntropy::try_new(OsEntropySource).unwrap(),
    ));
    let current = TokioHandleEntropy::new();
    for length in [16, 64, 256] {
        let mut bytes = vec![0; length];
        for round in 0..5 {
            let start = Instant::now();
            for _ in 0..FILLS {
                black_box(&previous)
                    .lock()
                    .unwrap()
                    .fill_random(black_box(&mut bytes));
                black_box(&bytes);
            }
            let old = start.elapsed();
            let start = Instant::now();
            for _ in 0..FILLS {
                black_box(&current).fill(black_box(&mut bytes));
                black_box(&bytes);
            }
            eprintln!(
                "length={length} round={round} fills={FILLS} previous={old:?} erased={:?}",
                start.elapsed()
            );
        }
    }
    let mut path = [0; crate::engine::PATH_REQUEST_ID_LEN];
    for round in 0..5 {
        let start = Instant::now();
        for _ in 0..FILLS {
            OsEntropySource
                .try_fill_entropy(black_box(&mut path))
                .unwrap();
            black_box(&path);
        }
        let old = start.elapsed();
        let start = Instant::now();
        for _ in 0..FILLS {
            black_box(&current)
                .try_fill_path_id(black_box(&mut path))
                .unwrap();
            black_box(&path);
        }
        eprintln!(
            "path round={round} fills={FILLS} previous={old:?} owned={:?}",
            start.elapsed()
        );
    }
    eprintln!("previous_handle={} current_handle={} previous_allocation_payload={} current_allocation_payload={} arc_counters={}", std::mem::size_of_val(&previous), std::mem::size_of_val(&current), std::mem::size_of::<Mutex<RuntimeEntropy<OsEntropySource>>>(), std::mem::size_of::<Mutex<EntropyState<OsEntropySource,OsEntropySource>>>(), 2 * std::mem::size_of::<usize>());
}
