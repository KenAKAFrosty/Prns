use super::{CompletionPool, NO_AWAITER};
use crate::engine::{CommandId, PacketReceiptDelivered, Settlement};
use crate::units::RttMillis;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use portable_atomic::Ordering;

type Pool<const N: usize> = CompletionPool<CriticalSectionRawMutex, N>;

fn delivered(ms: u64) -> Settlement {
    Settlement::SendSinglePacket(Ok(PacketReceiptDelivered {
        rtt: RttMillis::new(ms),
        evidence: crate::engine::DeliveryEvidence::Proof(crate::engine::DeliveryProof::Implicit(
            crate::routing::dedup::PacketHash::new([0; 32]),
        )),
    }))
}

#[test]
fn the_pool_mints_a_distinct_id_each_time() {
    let pool: Pool<2> = CompletionPool::new();
    assert_eq!(pool.mint(), CommandId(0));
    assert_eq!(pool.mint(), CommandId(1));
    assert_eq!(pool.mint(), CommandId(2));
}

#[test]
fn the_pool_never_mints_the_free_slot_sentinel() {
    let pool: Pool<1> = CompletionPool::new();
    pool.next_id.store(NO_AWAITER, Ordering::Relaxed);
    assert_eq!(pool.mint(), CommandId(0));
}

#[test]
fn the_pool_bounds_concurrent_awaited_sends() {
    let pool: Pool<2> = CompletionPool::new();
    let first = pool.claim(CommandId(0));
    let second = pool.claim(CommandId(1));
    assert!(first.is_some() && second.is_some());
    assert_ne!(first, second);
    assert_eq!(
        pool.claim(CommandId(2)),
        None,
        "a full pool refuses a claim"
    );
}

#[test]
fn settle_wakes_only_the_slot_awaiting_that_id() {
    let pool: Pool<3> = CompletionPool::new();
    pool.claim(CommandId(10));
    pool.claim(CommandId(11));
    pool.claim(CommandId(12));
    assert!(
        !pool.settle(CommandId(99), delivered(1)),
        "no slot awaits 99"
    );
    assert!(pool.settle(CommandId(11), delivered(1)));
    assert!(pool.settle(CommandId(10), delivered(1)));
    assert!(pool.settle(CommandId(12), delivered(1)));
}

#[test]
fn a_settled_slot_frees_for_reuse() {
    let pool: Pool<1> = CompletionPool::new();
    let id = CommandId(0);
    assert!(pool.claim(id).is_some());
    assert_eq!(pool.claim(CommandId(1)), None, "full while id awaits");
    assert!(pool.settle(id, delivered(1)));
    assert!(
        pool.claim(CommandId(1)).is_some(),
        "the slot frees once settled"
    );
}

#[test]
fn a_cancelled_await_releases_its_slot_and_ignores_a_late_settlement() {
    let pool: Pool<1> = CompletionPool::new();
    let id = CommandId(0);
    let slot = pool.claim(id).expect("a slot");
    pool.release(slot, id);
    assert!(
        !pool.settle(id, delivered(1)),
        "a settlement for a released await fires nothing"
    );
    assert!(
        pool.claim(CommandId(1)).is_some(),
        "the released slot is reusable"
    );
}

#[test]
fn a_late_release_never_clobbers_a_newer_claimant() {
    let pool: Pool<1> = CompletionPool::new();
    let first = CommandId(0);
    let slot = pool.claim(first).expect("a slot");
    assert!(pool.settle(first, delivered(1)));

    let second = CommandId(1);
    assert_eq!(pool.claim(second), Some(slot), "the same slot is reused");
    pool.release(slot, first);
    assert!(
        pool.settle(second, delivered(2)),
        "the stale release left the new claimant intact"
    );
}
