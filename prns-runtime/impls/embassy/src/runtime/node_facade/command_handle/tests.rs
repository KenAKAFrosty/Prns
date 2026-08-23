use super::{CompletionPool, NO_AWAITER};
use crate::engine::{
    CommandId, IssuedCommand, PacketReceiptDelivered, PrnsCommand, SendGroupFailure,
    SendGroupRejection, SendPlainPacketFailure, Settlement, MAX_SEND_GROUP_PLAINTEXT_LEN,
    MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN,
};
use crate::runtime::SendError;
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use portable_atomic::Ordering;

type Pool<const N: usize> = CompletionPool<CriticalSectionRawMutex, N>;
const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

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

#[test]
fn plain_and_group_payloads_beyond_their_mdu_are_rejected_before_enqueueing() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<1>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let plain_oversize = [0u8; MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN + 1];
    let group_oversize = [0u8; MAX_SEND_GROUP_PLAINTEXT_LEN + 1];

    block_on(async {
        assert_eq!(
            handle.send_plain_packet(PEER, &plain_oversize).await,
            Err(SendError::<SendPlainPacketFailure>::PayloadTooLarge),
        );
        assert_eq!(
            handle.send_group_packet(PEER, &group_oversize).await,
            Err(SendError::<SendGroupFailure>::PayloadTooLarge),
        );
    });
    assert!(commands.try_receive().is_err());
}

#[test]
fn awaited_plain_and_group_sends_preserve_commands_and_typed_settlements() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<1>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);

    let (plain, ()) = block_on(join(handle.send_plain_packet(PEER, b"plain"), async {
        let issued = commands.receiver().receive().await;
        let PrnsCommand::SendPlainPacket(command) = issued.command else {
            panic!("plain command")
        };
        assert_eq!(command.destination, PEER);
        assert_eq!(command.payload.as_slice(), b"plain");
        assert!(completions.settle(issued.id, Settlement::SendPlainPacket(Ok(()))));
    }));
    assert_eq!(plain, Ok(()));

    let failure = SendGroupFailure::Rejected(SendGroupRejection::NoGroupKey);
    let (group, ()) = block_on(join(handle.send_group_packet(PEER, b"group"), async {
        let issued = commands.receiver().receive().await;
        let PrnsCommand::SendGroup(command) = issued.command else {
            panic!("group command")
        };
        assert_eq!(command.destination, PEER);
        assert_eq!(command.payload.as_slice(), b"group");
        assert!(completions.settle(issued.id, Settlement::SendGroup(Err(failure))));
    }));
    assert_eq!(group, Err(SendError::Failed(failure)));
}
