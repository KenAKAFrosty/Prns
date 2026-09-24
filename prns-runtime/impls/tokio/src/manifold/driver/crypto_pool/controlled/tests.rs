use super::*;
use crate::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};
use crate::engine::{
    CommandId, DeliveryEvidence, DeliveryProof, PacketReceiptDelivered, ReceiptProofClaim,
};
use crate::identity::IdentitySigningPublicKey;
use crate::routing::dedup::PacketHash;
use crate::units::{InstantMillis, RttMillis};

const TRACE_CAPACITY: usize = 4096;

fn new_control(workers: usize) -> ControlledCrypto {
    ControlledCrypto::new(
        NonZeroUsize::new(workers).unwrap(),
        NonZeroUsize::new(TRACE_CAPACITY).unwrap(),
    )
}
fn probe(id: u8, class: CryptoJobClass) -> CryptoJob {
    CryptoJob::ScheduledTest(ScheduledTestJob {
        id,
        class,
        started: None,
        release: None,
    })
}
fn consume(pool: &CryptoPool) -> CryptoResult {
    let completed = pool.pop_completion().expect("published result");
    pool.record_completed(
        completed.worker.expect("worker ownership"),
        completed.class,
        completed.work,
        &completed.timing,
    );
    completed.result
}

#[test]
fn execution_and_publication_are_separate_and_use_real_signature_verification() {
    let control = new_control(1);
    let wake = Arc::new(Notify::new());
    let pool = control.attach(wake.clone()).unwrap();
    let secret = Ed25519SecretKey::new([0x51; 32]);
    let packet_hash = PacketHash::new([0x73; 32]);
    let owed = ReceiptProofVerifyOwed {
        claim: ReceiptProofClaim::SendSinglePacket {
            id: CommandId(7),
            delivered: PacketReceiptDelivered {
                rtt: RttMillis::new(0),
                evidence: DeliveryEvidence::Proof(DeliveryProof::Implicit(packet_hash)),
            },
        },
        packet_hash,
        signing_key: IdentitySigningPublicKey::new(ed25519_public_key(&secret)),
        signature: ed25519_sign(&secret, packet_hash.as_bytes()),
        arrived_at: InstantMillis(0),
    };
    control
        .hold(
            ControlledWorkKind::VerifySignature,
            CryptoWorkBoundary::Publication,
        )
        .unwrap();
    pool.submit(CryptoJob::VerifySignature(
        SignatureVerifyJob::ReceiptProof(owed),
    ));
    assert!(!pool.prepare_completion_wait());
    assert_eq!(
        control.execute(ControlledWorkerId(0)),
        Ok(ControlledCryptoStep::Executed(ControlledJobId(0)))
    );
    assert_eq!(
        control.publish(ControlledWorkerId(0)),
        Ok(ControlledCryptoStep::Held)
    );
    assert!(!pool.has_completion());
    assert_eq!(
        control.snapshot().unwrap(),
        ControlledCryptoSnapshot {
            queued: 0,
            computed: 1,
            published: 0
        }
    );
    control
        .release(
            ControlledWorkKind::VerifySignature,
            CryptoWorkBoundary::Publication,
        )
        .unwrap();
    assert_eq!(
        control.publish(ControlledWorkerId(0)),
        Ok(ControlledCryptoStep::Published(ControlledJobId(0)))
    );
    assert!(pool.has_completion());
    assert!(
        matches!(consume(&pool), CryptoResult::ReceiptProofVerified { owed, verification: ReceiptProofVerification::Valid } if owed.claim.command_id() == CommandId(7))
    );
    pool.packet_verdict_settled();
    assert_eq!(
        control.snapshot().unwrap(),
        ControlledCryptoSnapshot {
            queued: 0,
            computed: 0,
            published: 0
        }
    );
    assert_eq!(pool.workers[0].outstanding_jobs.get(), 0);
    assert_eq!(pool.workers[0].outstanding_work.get(), 0);
    assert_eq!(
        control.trace().unwrap(),
        vec![
            ControlledCryptoEvent::Queued {
                job: ControlledJobId(0),
                worker: ControlledWorkerId(0),
                kind: ControlledWorkKind::VerifySignature
            },
            ControlledCryptoEvent::Executed {
                job: ControlledJobId(0)
            },
            ControlledCryptoEvent::Published {
                job: ControlledJobId(0)
            },
            ControlledCryptoEvent::Consumed {
                job: ControlledJobId(0)
            },
        ]
    );
}

#[test]
fn interactive_selection_overtakes_bulk_without_reordering_a_worker_result_ring() {
    let control = new_control(1);
    let pool = control.attach(Arc::new(Notify::new())).unwrap();
    pool.submit(probe(1, CryptoJobClass::Bulk));
    pool.submit(probe(2, CryptoJobClass::Latency));
    assert_eq!(
        control.execute(ControlledWorkerId(0)),
        Ok(ControlledCryptoStep::Executed(ControlledJobId(1)))
    );
    assert_eq!(
        control.execute(ControlledWorkerId(0)),
        Ok(ControlledCryptoStep::Executed(ControlledJobId(0)))
    );
    for expected in [2, 1] {
        assert!(matches!(
            control.publish(ControlledWorkerId(0)).unwrap(),
            ControlledCryptoStep::Published(_)
        ));
        assert!(matches!(consume(&pool), CryptoResult::ScheduledTest(id) if id == expected));
    }
    assert_eq!(pool.state.completion_readiness.ready_count(), 0);
}

#[test]
fn four_workers_publish_out_of_order_but_preserve_their_actual_ownership() {
    let control = new_control(4);
    let pool = control.attach(Arc::new(Notify::new())).unwrap();
    for id in 0..4 {
        pool.submit(probe(id, CryptoJobClass::Latency));
    }
    for worker in (0..4).rev() {
        assert_eq!(
            control.execute(ControlledWorkerId(worker)),
            Ok(ControlledCryptoStep::Executed(ControlledJobId(
                worker as u64
            )))
        );
        assert_eq!(
            control.publish(ControlledWorkerId(worker)),
            Ok(ControlledCryptoStep::Published(ControlledJobId(
                worker as u64
            )))
        );
    }
    for id in 0..4 {
        assert!(matches!(consume(&pool), CryptoResult::ScheduledTest(actual) if actual == id));
    }
    assert!(pool
        .workers
        .iter()
        .all(|slot| slot.outstanding_jobs.get() == 0 && slot.outstanding_work.get() == 0));
}

enum RetireAt {
    Queued,
    Executed,
    Published,
}

#[test]
fn retiring_each_boundary_drops_owned_work_and_cannot_attach_to_a_new_generation() {
    for boundary in [RetireAt::Queued, RetireAt::Executed, RetireAt::Published] {
        let control = new_control(1);
        let pool = control.attach(Arc::new(Notify::new())).unwrap();
        let state = pool.state.clone();
        pool.submit(probe(1, CryptoJobClass::Latency));
        match boundary {
            RetireAt::Queued => {}
            RetireAt::Executed => {
                control.execute(ControlledWorkerId(0)).unwrap();
            }
            RetireAt::Published => {
                control.execute(ControlledWorkerId(0)).unwrap();
                control.publish(ControlledWorkerId(0)).unwrap();
            }
        }
        let occupancy = control.snapshot().unwrap();
        drop(pool);
        assert_eq!(
            control.trace().unwrap().last(),
            Some(&ControlledCryptoEvent::Retired { occupancy })
        );
        assert_eq!(control.step(), Err(ControlledCryptoError::Retired));
        assert!(matches!(
            control.attach(Arc::new(Notify::new())),
            Err(ControlledCryptoError::AlreadyAttached)
        ));
        assert_eq!(Arc::strong_count(&state), 1);
        assert!(state.shutdown.load(Ordering::Acquire));
        let fresh = new_control(1);
        assert_eq!(fresh.step(), Err(ControlledCryptoError::NotAttached));
    }
}

#[test]
fn admission_and_result_pressure_use_production_ring_bounds() {
    let control = new_control(1);
    let pool = control.attach(Arc::new(Notify::new())).unwrap();
    control
        .hold(ControlledWorkKind::Probe, CryptoWorkBoundary::Execution)
        .unwrap();
    for id in 0..MIN_CRYPTO_QUEUE_DEPTH {
        assert!(pool.has_queue_capacity(1));
        pool.submit(probe(id as u8, CryptoJobClass::Latency));
    }
    assert!(!pool.has_queue_capacity(1));
    assert_eq!(control.step(), Ok(ControlledCryptoStep::Held));
    control
        .release(ControlledWorkKind::Probe, CryptoWorkBoundary::Execution)
        .unwrap();
    for _ in 0..MIN_CRYPTO_QUEUE_DEPTH {
        control.execute(ControlledWorkerId(0)).unwrap();
    }
    assert_eq!(control.snapshot().unwrap().computed, MIN_CRYPTO_QUEUE_DEPTH);
    for _ in 0..MIN_CRYPTO_QUEUE_DEPTH {
        control.publish(ControlledWorkerId(0)).unwrap();
    }
    for _ in 0..MIN_CRYPTO_QUEUE_DEPTH {
        let _ = consume(&pool);
    }
    assert!(pool.has_queue_capacity(1));
    assert_eq!(
        control.execute(ControlledWorkerId(1)),
        Err(ControlledCryptoError::WorkerOutOfRange)
    );
}

#[test]
fn full_result_ring_defers_publication_until_the_manifold_consumes_a_result() {
    let control = new_control(1);
    let pool = control.attach(Arc::new(Notify::new())).unwrap();
    // Submit directly to the pool to exercise its reserved result headroom. Driver admission is intentionally more conservative than this ring's capacity.
    for id in 0..CRYPTO_WORKER_RESULT_RING_DEPTH {
        pool.submit(probe(id as u8, CryptoJobClass::Latency));
        assert!(matches!(
            control.execute(ControlledWorkerId(0)),
            Ok(ControlledCryptoStep::Executed(_))
        ));
        assert!(matches!(
            control.publish(ControlledWorkerId(0)),
            Ok(ControlledCryptoStep::Published(_))
        ));
    }
    pool.submit(probe(
        CRYPTO_WORKER_RESULT_RING_DEPTH as u8,
        CryptoJobClass::Latency,
    ));
    control.execute(ControlledWorkerId(0)).unwrap();
    assert_eq!(
        control.publish(ControlledWorkerId(0)),
        Ok(ControlledCryptoStep::Backpressured)
    );
    assert_eq!(
        control.snapshot().unwrap(),
        ControlledCryptoSnapshot {
            queued: 0,
            computed: 1,
            published: CRYPTO_WORKER_RESULT_RING_DEPTH
        }
    );
    assert!(matches!(consume(&pool), CryptoResult::ScheduledTest(0)));
    assert!(matches!(
        control.publish(ControlledWorkerId(0)),
        Ok(ControlledCryptoStep::Published(_))
    ));
    for id in 1..=CRYPTO_WORKER_RESULT_RING_DEPTH {
        assert!(
            matches!(consume(&pool), CryptoResult::ScheduledTest(actual) if actual == id as u8)
        );
    }
    assert_eq!(pool.workers[0].outstanding_jobs.get(), 0);
    assert_eq!(pool.workers[0].outstanding_work.get(), 0);
    assert_eq!(
        control.snapshot().unwrap(),
        ControlledCryptoSnapshot {
            queued: 0,
            computed: 0,
            published: 0
        }
    );
}
