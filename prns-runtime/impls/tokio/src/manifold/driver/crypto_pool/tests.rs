use super::*;

#[test]
fn automatic_workers_use_bounded_efficiency_spillover_and_keep_host_headroom() {
    assert_eq!(automatic_worker_count(10, Some(4)), 6);
    assert_eq!(automatic_worker_count(6, Some(4)), 4);
    assert_eq!(automatic_worker_count(16, Some(12)), 12);
    assert_eq!(automatic_worker_count(8, None), 6);
}

#[test]
fn crypto_backpressure_depth_is_bounded_across_worker_counts() {
    assert_eq!(crypto_backpressure_depth(1), MIN_CRYPTO_QUEUE_DEPTH);
    assert_eq!(crypto_backpressure_depth(6), MIN_CRYPTO_QUEUE_DEPTH);
    assert_eq!(crypto_backpressure_depth(32), MAX_CRYPTO_QUEUE_DEPTH);
    assert_eq!(
        crypto_backpressure_depth(usize::MAX),
        MAX_CRYPTO_QUEUE_DEPTH
    );
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn crypto_metrics_are_bounded_snapshots() {
    let pool = CryptoPool::spawn(1, Arc::new(Notify::new())).expect("worker spawns");

    assert_eq!(bounded_u32(usize::MAX), u32::MAX);
    assert!(!pool.has_queue_capacity(usize::MAX));
    pool.workers[0].outstanding_jobs.set(1);
    pool.record_completed(0);

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
    let pool = CryptoPool::spawn(2, Arc::new(Notify::new())).expect("workers spawn");
    let state = pool.state.clone();
    drop(pool);
    assert!(state.shutdown.load(Ordering::Acquire));
    assert_eq!(state.queued_jobs.load(Ordering::Acquire), 0);
    assert_eq!(Arc::strong_count(&state), 1);
}

#[tokio::test]
async fn completion_wake_carries_no_payload_and_result_moves_through_worker_ring() {
    use crate::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};

    let secret = Ed25519SecretKey::new([0x51; 32]);
    let signing_key = IdentitySigningPublicKey::new(ed25519_public_key(&secret));
    let packet_hash = PacketHash::new([0x73; 32]);
    let signature = ed25519_sign(&secret, packet_hash.as_bytes());
    let completion_wake = Arc::new(Notify::new());
    let pool = CryptoPool::spawn(1, completion_wake.clone()).expect("worker spawns");

    pool.submit(CryptoJob::Verify(EngineVerifyJob {
        packet_hash,
        signing_key,
        signature,
        id: CommandId(7),
        settlement: Settlement::AnnounceNow(Ok(())),
        arrived_at: InstantMillis(0),
    }));

    tokio::time::timeout(Duration::from_secs(1), completion_wake.notified())
        .await
        .expect("worker signals the payload-free completion wake");
    let completion = pool
        .pop_completion()
        .expect("result moved into its SPSC ring");
    assert_eq!(completion.worker, 0);
    assert!(matches!(
        completion.result,
        CryptoResult::Verified {
            id: CommandId(7),
            valid: true,
            ..
        }
    ));
    pool.record_completed(completion.worker);
    pool.packet_verdict_settled();
    assert!(!pool.has_completion());
}

#[test]
fn command_sized_burst_backpressures_without_dropping_jobs_or_results() {
    use crate::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};

    const JOBS: usize = 64;
    let secret = Ed25519SecretKey::new([0x39; 32]);
    let signing_key = IdentitySigningPublicKey::new(ed25519_public_key(&secret));
    let packet_hash = PacketHash::new([0xa7; 32]);
    let signature = ed25519_sign(&secret, packet_hash.as_bytes());
    let pool = CryptoPool::spawn(1, Arc::new(Notify::new())).expect("worker spawns");

    for id in 0..JOBS {
        pool.submit(CryptoJob::Verify(EngineVerifyJob {
            packet_hash,
            signing_key,
            signature,
            id: CommandId(id as u64),
            settlement: Settlement::AnnounceNow(Ok(())),
            arrived_at: InstantMillis(0),
        }));
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut completed = 0usize;
    while completed < JOBS {
        if let Some(completion) = pool.pop_completion() {
            assert!(matches!(
                completion.result,
                CryptoResult::Verified { valid: true, .. }
            ));
            pool.record_completed(completion.worker);
            pool.packet_verdict_settled();
            completed += 1;
        } else {
            assert!(std::time::Instant::now() < deadline, "all jobs complete");
            std::thread::yield_now();
        }
    }

    assert_eq!(pool.state.queued_jobs.load(Ordering::Acquire), 0);
    assert_eq!(pool.state.ready_results.load(Ordering::Acquire), 0);
    assert!(!pool.has_completion());
}

#[test]
fn worker_verifier_cache_retains_recent_decompressed_keys() {
    use crate::crypto::{ed25519_public_key, Ed25519SecretKey};

    let keys: Vec<_> = (0..WORKER_VERIFIER_CACHE_DEPTH)
        .map(|index| {
            let byte = u8::try_from(index).unwrap_or_default().saturating_add(0x31);
            ed25519_public_key(&Ed25519SecretKey::new([byte; 32]))
        })
        .collect();
    let mut cache = core::array::from_fn(|_| None);

    for key in &keys {
        assert_eq!(
            cached_verifier(&mut cache, key)
                .expect("fixture key decompresses")
                .public_key(),
            key
        );
    }
    assert!(keys.iter().all(|key| cache
        .iter()
        .flatten()
        .any(|verifier| verifier.public_key() == key)));

    let recent = cached_verifier(&mut cache, &keys[0]).expect("oldest retained key is reused");
    assert_eq!(recent.public_key(), &keys[0]);
}
