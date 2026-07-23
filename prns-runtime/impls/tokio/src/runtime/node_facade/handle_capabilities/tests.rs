use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::InstantMillis;
use crate::interfaces::{PacketPhyStats, RssiDbm};
use crate::manifold::driver::HostCommand;
use crate::node_introspection::{
    AnnounceRateSnapshot, NodeIntrospection, NodeIntrospectionRequest,
};
use crate::routing::dedup::PacketHash;
use crate::routing::{
    BlackholeExpiry, BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome,
};
#[cfg(feature = "runtime-metrics")]
use crate::runtime::RuntimeMetricsSnapshot;
use crate::runtime::{
    ClearAnnounceQueuesOutcome, DropRouteOutcome, DropRoutesViaOutcome, IdentityBlackholeControl,
    IdentityBlackholeControlError, IdentityBlackholeHostCommand, IdentityBlackholeSource,
    IdentityBlackholeSourceError, RoutingControl, RoutingControlError,
};
use crate::wire::{DestinationHash, TransportId};

use super::super::PrnsNodeHandle;

const PEER: DestinationHash = DestinationHash::new([0xAB; 16]);

fn handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

#[test]
fn inspection_reads_the_runtime_packet_phy_store() {
    let (handle, _command_rx) = handle();
    let packet_hash = PacketHash::new([0x42; 32]);
    let packet_phy = PacketPhyStats {
        rssi: Some(RssiDbm::new(-82)),
        snr: None,
        quality: None,
    };
    handle.store.remember_packet_phy(packet_hash, packet_phy);

    assert_eq!(
        NodeIntrospection::packet_phy(&handle, packet_hash),
        Some(packet_phy)
    );
}

#[cfg(feature = "runtime-metrics")]
#[tokio::test]
async fn metrics_snapshots_are_requested_from_the_manifold() {
    let (handle, mut command_rx) = handle();
    let expected = RuntimeMetricsSnapshot {
        taken_at: InstantMillis(42),
        engine: Default::default(),
        egress: Default::default(),
        crypto: None,
        reliability: Default::default(),
    };
    let snapshotting = tokio::spawn(async move { handle.metrics_snapshot().await });

    let HostCommand::SnapshotMetrics { reply } = command_rx.recv().await.unwrap() else {
        panic!("expected a metrics snapshot command");
    };
    reply.send(expected.clone()).unwrap();

    assert_eq!(snapshotting.await.unwrap(), Some(expected));
}

#[tokio::test]
async fn announce_rate_introspection_resolves_its_manifold_snapshot() {
    let (handle, mut command_rx) = handle();
    let expected = std::vec![AnnounceRateSnapshot {
        destination: DestinationHash::new([0x42; 16]),
        last_allowed_announce_at: InstantMillis(20),
        blocked_until: InstantMillis(0),
        rate_violations: 1,
        observed_at: std::vec![InstantMillis(10), InstantMillis(20)],
    }];
    let reading = tokio::spawn(async move { handle.announce_rates().await });

    let HostCommand::NodeIntrospection(NodeIntrospectionRequest::AnnounceRates { reply }) =
        command_rx.recv().await.unwrap()
    else {
        panic!("expected an announce-rate introspection request");
    };
    reply.send(expected.clone()).unwrap();

    assert_eq!(reading.await.unwrap(), expected);
}

#[tokio::test]
async fn destination_identity_hash_resolves_its_manifold_snapshot() {
    let (handle, mut command_rx) = handle();
    let identity = crate::identity::IdentityHash::new([0x42; 16]);
    let reading = tokio::spawn(async move { handle.destination_identity_hash(PEER).await });

    let HostCommand::NodeIntrospection(NodeIntrospectionRequest::DestinationIdentityHash {
        destination,
        reply,
    }) = command_rx.recv().await.unwrap()
    else {
        panic!("expected destination identity introspection request");
    };
    assert_eq!(destination, PEER);
    reply.send(Some(identity)).unwrap();

    assert_eq!(reading.await.unwrap(), Some(identity));
}

#[tokio::test]
async fn destination_identity_query_resolves_public_material() {
    let (handle, mut command_rx) = handle();
    let identity = crate::identity::IdentityHash::new([0x42; 16]);
    let public = crate::identity::PublicIdentityMaterial::from_bytes([0x31; 64]);
    let expected = crate::node_introspection::DestinationIdentitySnapshot {
        destination: PEER,
        identity,
        public,
    };
    let query = crate::node_introspection::DestinationIdentityQuery::Identity(identity);
    let reading = tokio::spawn(async move { handle.destination_identity(query).await });

    let HostCommand::NodeIntrospection(NodeIntrospectionRequest::DestinationIdentity {
        query: received,
        reply,
    }) = command_rx.recv().await.unwrap()
    else {
        panic!("expected destination identity material introspection request");
    };
    assert_eq!(received, query);
    reply.send(Some(expected)).unwrap();

    assert_eq!(reading.await.unwrap(), Some(expected));
}

#[tokio::test]
async fn routing_controls_resolve_their_typed_manifold_replies() {
    let (handle, mut command_rx) = handle();

    let dropping = tokio::spawn({
        let handle = handle.clone();
        async move { handle.drop_route(PEER).await }
    });
    let HostCommand::DropRoute { destination, reply } = command_rx.recv().await.unwrap() else {
        panic!("expected a route drop command");
    };
    assert_eq!(destination, PEER);
    reply.send(DropRouteOutcome::Dropped).unwrap();
    assert_eq!(dropping.await.unwrap(), Ok(DropRouteOutcome::Dropped));

    let transport = TransportId::new([0x42; 16]);
    let dropping_via = tokio::spawn({
        let handle = handle.clone();
        async move { handle.drop_routes_via(transport).await }
    });
    let HostCommand::DropRoutesVia {
        transport: requested,
        reply,
    } = command_rx.recv().await.unwrap()
    else {
        panic!("expected a transport route drop command");
    };
    assert_eq!(requested, transport);
    reply
        .send(DropRoutesViaOutcome { dropped_routes: 3 })
        .unwrap();
    assert_eq!(
        dropping_via.await.unwrap(),
        Ok(DropRoutesViaOutcome { dropped_routes: 3 })
    );

    let clearing = tokio::spawn(async move { handle.clear_announce_queues().await });
    let HostCommand::ClearAnnounceQueues { reply } = command_rx.recv().await.unwrap() else {
        panic!("expected an announce queue clear command");
    };
    reply
        .send(ClearAnnounceQueuesOutcome {
            dropped_announces: 5,
        })
        .unwrap();
    assert_eq!(
        clearing.await.unwrap(),
        Ok(ClearAnnounceQueuesOutcome {
            dropped_announces: 5,
        })
    );
}

#[tokio::test]
async fn routing_controls_report_a_stopped_manifold() {
    let (handle, command_rx) = handle();
    drop(command_rx);

    assert_eq!(
        handle.drop_route(PEER).await,
        Err(RoutingControlError::NodeStopped)
    );
    assert_eq!(
        handle.drop_routes_via(TransportId::new([0x42; 16])).await,
        Err(RoutingControlError::NodeStopped)
    );
    assert_eq!(
        handle.clear_announce_queues().await,
        Err(RoutingControlError::NodeStopped)
    );
}

#[tokio::test]
async fn identity_blackhole_capabilities_resolve_typed_manifold_replies() {
    let (handle, mut command_rx) = handle();
    let identity = crate::identity::IdentityHash::new([0x31; 16]);
    let source = crate::identity::IdentityHash::new([0x41; 16]);
    let expected = BlackholedIdentity {
        identity,
        source,
        expiry: BlackholeExpiry::Indefinite,
        reason: Some(String::from("operator")),
    };

    let reading = tokio::spawn({
        let handle = handle.clone();
        async move { handle.blackholed_identities().await }
    });
    let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::ReadAll { reply }) =
        command_rx.recv().await.unwrap()
    else {
        panic!("expected a blackhole table read command");
    };
    reply.send(vec![expected.clone()]).unwrap();
    assert_eq!(reading.await.unwrap(), Ok(vec![expected.clone()]));

    let checking = tokio::spawn({
        let handle = handle.clone();
        async move { handle.is_blackholed(identity).await }
    });
    let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::IsBlackholed {
        identity: requested,
        reply,
    }) = command_rx.recv().await.unwrap()
    else {
        panic!("expected an identity blackhole query command");
    };
    assert_eq!(requested, identity);
    reply.send(true).unwrap();
    assert_eq!(checking.await.unwrap(), Ok(true));

    let blackholing = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .blackhole_identity(BlackholedIdentity {
                    identity,
                    source,
                    expiry: BlackholeExpiry::Indefinite,
                    reason: Some("operator"),
                })
                .await
        }
    });
    let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::Blackhole { entry, reply }) =
        command_rx.recv().await.unwrap()
    else {
        panic!("expected an identity blackhole command");
    };
    assert_eq!(entry, expected);
    reply.send(Ok(BlackholeIdentityOutcome::Added)).unwrap();
    assert_eq!(
        blackholing.await.unwrap(),
        Ok(BlackholeIdentityOutcome::Added)
    );

    let unblackholing = tokio::spawn(async move { handle.unblackhole_identity(identity).await });
    let HostCommand::IdentityBlackhole(IdentityBlackholeHostCommand::Unblackhole {
        identity: requested,
        reply,
    }) = command_rx.recv().await.unwrap()
    else {
        panic!("expected an identity unblackhole command");
    };
    assert_eq!(requested, identity);
    reply.send(Ok(UnblackholeIdentityOutcome::Removed)).unwrap();
    assert_eq!(
        unblackholing.await.unwrap(),
        Ok(UnblackholeIdentityOutcome::Removed)
    );
}

#[tokio::test]
async fn identity_blackhole_capabilities_report_a_stopped_manifold() {
    let (handle, command_rx) = handle();
    drop(command_rx);
    let identity = crate::identity::IdentityHash::new([0x31; 16]);
    let source = crate::identity::IdentityHash::new([0x41; 16]);

    assert_eq!(
        handle.blackholed_identities().await,
        Err(IdentityBlackholeSourceError::NodeStopped)
    );
    assert_eq!(
        handle.is_blackholed(identity).await,
        Err(IdentityBlackholeSourceError::NodeStopped)
    );
    assert_eq!(
        handle
            .blackhole_identity(BlackholedIdentity {
                identity,
                source,
                expiry: BlackholeExpiry::Indefinite,
                reason: None,
            })
            .await,
        Err(IdentityBlackholeControlError::NodeStopped)
    );
    assert_eq!(
        handle.unblackhole_identity(identity).await,
        Err(IdentityBlackholeControlError::NodeStopped)
    );
}
