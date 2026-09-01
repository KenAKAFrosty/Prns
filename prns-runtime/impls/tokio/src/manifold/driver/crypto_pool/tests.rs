use super::*;
use std::time::Duration;

use crate::engine::{
    CommandId, DeliveryEvidence, DeliveryProof, PacketReceiptDelivered, ReceiptProofClaim,
};
use crate::identity::IdentitySigningPublicKey;
use crate::interfaces::InterfaceId;
use crate::routing::dedup::PacketHash;
use crate::units::{InstantMillis, RttMillis};

fn receipt_proof_verify_owed(
    id: CommandId,
    packet_hash: PacketHash,
    signing_key: IdentitySigningPublicKey,
    signature: Ed25519Signature,
) -> ReceiptProofVerifyOwed {
    ReceiptProofVerifyOwed {
        claim: ReceiptProofClaim::SendSinglePacket {
            id,
            delivered: PacketReceiptDelivered {
                rtt: RttMillis::new(0),
                evidence: DeliveryEvidence::Proof(DeliveryProof::Implicit(packet_hash)),
            },
        },
        packet_hash,
        signing_key,
        signature,
        arrived_at: InstantMillis(0),
    }
}

#[test]
fn packet_verdict_hotness_is_outstanding_or_a_bounded_activity_budget() {
    let pool = CryptoPool::spawn(1, Arc::new(Notify::new())).expect("worker spawns");

    pool.packet_verdicts_owed.set(1);
    assert!(pool.take_packet_verdict_hot_turn());
    pool.packet_verdict_settled();

    for _ in 0..CryptoPool::PACKET_VERDICT_HOT_TURNS {
        assert!(pool.take_packet_verdict_hot_turn());
    }
    assert!(!pool.take_packet_verdict_hot_turn());
    assert!(!pool.take_packet_verdict_hot_turn());
}

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

#[test]
fn verification_batch_target_uses_effective_parallelism_without_exceeding_worker_capacity() {
    assert_eq!(verify_batch_target(1, Some(4)), CRYPTO_WORKER_BATCH_DEPTH);
    assert_eq!(verify_batch_target(2, Some(4)), CRYPTO_WORKER_BATCH_DEPTH);
    assert_eq!(verify_batch_target(4, Some(4)), 4);
    assert_eq!(verify_batch_target(6, Some(4)), 4);
    assert_eq!(verify_batch_target(8, None), 2);
}

#[test]
fn batch_affinity_is_bounded_by_target_and_estimated_load() {
    let mut pool = CryptoPool::spawn(6, Arc::new(Notify::new())).expect("workers spawn");
    pool.verify_batch_target = 4;
    assert_eq!(pool.verify_batch_target, 4);

    let first = &pool.workers[0];
    first.outstanding_jobs.set(1);
    first.outstanding_work.set(1);
    first.tail_class.set(Some(CryptoJobClass::Verify));
    first.tail_run.set(1);
    assert_eq!(
        pool.worker_for(CryptoJobClass::Verify, 1),
        0,
        "a short compatible tail is worth bounded skew"
    );

    first.outstanding_jobs.set(4);
    first.outstanding_work.set(4);
    first.tail_run.set(4);
    assert_eq!(
        pool.worker_for(CryptoJobClass::Verify, 1),
        1,
        "a full target batch spills to the idle worker"
    );

    first.outstanding_jobs.set(1);
    first.outstanding_work.set(33);
    first.tail_class.set(Some(CryptoJobClass::Bulk));
    first.tail_run.set(1);
    assert_eq!(
        pool.worker_for(CryptoJobClass::Verify, 1),
        1,
        "a resource-sized job cannot masquerade as one cheap queue entry"
    );
}

#[cfg(feature = "runtime-metrics")]
#[test]
fn crypto_metrics_are_bounded_snapshots() {
    let pool = CryptoPool::spawn(1, Arc::new(Notify::new())).expect("worker spawns");

    assert_eq!(bounded_u32(usize::MAX), u32::MAX);
    assert!(!pool.has_queue_capacity(usize::MAX));
    pool.workers[0].outstanding_jobs.set(1);
    pool.workers[0].outstanding_work.set(1);
    pool.record_completed(0, 1);

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
    assert!(!pool.prepare_completion_wait());

    pool.submit(CryptoJob::VerifyReceiptProof(receipt_proof_verify_owed(
        CommandId(7),
        packet_hash,
        signing_key,
        signature,
    )));

    tokio::time::timeout(Duration::from_secs(1), completion_wake.notified())
        .await
        .expect("worker signals the payload-free completion wake");
    let completion = pool
        .pop_completion()
        .expect("result moved into its SPSC ring");
    assert_eq!(completion.worker, 0);
    assert!(matches!(
        completion.result,
        CryptoResult::ReceiptProofVerified {
            owed,
            verification: ReceiptProofVerification::Valid,
        } if owed.claim.command_id() == CommandId(7)
    ));
    pool.record_completed(completion.worker, completion.work);
    pool.packet_verdict_settled();
    assert!(!pool.has_completion());
}

#[tokio::test]
async fn link_receipt_signing_moves_metadata_and_signature_through_the_worker_ring() {
    use crate::crypto::{ed25519_public_key, ed25519_verify, Ed25519SecretKey};
    use crate::engine::LinkReceiptSignOwed;

    let secret = Ed25519SecretKey::new([0x61; 32]);
    let public = ed25519_public_key(&secret);
    let target = InterfaceId::new([0x72; 8]);
    let link_id = LinkId::new([0x83; 16]);
    let packet_hash = PacketHash::new([0x94; 32]);
    let completion_wake = Arc::new(Notify::new());
    let pool = CryptoPool::spawn(1, completion_wake.clone()).expect("worker spawns");
    assert!(!pool.prepare_completion_wait());

    let mut receipts = vec![LinkReceiptSignOwed {
        target,
        link_id,
        packet_hash,
        signing_secret: secret,
    }];
    pool.submit_link_receipts(&mut receipts);
    assert!(receipts.is_empty());

    tokio::time::timeout(Duration::from_secs(1), completion_wake.notified())
        .await
        .expect("worker signals LINK receipt completion");
    let completion = pool
        .pop_completion()
        .expect("LINK receipt result moves back");
    let CryptoResult::LinkReceiptSigned(completed) = completion.result else {
        panic!("the LINK receipt job must return its typed result");
    };
    assert_eq!(completed.target, target);
    assert_eq!(completed.link_id, link_id);
    assert_eq!(completed.packet_hash, packet_hash);
    ed25519_verify(&public, packet_hash.as_bytes(), &completed.signature)
        .expect("worker returns the exact valid receipt signature");
    pool.record_completed(completion.worker, completion.work);
    pool.packet_verdict_settled();
}

#[tokio::test]
async fn already_ready_link_receipts_move_as_one_worker_batch() {
    use crate::crypto::{ed25519_public_key, ed25519_verify, Ed25519SecretKey};
    use crate::engine::LinkReceiptSignOwed;

    let secret_bytes = [0xa5; 32];
    let public = ed25519_public_key(&Ed25519SecretKey::new(secret_bytes));
    let target = InterfaceId::new([0xb6; 8]);
    let link_id = LinkId::new([0xc7; 16]);
    let mut receipts = (0u8..4)
        .map(|index| LinkReceiptSignOwed {
            target,
            link_id,
            packet_hash: PacketHash::new([index; 32]),
            signing_secret: Ed25519SecretKey::new(secret_bytes),
        })
        .collect::<Vec<_>>();
    let completion_wake = Arc::new(Notify::new());
    let pool = CryptoPool::spawn(1, completion_wake.clone()).expect("worker spawns");
    assert!(!pool.prepare_completion_wait());

    pool.submit_link_receipts(&mut receipts);
    assert!(receipts.is_empty());
    assert_eq!(pool.packet_verdicts_owed.get(), 4);

    tokio::time::timeout(Duration::from_secs(1), completion_wake.notified())
        .await
        .expect("the already-ready receipt batch completes without a fill wait");
    for index in 0u8..4 {
        let completion = pool
            .pop_completion()
            .expect("the bulk-published result ring contains every receipt");
        let CryptoResult::LinkReceiptSigned(completed) = completion.result else {
            panic!("the LINK receipt batch must return typed results");
        };
        assert_eq!(completed.target, target);
        assert_eq!(completed.link_id, link_id);
        assert_eq!(completed.packet_hash, PacketHash::new([index; 32]));
        ed25519_verify(
            &public,
            completed.packet_hash.as_bytes(),
            &completed.signature,
        )
        .expect("every batched receipt keeps its exact signature semantics");
        pool.record_completed(completion.worker, completion.work);
        pool.packet_verdict_settled();
    }
    assert!(!pool.has_completion());
}

#[test]
fn same_link_receipt_backlog_routes_in_load_balanced_pairs() {
    use crate::crypto::Ed25519SecretKey;
    use crate::engine::LinkReceiptSignOwed;

    let target = InterfaceId::new([0xd8; 8]);
    let link_id = LinkId::new([0xe9; 16]);
    let mut receipts = (0u8..5)
        .map(|index| LinkReceiptSignOwed {
            target,
            link_id,
            packet_hash: PacketHash::new([index; 32]),
            signing_secret: Ed25519SecretKey::new([0xfa; 32]),
        })
        .collect::<Vec<_>>();
    let pool = CryptoPool::spawn(4, Arc::new(Notify::new())).expect("workers spawn");

    pool.submit_link_receipts(&mut receipts);

    assert!(receipts.is_empty());
    assert_eq!(
        pool.workers
            .iter()
            .map(|worker| worker.outstanding_jobs.get())
            .collect::<Vec<_>>(),
        vec![2, 2, 1, 0],
        "same-key work forms pairs without pinning the whole burst to one worker"
    );
}

#[test]
fn completion_wait_arm_closes_ready_before_and_after_arm_races() {
    let pool = CryptoPool::spawn(1, Arc::new(Notify::new())).expect("worker spawns");

    assert!(!pool.prepare_completion_wait());
    assert!(
        pool.state.completion_wake_armed.load(Ordering::Acquire),
        "an empty pool arms its one Tokio hole-punch"
    );

    pool.state.ready_results.store(1, Ordering::Release);
    assert!(pool.prepare_completion_wait());
    assert!(
        !pool.state.completion_wake_armed.load(Ordering::Acquire),
        "durable readiness disarms a redundant notification"
    );
    pool.state.ready_results.store(0, Ordering::Release);
}

#[test]
fn parked_worker_arm_is_cleared_by_submission_without_losing_the_job() {
    use crate::crypto::Ed25519SecretKey;
    use crate::engine::ProofSignOwed;

    let pool = CryptoPool::spawn(1, Arc::new(Notify::new())).expect("worker spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while !pool.workers[0].wake_armed.load(Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline, "worker arms its park");
        std::thread::yield_now();
    }

    pool.submit(CryptoJob::SignProof(ProofSignOwed {
        target: InterfaceId::new([0x21; 8]),
        packet_hash: PacketHash::new([0x32; 32]),
        signing_secret: Ed25519SecretKey::new([0x43; 32]),
    }));
    let completion = loop {
        if let Some(completion) = pool.pop_completion() {
            break completion;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "submitted job completes"
        );
        std::thread::yield_now();
    };
    pool.record_completed(completion.worker, completion.work);
    pool.packet_verdict_settled();
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
        pool.submit(CryptoJob::VerifyReceiptProof(receipt_proof_verify_owed(
            CommandId(id as u64),
            packet_hash,
            signing_key,
            signature,
        )));
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut completed = 0usize;
    while completed < JOBS {
        if let Some(completion) = pool.pop_completion() {
            assert!(matches!(
                completion.result,
                CryptoResult::ReceiptProofVerified {
                    verification: ReceiptProofVerification::Valid,
                    ..
                }
            ));
            pool.record_completed(completion.worker, completion.work);
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
fn batch_rejection_falls_back_to_exact_per_job_verdicts() {
    use crate::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};

    const JOBS: usize = CRYPTO_WORKER_BATCH_DEPTH;
    const INVALID_JOB: usize = 3;
    let secret = Ed25519SecretKey::new([0x63; 32]);
    let signing_key = IdentitySigningPublicKey::new(ed25519_public_key(&secret));
    let packet_hash = PacketHash::new([0x8b; 32]);
    let valid_signature = ed25519_sign(&secret, packet_hash.as_bytes());
    let pool = CryptoPool::spawn(1, Arc::new(Notify::new())).expect("worker spawns");

    for id in 0..JOBS {
        let mut signature = valid_signature;
        if id == INVALID_JOB {
            signature.0[0] ^= 1;
        }
        pool.submit(CryptoJob::VerifyReceiptProof(receipt_proof_verify_owed(
            CommandId(id as u64),
            packet_hash,
            signing_key,
            signature,
        )));
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut completed = 0usize;
    while completed < JOBS {
        if let Some(completion) = pool.pop_completion() {
            let CryptoResult::ReceiptProofVerified { owed, verification } = completion.result
            else {
                panic!("the test submits only receipt-proof verification jobs");
            };
            assert_eq!(
                verification,
                if owed.claim.command_id() == CommandId(INVALID_JOB as u64) {
                    ReceiptProofVerification::Invalid
                } else {
                    ReceiptProofVerification::Valid
                }
            );
            pool.record_completed(completion.worker, completion.work);
            pool.packet_verdict_settled();
            completed += 1;
        } else {
            assert!(std::time::Instant::now() < deadline, "all jobs complete");
            std::thread::yield_now();
        }
    }
}

#[test]
fn weak_keys_never_enter_batch_verification() {
    use crate::crypto::{Ed25519PublicKey, Ed25519Signature};

    let mut compressed_identity = [0u8; Ed25519PublicKey::LEN];
    compressed_identity[0] = 1;
    let signing_key = IdentitySigningPublicKey::new(Ed25519PublicKey(compressed_identity));
    let mut jobs: HeaplessVec<ScheduledVerifyJob, CRYPTO_WORKER_BATCH_DEPTH> = HeaplessVec::new();
    for id in 0..2 {
        assert!(jobs
            .push(ScheduledVerifyJob {
                owed: receipt_proof_verify_owed(
                    CommandId(id),
                    PacketHash::new([0x91; 32]),
                    signing_key,
                    Ed25519Signature([0u8; Ed25519Signature::LEN]),
                ),
                work: 1,
            })
            .is_ok());
    }
    let mut cache = core::array::from_fn(|_| None);

    assert_eq!(verify_job_batch(&jobs, &mut cache), None);
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
