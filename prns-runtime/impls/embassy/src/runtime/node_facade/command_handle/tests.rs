use super::{CompletionPool, JournalRoute, RequestSlotGuard, ResponseCapture, NO_AWAITER};
use crate::engine::{
    AnnounceAppData, AnnounceNow, AnnounceNowFailure, AnnounceNowRejection, AnnounceTarget,
    CloseRemoteControlPairing, CloseRemoteControlPairingFailure, CommandId, DeliveryEvidence,
    IssuedCommand, Journaled, OpenRemoteControlPairing, PacketReceiptDelivered, PrnsCommand,
    RemoteControlPairingOpened, SendGroupFailure, SendGroupRejection, SendPlainPacketFailure,
    SendRequestFailure, SetRegisteredAnnounceAppData, SetRegisteredAnnounceAppDataFailure,
    SetRegisteredAnnounceAppDataRejection, Settlement, MAX_SEND_GROUP_PLAINTEXT_LEN,
    MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN,
};
use crate::remote_control::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingEndpoint,
    RemoteControlPairingExpiresAfter, RemoteControlPairingInvitationCode,
    RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
    RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlTargetAccess,
    RemoteControlTargetIdentity,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::runtime::remote_control_controller_grants::{
    RemoteControlControllerGrantCommand, RemoteControlControllerGrantCompletion,
};
use crate::runtime::remote_control_target_accesses::{
    RemoteControlTargetAccessCommand, RemoteControlTargetAccessCompletion,
};
use crate::runtime::{
    AnnounceNowError, RemoteControlControllerGrantControl, RemoteControlPairingControlError,
    RemoteControlTargetAccessControl, ResolvedRemoteControlTarget,
    RevokeRemoteControlControllerControlError, SendError, SetRegisteredAnnounceAppDataError,
    SetRemoteControlControllerGrantControlError, SetRemoteControlControllerGrantServiceError,
};
use crate::units::{ByteLimit, DurationMillis, InstantMillis, RttMillis};
use crate::wire::DestinationHash;
use embassy_futures::{block_on, join::join};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use portable_atomic::Ordering;

type Pool<const COMPLETIONS: usize> = CompletionPool<CriticalSectionRawMutex, COMPLETIONS>;
const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

fn open_pairing() -> OpenRemoteControlPairing {
    OpenRemoteControlPairing {
        target: crate::engine::EgressTarget::AllInterfaces,
        expires_after: RemoteControlPairingExpiresAfter::try_from(DurationMillis(60_000)).unwrap(),
        attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(DurationMillis(30_000))
            .unwrap(),
        permissions: RemoteControlPairingPermissions::try_from(RemoteControlRequestSet::only(
            RemoteControlRequestKind::Describe,
        ))
        .unwrap(),
        public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(b"node".as_slice())
            .unwrap(),
    }
}

fn opened_pairing() -> RemoteControlPairingOpened {
    RemoteControlPairingOpened {
        endpoint: RemoteControlPairingEndpoint::from(
            &crate::remote_control::RemoteControlPairingIdentity::new(
                crate::identity::IdentityHash::new([0x51; 16]),
            ),
        ),
        expires_at: InstantMillis(61_000),
        invitation_code: RemoteControlPairingInvitationCode::from_value(0x1234_ABCD),
    }
}

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
fn remote_control_controller_grants_preserves_exact_set_and_revoke_settlements() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<0>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let previous = super::super::test_remote_control_grant(
        crate::remote_control::RemoteControlRequestKind::Describe,
    );
    let grant = super::super::test_remote_control_grant(
        crate::remote_control::RemoteControlRequestKind::AnnounceSelf,
    );

    let (set, ()) = block_on(join(
        handle.set_remote_control_controller_grant(grant),
        async {
            let RemoteControlControllerGrantCommand::SetControllerGrant {
                id,
                grant: submitted,
            } = handle.next_remote_control_controller_grant_command().await
            else {
                panic!("set controller grant command")
            };
            assert_eq!(submitted, grant);
            assert!(handle.settle_remote_control_controller_grant(
                id,
                RemoteControlControllerGrantCompletion::ControllerGrantSet(Ok(
                    crate::remote_control::SetRemoteControlControllerGrantOutcome::Updated {
                        previous,
                    },
                )),
            ));
        },
    ));
    assert_eq!(
        set,
        Ok(crate::remote_control::SetRemoteControlControllerGrantOutcome::Updated { previous }),
    );

    let (revoke, ()) = block_on(join(
        handle.revoke_remote_control_controller(*grant.controller()),
        async {
            let RemoteControlControllerGrantCommand::RevokeController { id, controller } =
                handle.next_remote_control_controller_grant_command().await
            else {
                panic!("revoke controller command")
            };
            assert_eq!(controller, *grant.controller());
            assert!(handle.settle_remote_control_controller_grant(
                id,
                RemoteControlControllerGrantCompletion::ControllerRevoked(Ok(
                    crate::remote_control::RevokeRemoteControlControllerOutcome::Revoked { grant },
                )),
            ));
        },
    ));
    assert_eq!(
        revoke,
        Ok(crate::remote_control::RevokeRemoteControlControllerOutcome::Revoked { grant }),
    );
}

#[test]
fn remote_control_target_resolution_preserves_the_exact_target_and_settlement() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<0>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let service = super::super::test_remote_control_service();
    let identities = service
        .configuration()
        .unwrap()
        .identity_secrets()
        .identities();
    let target = identities.target().identity_hash();
    let access = RemoteControlTargetAccess::new(
        RemoteControlTargetIdentity::new(*identities.target().public_keys()),
        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
    )
    .unwrap();
    let expected = ResolvedRemoteControlTarget::from((identities.controller(), &access));
    let completion = ResolvedRemoteControlTarget::from((identities.controller(), &access));

    let (resolved, ()) = block_on(join(handle.resolve_remote_control_target(target), async {
        let RemoteControlTargetAccessCommand::ResolveTarget {
            id,
            target: submitted,
        } = handle.next_remote_control_target_access_command().await
        else {
            panic!("resolve target command")
        };
        assert_eq!(submitted, target);
        assert!(handle.settle_remote_control_target_access(
            id,
            RemoteControlTargetAccessCompletion::Resolved(Ok(completion)),
        ));
    }));
    assert_eq!(resolved, Ok(expected));
}

#[test]
fn remote_control_controller_grants_maps_capacity_and_busy_without_crossing_operation_spaces() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<0>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let grant = super::super::test_remote_control_grant(
        crate::remote_control::RemoteControlRequestKind::Describe,
    );

    let (capacity, ()) = block_on(join(
        handle.set_remote_control_controller_grant(grant),
        async {
            let RemoteControlControllerGrantCommand::SetControllerGrant { id, .. } =
                handle.next_remote_control_controller_grant_command().await
            else {
                panic!("set controller grant command")
            };
            assert!(handle.settle_remote_control_controller_grant(
                id,
                RemoteControlControllerGrantCompletion::ControllerGrantSet(Err(
                    SetRemoteControlControllerGrantServiceError::CapacityExhausted,
                )),
            ));
        },
    ));
    assert_eq!(
        capacity,
        Err(SetRemoteControlControllerGrantControlError::CapacityExhausted),
    );

    let held = completions.mint();
    assert!(completions.remote_control_controller_grants.submit(
        RemoteControlControllerGrantCommand::RevokeController {
            id: held,
            controller: *grant.controller(),
        },
    ));
    assert_eq!(
        block_on(handle.set_remote_control_controller_grant(grant)),
        Err(SetRemoteControlControllerGrantControlError::Busy),
    );
    assert_eq!(
        block_on(handle.revoke_remote_control_controller(*grant.controller())),
        Err(RevokeRemoteControlControllerControlError::Busy),
    );
    completions.remote_control_controller_grants.release(held);
}

#[test]
fn remote_control_controller_grants_ignores_a_settlement_for_a_released_operation() {
    let completions = Pool::<0>::new();
    let grant = super::super::test_remote_control_grant(
        crate::remote_control::RemoteControlRequestKind::Describe,
    );
    let released = completions.mint();
    assert!(completions.remote_control_controller_grants.submit(
        RemoteControlControllerGrantCommand::RevokeController {
            id: released,
            controller: *grant.controller(),
        },
    ));
    completions
        .remote_control_controller_grants
        .release(released);
    let current = completions.mint();
    assert!(completions.remote_control_controller_grants.submit(
        RemoteControlControllerGrantCommand::RevokeController {
            id: current,
            controller: *grant.controller(),
        },
    ));
    assert!(matches!(
        block_on(completions.remote_control_controller_grants.next_command()),
        RemoteControlControllerGrantCommand::RevokeController { id, .. } if id == current,
    ));

    assert!(!completions.remote_control_controller_grants.settle(
        released,
        RemoteControlControllerGrantCompletion::ControllerRevoked(Ok(
            crate::remote_control::RevokeRemoteControlControllerOutcome::NotFound,
        )),
    ));
    assert!(completions.remote_control_controller_grants.settle(
        current,
        RemoteControlControllerGrantCompletion::ControllerRevoked(Ok(
            crate::remote_control::RevokeRemoteControlControllerOutcome::NotFound,
        )),
    ));
    assert!(matches!(
        block_on(
            completions
                .remote_control_controller_grants
                .completion(current)
        ),
        RemoteControlControllerGrantCompletion::ControllerRevoked(Ok(
            crate::remote_control::RevokeRemoteControlControllerOutcome::NotFound,
        )),
    ));
    assert!(!completions.remote_control_controller_grants.settle(
        current,
        RemoteControlControllerGrantCompletion::ControllerRevoked(Ok(
            crate::remote_control::RevokeRemoteControlControllerOutcome::NotFound,
        )),
    ));
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
fn a_cancelled_internal_pairing_settlement_releases_its_dedicated_slot() {
    let pool: Pool<0> = CompletionPool::new();
    let cancelled = CommandId(0);
    assert!(pool.remote_control_pairing_settlement.claim(cancelled));
    pool.remote_control_pairing_settlement.release(cancelled);
    let mut routed = None;
    assert!(matches!(
        pool.route_settlement(cancelled, delivered(1), |settlement| {
            routed = Some(settlement);
        }),
        JournalRoute::Application,
    ));
    assert_eq!(routed, Some(delivered(1)));
    assert!(pool.remote_control_pairing_settlement.claim(CommandId(1)));
}

#[test]
fn an_unclaimed_settlement_moves_to_the_application_route() {
    let pool: Pool<0> = CompletionPool::new();
    let mut routed = None;
    let route = pool.route_settlement(CommandId(7), delivered(11), |settlement| {
        routed = Some(settlement);
    });

    assert!(matches!(route, JournalRoute::Application));
    assert_eq!(routed, Some(delivered(11)));
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
        ResponseCapture::NotAwaited,
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
fn pairing_lifecycle_preserves_commands_settlements_and_completion_capacity() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<1>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let expected = open_pairing();
    let opened = opened_pairing();
    let (result, ()) = block_on(join(
        handle.open_remote_control_pairing(expected.clone()),
        async {
            let issued = commands.receiver().receive().await;
            assert_eq!(
                issued.command,
                PrnsCommand::OpenRemoteControlPairing(expected),
            );
            assert!(
                completions.settle(issued.id, Settlement::OpenRemoteControlPairing(Ok(opened)),)
            );
        },
    ));
    assert_eq!(result, Ok(opened_pairing()));

    let failure = CloseRemoteControlPairingFailure::IdentityNotHeld;
    let (result, ()) = block_on(join(handle.close_remote_control_pairing(), async {
        let issued = commands.receiver().receive().await;
        assert_eq!(
            issued.command,
            PrnsCommand::CloseRemoteControlPairing(CloseRemoteControlPairing),
        );
        assert!(completions.settle(
            issued.id,
            Settlement::CloseRemoteControlPairing(Err(failure)),
        ));
    }));
    assert_eq!(
        result,
        Err(RemoteControlPairingControlError::Failed(failure)),
    );

    let no_completions = Pool::<0>::new();
    let bounded = super::PrnsNodeHandle::new(commands.sender(), &no_completions);
    assert_eq!(
        block_on(bounded.open_remote_control_pairing(open_pairing())),
        Err(RemoteControlPairingControlError::Busy),
    );
}

#[test]
fn internal_pairing_settlement_is_independent_of_public_completion_capacity() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<0>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let announce = AnnounceNow {
        destination: PEER,
        target: AnnounceTarget::AllInterfaces,
        app_data: AnnounceAppData::Registered,
    };
    let expected = announce.clone();
    let failure = AnnounceNowFailure::Rejected(AnnounceNowRejection::UnknownDestination);

    let (result, ()) = block_on(join(handle.settle_pairing_command(announce), async {
        let issued = commands.receiver().receive().await;
        assert_eq!(issued.command, PrnsCommand::AnnounceNow(expected));
        assert!(matches!(
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::AnnounceNow(Err(failure)),
                },
                |_| panic!("internal pairing settlement reached the application"),
            ),
            JournalRoute::Awaiter,
        ));
    }));

    assert_eq!(
        result,
        Err(RemoteControlPairingControlError::Failed(failure)),
    );
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
fn registered_announce_app_data_update_awaits_and_preserves_its_typed_settlement() {
    let commands = Channel::<CriticalSectionRawMutex, IssuedCommand, 1>::new();
    let completions = Pool::<1>::new();
    let handle = super::PrnsNodeHandle::new(commands.sender(), &completions);
    let set = SetRegisteredAnnounceAppData {
        destination: PEER,
        app_data: crate::routing::announce::emit::AnnounceAppDataBytes::from_slice(b"default")
            .expect("valid app data"),
    };
    let expected = set.clone();
    let failure = SetRegisteredAnnounceAppDataFailure::Rejected(
        SetRegisteredAnnounceAppDataRejection::UnknownDestination,
    );

    let (result, ()) = block_on(join(handle.set_registered_announce_app_data(set), async {
        let issued = commands.receiver().receive().await;
        assert_eq!(
            issued.command,
            PrnsCommand::SetRegisteredAnnounceAppData(expected),
        );
        assert!(completions.settle(
            issued.id,
            Settlement::SetRegisteredAnnounceAppData(Err(failure)),
        ));
    }));

    assert_eq!(
        result,
        Err(SetRegisteredAnnounceAppDataError::Rejected(
            SetRegisteredAnnounceAppDataRejection::UnknownDestination,
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
            handle.route_journaled(
                Journaled::ResponseReceived {
                    command_id: issued.id,
                    link_id,
                    request_id: RequestId([0xA9; 16]),
                    data: &response,
                },
                |_| {},
            ),
            JournalRoute::Awaiter,
        ));
        assert!(matches!(
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::new(29),
                        evidence: DeliveryEvidence::Response,
                    })),
                },
                |_| {},
            ),
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
                handle.route_journaled(
                    Journaled::ResponseSegmentReceived {
                        command_id: issued.id,
                        link_id,
                        request_id: RequestId([0x54; 16]),
                        segment_index,
                        total_segments: 2,
                        data,
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));
        }
        assert!(matches!(
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                        rtt: RttMillis::new(31),
                        evidence: DeliveryEvidence::Response,
                    })),
                },
                |_| {},
            ),
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
            handle.route_journaled(
                Journaled::CommandSettled {
                    id: issued.id,
                    settlement: Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
                },
                |_| {},
            ),
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
                handle.route_journaled(
                    Journaled::ResponseReceived {
                        command_id: issued.id,
                        link_id,
                        request_id: RequestId([0x98; 16]),
                        data: &[1, 2, 3, 4],
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));
            assert!(matches!(
                handle.route_journaled(
                    Journaled::CommandSettled {
                        id: issued.id,
                        settlement: Settlement::SendRequest(Ok(PacketReceiptDelivered {
                            rtt: RttMillis::new(41),
                            evidence: DeliveryEvidence::Response,
                        })),
                    },
                    |_| {},
                ),
                JournalRoute::Awaiter,
            ));
        },
    ));

    assert_eq!(
        result,
        Err(SendError::Failed(SendRequestFailure::ResponseTooLarge)),
    );
}
