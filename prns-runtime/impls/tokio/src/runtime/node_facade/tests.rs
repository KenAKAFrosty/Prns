use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::{
    AnnounceNow, AnnounceNowFailure, EstablishLink, EstablishLinkFailure, Identify,
    PacketReceiptDelivered, PathFound, PrnsCommand, SendGroupFailure, SendPlainPacketFailure,
    Settlement, MAX_SEND_GROUP_PLAINTEXT_LEN, MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN,
    MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN,
};
use crate::identity::IdentityHash;
use crate::manifold::driver::HostCommand;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};
use crate::runtime::{RuntimeRequestHandlerError, SendError};
use crate::storage::TablePushError;
use crate::wire::DestinationHash;

use super::PrnsNodeHandle;

const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

fn delivered(ms: u64) -> PacketReceiptDelivered {
    PacketReceiptDelivered {
        rtt: crate::units::RttMillis::new(ms),
        evidence: crate::engine::DeliveryEvidence::Proof(crate::engine::DeliveryProof::Implicit(
            crate::routing::dedup::PacketHash::new([0; 32]),
        )),
    }
}

fn handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

#[tokio::test]
async fn payload_beyond_the_mdu_is_rejected_before_the_wire() {
    let (prns, _command_rx) = handle();
    let oversize = [0u8; MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN + 1];
    assert_eq!(
        prns.send_single_packet(PEER, &oversize).await,
        Err(SendError::PayloadTooLarge),
    );
}

#[tokio::test]
async fn plain_and_group_payloads_beyond_their_mdu_are_rejected_before_the_wire() {
    let (prns, _command_rx) = handle();
    let plain_oversize = [0u8; MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN + 1];
    assert_eq!(
        prns.send_plain_packet(PEER, &plain_oversize).await,
        Err(SendError::<SendPlainPacketFailure>::PayloadTooLarge),
    );
    let group_oversize = [0u8; MAX_SEND_GROUP_PLAINTEXT_LEN + 1];
    assert_eq!(
        prns.send_group_packet(PEER, &group_oversize).await,
        Err(SendError::<SendGroupFailure>::PayloadTooLarge),
    );
}

#[tokio::test]
async fn a_send_on_a_stopped_node_settles_as_node_stopped() {
    let (prns, command_rx) = handle();
    drop(command_rx);
    assert_eq!(
        prns.send_single_packet(PEER, b"ping").await,
        Err(SendError::NodeStopped),
    );
}

#[tokio::test]
async fn an_awaited_send_issues_the_completion_carrying_command() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let send = tokio::spawn(async move { issuer.send_single_packet(PEER, b"ping").await });

    match command_rx.recv().await.expect("the command was issued") {
        HostCommand::AwaitedEngine { issued, completion } => {
            assert!(matches!(issued.command, PrnsCommand::SendSinglePacket(_)));
            completion
                .send(Settlement::SendSinglePacket(Ok(delivered(7))))
                .expect("the awaiter is still parked");
        }
        _ => panic!("send_single must issue an AwaitedEngine command"),
    }

    assert_eq!(send.await.expect("the send task joins"), Ok(delivered(7)),);
}

#[tokio::test]
async fn awaited_plain_and_group_sends_issue_their_distinct_commands() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let plain = tokio::spawn(async move { issuer.send_plain_packet(PEER, b"plain").await });
    match command_rx.recv().await.expect("plain command") {
        HostCommand::AwaitedEngine { issued, completion } => {
            assert!(matches!(issued.command, PrnsCommand::SendPlainPacket(_)));
            completion
                .send(Settlement::SendPlainPacket(Ok(())))
                .expect("plain awaiter");
        }
        _ => panic!("plain send uses an awaited engine command"),
    }
    assert_eq!(plain.await.expect("plain task"), Ok(()));

    let issuer = prns.clone();
    let group = tokio::spawn(async move { issuer.send_group_packet(PEER, b"group").await });
    match command_rx.recv().await.expect("group command") {
        HostCommand::AwaitedEngine { issued, completion } => {
            assert!(matches!(issued.command, PrnsCommand::SendGroup(_)));
            completion
                .send(Settlement::SendGroup(Ok(())))
                .expect("group awaiter");
        }
        _ => panic!("group send uses an awaited engine command"),
    }
    assert_eq!(group.await.expect("group task"), Ok(()));
}

#[tokio::test]
async fn runtime_request_path_mutations_are_acknowledged() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let register = tokio::spawn(async move {
        issuer
            .register_request_path(PEER, "/page/new.mu", RequestPolicy::AllowAll)
            .await
    });

    match command_rx.recv().await.expect("registration command") {
        HostCommand::RegisterRequestHandler {
            destination,
            path_hash,
            policy,
            ready,
        } => {
            assert_eq!(destination, PEER);
            assert_eq!(path_hash, RequestPathHash::of("/page/new.mu"));
            assert_eq!(policy, RequestPolicy::AllowAll);
            ready.send(Ok(())).expect("registration waiter");
        }
        _ => panic!("request path registration uses its host command"),
    }
    assert_eq!(register.await.expect("registration joins"), Ok(()));

    let issuer = prns.clone();
    let unregister =
        tokio::spawn(async move { issuer.unregister_request_path(PEER, "/page/new.mu").await });
    match command_rx.recv().await.expect("unregistration command") {
        HostCommand::UnregisterRequestHandler {
            destination,
            path_hash,
            ready,
        } => {
            assert_eq!(destination, PEER);
            assert_eq!(path_hash, RequestPathHash::of("/page/new.mu"));
            ready.send(true).expect("unregistration waiter");
        }
        _ => panic!("request path unregistration uses its host command"),
    }
    assert_eq!(unregister.await.expect("unregistration joins"), Ok(true));
}

#[tokio::test]
async fn runtime_request_path_registration_reports_capacity_and_shutdown() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let register = tokio::spawn(async move {
        issuer
            .register_request_path(PEER, "/page/full.mu", RequestPolicy::AllowAll)
            .await
    });
    let HostCommand::RegisterRequestHandler { ready, .. } =
        command_rx.recv().await.expect("registration command")
    else {
        panic!("request path registration uses its host command");
    };
    ready
        .send(Err(TablePushError::TableFull))
        .expect("registration waiter");
    assert_eq!(
        register.await.expect("registration joins"),
        Err(RuntimeRequestHandlerError::TableFull)
    );

    drop(command_rx);
    assert_eq!(
        prns.unregister_request_path(PEER, "/page/full.mu").await,
        Err(RuntimeRequestHandlerError::NodeStopped)
    );
}

#[tokio::test]
async fn establish_link_resolves_the_link_id_from_the_settlement() {
    use crate::engine::LinkEstablished;

    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let establish = tokio::spawn(async move { issuer.establish_link(PEER).await });

    match command_rx.recv().await.expect("the command was issued") {
        HostCommand::AwaitedEngine { issued, completion } => {
            assert_eq!(
                issued.command,
                PrnsCommand::EstablishLink(EstablishLink { destination: PEER }),
            );
            completion
                .send(Settlement::EstablishLink(Ok(LinkEstablished {
                    link_id: LinkId::new([0x42; 16]),
                    rtt_millis: 11,
                })))
                .expect("the awaiter is still parked");
        }
        _ => panic!("establish_link must issue an AwaitedEngine command"),
    }

    assert_eq!(
        establish.await.expect("the establish task joins"),
        Ok(LinkId::new([0x42; 16])),
    );
}

#[tokio::test]
async fn establish_link_with_rtt_preserves_the_full_settlement() {
    use crate::engine::LinkEstablished;

    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let establish = tokio::spawn(async move { issuer.establish_link_with_rtt(PEER).await });
    let established = LinkEstablished {
        link_id: LinkId::new([0x42; 16]),
        rtt_millis: 11,
    };

    match command_rx.recv().await.expect("the command was issued") {
        HostCommand::AwaitedEngine { completion, .. } => {
            completion
                .send(Settlement::EstablishLink(Ok(established)))
                .expect("the awaiter is still parked");
        }
        _ => panic!("establish_link_with_rtt must issue an AwaitedEngine command"),
    }

    assert_eq!(
        establish.await.expect("the establish task joins"),
        Ok(established)
    );
}

#[tokio::test]
async fn establish_link_surfaces_a_typed_failure() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let establish = tokio::spawn(async move { issuer.establish_link(PEER).await });

    let HostCommand::AwaitedEngine { completion, .. } =
        command_rx.recv().await.expect("the command was issued")
    else {
        panic!("establish_link must issue an AwaitedEngine command");
    };
    completion
        .send(Settlement::EstablishLink(Err(
            EstablishLinkFailure::Timeout,
        )))
        .expect("the awaiter is still parked");

    assert_eq!(
        establish.await.expect("the establish task joins"),
        Err(SendError::Failed(EstablishLinkFailure::Timeout)),
    );
}

#[tokio::test]
async fn identify_awaits_the_matching_write_settlement() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let link_id = LinkId::new([0x42; 16]);
    let identity = IdentityHash::new([0x24; 16]);
    let identify = tokio::spawn(async move { issuer.identify(link_id, identity).await });

    let HostCommand::AwaitedEngine { issued, completion } =
        command_rx.recv().await.expect("the command was issued")
    else {
        panic!("identify must issue an awaited engine command");
    };
    assert_eq!(
        issued.command,
        PrnsCommand::Identify(Identify { link_id, identity })
    );
    completion
        .send(Settlement::Identify(Ok(())))
        .expect("the awaiter is still parked");

    assert_eq!(identify.await.expect("the identify task joins"), Ok(()));
}

#[tokio::test]
async fn request_path_mints_an_id_and_awaits_the_typed_result() {
    let (prns, mut command_rx) = handle();
    let issuer = prns.clone();
    let requested = tokio::spawn(async move { issuer.request_path(PEER).await });

    let HostCommand::AwaitedEngine { issued, completion } =
        command_rx.recv().await.expect("the command was issued")
    else {
        panic!("request_path must issue an awaited engine command");
    };
    let PrnsCommand::RequestPath(request) = issued.command else {
        panic!("request_path must issue its matching engine command");
    };
    assert_eq!(request.destination, PEER);
    completion
        .send(Settlement::RequestPath(Ok(PathFound {
            hops: crate::units::HopCount(3),
        })))
        .expect("the awaiter is still parked");

    assert_eq!(
        requested.await.expect("the request task joins"),
        Ok(PathFound {
            hops: crate::units::HopCount(3),
        })
    );
}

#[tokio::test]
async fn announce_now_awaits_and_surfaces_its_typed_settlement() {
    let (prns, mut command_rx) = handle();
    let command = AnnounceNow {
        destination: PEER,
        target: crate::engine::AnnounceTarget::AllInterfaces,
        app_data: crate::engine::AnnounceAppData::Registered,
    };
    let expected = command.clone();
    let issuer = prns.clone();
    let announced = tokio::spawn(async move { issuer.announce_now(command).await });
    let HostCommand::AwaitedEngine { issued, completion } =
        command_rx.recv().await.expect("the command was issued")
    else {
        panic!("announce_now must issue an awaited engine command");
    };
    assert_eq!(issued.command, PrnsCommand::AnnounceNow(expected));
    completion
        .send(Settlement::AnnounceNow(Err(AnnounceNowFailure::Rejected(
            crate::engine::AnnounceNowRejection::UnknownDestination,
        ))))
        .expect("the awaiter is still parked");
    assert_eq!(
        announced.await.expect("the announce task joins"),
        Err(crate::runtime::AnnounceNowError::Rejected(
            crate::engine::AnnounceNowRejection::UnknownDestination,
        )),
    );
}

#[test]
fn the_prns_node_api_trait_dispatches_to_the_handle() {
    use crate::routing::links::LinkId;
    use crate::runtime::PrnsNodeApi;

    let (prns, mut command_rx) = handle();
    let queued = PrnsNodeApi::close_link(&prns, LinkId::new([3; 16]));
    assert!(
        queued,
        "the trait method reaches the handle and queues the close"
    );
    assert!(
        matches!(command_rx.try_recv(), Ok(HostCommand::Engine(_))),
        "dispatched through PrnsNodeApi, the close rode the channel"
    );
}
