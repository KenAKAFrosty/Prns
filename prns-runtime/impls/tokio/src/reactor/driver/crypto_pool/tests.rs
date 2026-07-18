use super::*;

#[test]
fn crypto_backpressure_depth_is_bounded_across_worker_counts() {
    assert_eq!(crypto_backpressure_depth(1), MIN_CRYPTO_QUEUE_DEPTH);
    assert_eq!(crypto_backpressure_depth(16), MAX_CRYPTO_QUEUE_DEPTH);
    assert_eq!(
        crypto_backpressure_depth(usize::MAX),
        MAX_CRYPTO_QUEUE_DEPTH
    );
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn crypto_metrics_are_bounded_snapshots() {
    let (results, _result_rx) = tokio::sync::mpsc::unbounded_channel();
    let pool = CryptoPool::spawn(1, results).expect("worker spawns");

    assert_eq!(bounded_u32(usize::MAX), u32::MAX);
    assert!(!pool.has_queue_capacity(usize::MAX));
    pool.record_completed();

    assert_eq!(
        pool.metrics_snapshot(),
        CryptoMetricsSnapshot {
            completed_jobs: 1,
            backpressure_deferrals: 1,
            ..CryptoMetricsSnapshot::default()
        }
    );
}

#[test]
fn dropping_a_crypto_pool_joins_every_worker() {
    let (results, _result_rx) = tokio::sync::mpsc::unbounded_channel();
    let pool = CryptoPool::spawn(2, results).expect("workers spawn");
    let queue = pool.queue.clone();
    drop(pool);
    assert!(queue.shutdown.load(Ordering::Acquire));
    assert_eq!(queue.len.load(Ordering::Acquire), 0);
    assert_eq!(Arc::strong_count(&queue), 1);
}
