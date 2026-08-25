use super::{CompletionPool, JournalRoute, RequestSlotGuard, NO_AWAITER};
use crate::engine::{
    AnnounceAppData, AnnounceNow, AnnounceNowFailure, AnnounceNowRejection, AnnounceTarget,
    CommandId, DeliveryEvidence, IssuedCommand, Journaled, PacketReceiptDelivered, PrnsCommand,
    SendGroupFailure, SendGroupRejection, SendPlainPacketFailure, SendRequestFailure, Settlement,
    MAX_SEND_GROUP_PLAINTEXT_LEN, MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::runtime::{AnnounceNowError, SendError};
use crate::units::{ByteLimit, RttMillis};
use crate::wire::DestinationHash;
use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use portable_atomic::Ordering;

type Pool<const COMPLETIONS: usize> = CompletionPool<CriticalSectionRawMutex, COMPLETIONS>;
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
    let first = pool.claim_settlement(CommandId(0));
    let second = pool.claim_settlement(CommandId(1));
    assert!(first.is_some() && second.is_some());
    assert_ne!(first, second);
    assert_eq!(
        pool.claim_settlement(CommandId(2)),
        None,
        "a full pool refuses a claim"
    );
}

#[test]
fn request_completions_are_independently_bounded() {
    let pool = CompletionPool::<CriticalSectionRawMutex, 1, 1, 4>::new();
    assert!(pool.claim_settlement(CommandId(0)).is_some());
    assert!(pool.claim_request(CommandId(1)).is_some());
    assert_eq!(pool.claim_settlement(CommandId(2)), None);
    assert_eq!(pool.claim_request(CommandId(3)), None);
}

#[test]
fn response_capacity_costs_memory_only_when_request_slots_exist() {
    const RESPONSE_CAPACITY: usize = crate::runtime::RemoteControlDescribe::RESPONSE_CAPACITY;
    type NoRequests = CompletionPool<CriticalSectionRawMutex, 4, 0, 0>;
    type CapacityWithoutRequests = CompletionPool<CriticalSectionRawMutex, 4, 0, RESPONSE_CAPACITY>;
    type OneRequest = CompletionPool<CriticalSectionRawMutex, 4, 1, RESPONSE_CAPACITY>;

    assert_eq!(
        core::mem::size_of::<NoRequests>(),
        core::mem::size_of::<CapacityWithoutRequests>(),
    );
    assert!(core::mem::size_of::<OneRequest>() > core::mem::size_of::<NoRequests>());
}

#[test]
fn settle_wakes_only_the_slot_awaiting_that_id() {
    let pool: Pool<3> = CompletionPool::new();
    pool.claim_settlement(CommandId(10));
    pool.claim_settlement(CommandId(11));
    pool.claim_settlement(CommandId(12));
    assert!(
        !pool.settle(CommandId(99), delivered(1)),
        "no slot awaits 99"
    );
    assert!(pool.settle(CommandId(11), delivered(1)));
    assert!(pool.settle(CommandId(10), delivered(1)));
    assert!(pool.settle(CommandId(12), delivered(1)));
}

#[test]
fn a_settled_slot_stays_claimed_until_the_waiter_releases_it() {
    let pool: Pool<1> = CompletionPool::new();
    let id = CommandId(0);
    let slot = pool.claim_settlement(id).expect("a slot");
    assert_eq!(
        pool.claim_settlement(CommandId(1)),
        None,
        "full while id awaits"
    );
    assert!(pool.settle(id, delivered(1)));
    assert_eq!(
        pool.claim_settlement(CommandId(1)),
        None,
        "the waiter still owns its settled signal"
    );
    pool.release(slot, id);
    assert!(
        pool.claim_settlement(CommandId(1)).is_some(),
        "the slot frees once released"
    );
}

#[test]
fn a_cancelled_await_releases_its_slot_and_ignores_a_late_settlement() {
    let pool: Pool<1> = CompletionPool::new();
    let id = CommandId(0);
    let slot = pool.claim_settlement(id).expect("a slot");
    pool.release(slot, id);
    assert!(
        !pool.settle(id, delivered(1)),
        "a settlement for a released await fires nothing"
    );
    assert!(
        pool.claim_settlement(CommandId(1)).is_some(),
        "the released slot is reusable"
    );
}

#[test]
fn a_cancelled_request_releases_its_slot_and_routes_late_delivery_to_the_application() {
    let pool = CompletionPool::<CriticalSectionRawMutex, 0, 1, 4>::new();
    let id = CommandId(0);
    let slot = pool.claim_request(id).expect("a request slot");
    drop(RequestSlotGuard {
        pool: &pool,
        slot,
        id,
    });

    assert!(matches!(
        pool.capture_response(id, &[1, 2]),
        JournalRoute::Application,
    ));
    assert!(!pool.settle_request(
        id,
        Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
    ));
    assert!(pool.claim_request(CommandId(1)).is_some());
}

#[test]
fn a_late_release_never_clobbers_a_newer_claimant() {
    let pool: Pool<1> = CompletionPool::new();
    let first = CommandId(0);
    let slot = pool.claim_settlement(first).expect("a slot");
    assert!(pool.settle(first, delivered(1)));
    pool.release(slot, first);

    let second = CommandId(1);
    assert_eq!(
        pool.claim_settlement(second),
        Some(slot),
        "the same slot is reused"
    );
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

#[test]
fn announce_now_awaits_and_preserves_its_typed_settlement() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<1>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let announce = AnnounceNow {
        destination: PEER,
        target: AnnounceTarget::AllInterfaces,
        app_data: AnnounceAppData::Registered,
    };
    let expected = announce.clone();
    let failure = AnnounceNowFailure::Rejected(AnnounceNowRejection::UnknownDestination);

    let (result, ()) = block_on(join(handle.announce_now(announce), async {
        let issued = commands.receiver().receive().await;
        assert_eq!(issued.command, PrnsCommand::AnnounceNow(expected));
        assert!(completions.settle(issued.id, Settlement::AnnounceNow(Err(failure))));
    }));

    assert_eq!(
        result,
        Err(AnnounceNowError::Rejected(
            AnnounceNowRejection::UnknownDestination,
        )),
    );
}

#[test]
fn bounded_request_captures_the_response_before_its_borrow_expires() {
    const RESPONSE_BYTES: usize = 8;
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = CompletionPool::<CriticalSectionRawMutex, 0, 1, RESPONSE_BYTES>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let link_id = LinkId::new([0x21; 16]);
    let path_hash = RequestPathHash::of("/bounded");

    let (result, ()) = block_on(join(handle.request(link_id, path_hash, b"ask"), async {
        let issued = commands.receiver().receive().await;
        let PrnsCommand::SendRequest(request) = issued.command else {
            panic!("request command")
        };
        assert_eq!(request.link_id, link_id);
        assert_eq!(request.path_hash, path_hash);
        assert_eq!(request.data.as_slice(), b"ask");
        assert_eq!(
            request.maximum_response_bytes,
            ByteLimit::Maximum(RESPONSE_BYTES as u64),
        );
        let response = [0x43, 0x65, 0x87];
        assert!(matches!(
            handle.route_journaled(&Journaled::ResponseReceived {
                command_id: issued.id,
                link_id,
                request_id: RequestId([0xA9; 16]),
                data: &response,
            }),
            JournalRoute::Awaiter,
        ));
        assert!(matches!(
            handle.route_journaled(&Journaled::CommandSettled {
                id: issued.id,
                settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(29),
                    evidence: DeliveryEvidence::Response,
                })),
            }),
            JournalRoute::Awaiter,
        ));
    }));

    let Ok((response, rtt)) = result else {
        panic!("bounded response")
    };
    assert_eq!(response.as_slice(), [0x43, 0x65, 0x87]);
    assert_eq!(rtt, RttMillis::new(29));
}

#[test]
fn bounded_request_concatenates_segments_and_preserves_failures() {
    const RESPONSE_BYTES: usize = 5;
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = CompletionPool::<CriticalSectionRawMutex, 0, 1, RESPONSE_BYTES>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let link_id = LinkId::new([0x32; 16]);
    let path_hash = RequestPathHash::of("/segmented");

    let (result, ()) = block_on(join(handle.request(link_id, path_hash, &[]), async {
        let issued = commands.receiver().receive().await;
        for (segment_index, data) in [(0, &[1, 2][..]), (1, &[3, 4, 5][..])] {
            assert!(matches!(
                handle.route_journaled(&Journaled::ResponseSegmentReceived {
                    command_id: issued.id,
                    link_id,
                    request_id: RequestId([0x54; 16]),
                    segment_index,
                    total_segments: 2,
                    data,
                }),
                JournalRoute::Awaiter,
            ));
        }
        assert!(matches!(
            handle.route_journaled(&Journaled::CommandSettled {
                id: issued.id,
                settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(31),
                    evidence: DeliveryEvidence::Response,
                })),
            }),
            JournalRoute::Awaiter,
        ));
    }));
    let Ok((response, _rtt)) = result else {
        panic!("segmented response")
    };
    assert_eq!(response.as_slice(), [1, 2, 3, 4, 5]);

    let (result, ()) = block_on(join(handle.request(link_id, path_hash, &[]), async {
        let issued = commands.receiver().receive().await;
        assert!(matches!(
            handle.route_journaled(&Journaled::CommandSettled {
                id: issued.id,
                settlement: Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
            }),
            JournalRoute::Awaiter,
        ));
    }));
    assert_eq!(result, Err(SendError::Failed(SendRequestFailure::Timeout)),);
}

#[test]
fn bounded_request_refuses_response_bytes_beyond_its_static_capacity() {
    const RESPONSE_BYTES: usize = 3;
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = CompletionPool::<CriticalSectionRawMutex, 0, 1, RESPONSE_BYTES>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let link_id = LinkId::new([0x76; 16]);

    let (result, ()) = block_on(join(
        handle.request(link_id, RequestPathHash::of("/capacity"), &[]),
        async {
            let issued = commands.receiver().receive().await;
            assert!(matches!(
                handle.route_journaled(&Journaled::ResponseReceived {
                    command_id: issued.id,
                    link_id,
                    request_id: RequestId([0x98; 16]),
                    data: &[1, 2, 3, 4],
                }),
                JournalRoute::Awaiter,
            ));
            assert!(matches!(
                handle.route_journaled(&Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::new(41),
                        evidence: DeliveryEvidence::Response,
                    })),
                }),
                JournalRoute::Awaiter,
            ));
        },
    ));

    assert_eq!(
        result,
        Err(SendError::Failed(SendRequestFailure::ResponseTooLarge)),
    );
}
