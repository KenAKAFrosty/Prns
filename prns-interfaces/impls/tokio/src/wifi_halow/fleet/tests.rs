use super::*;
use crate::wifi_halow::ReceivedDatagram;
use prns_core::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, RatchetPolicy};
use prns_core::identity::Zeroizing;
use prns_core::interfaces::{AnnounceBandwidthCap, BitrateBps, MacAddress};
use prns_core::routing::links::resources::ResourceStrategy;
use prns_core::routing::{LinkRequestPolicy, ProofStrategy};
use prns_runtime::runtime::{
    ManuallyAttached, NoPersistence, NoRemoteControlHostControls, PreConfiguredDestination,
    PrnsNode, PrnsNodeRecipe, ServeMyRequestEndpoints,
};
use prns_runtime::storage::GrowableHeap;
use std::io;
use tokio::sync::Mutex;

struct TestRadio {
    incoming: Mutex<mpsc::Receiver<(PeerMac, Vec<u8>)>>,
    outgoing: mpsc::Sender<(Destination, Vec<u8>)>,
}
impl HaLowDatagrams for TestRadio {
    async fn send(&self, destination: Destination, bytes: &[u8]) -> io::Result<()> {
        self.outgoing
            .send((destination, bytes.to_vec()))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
    async fn receive(&self, buffer: &mut [u8]) -> io::Result<ReceivedDatagram> {
        let mut incoming = self.incoming.lock().await;
        let (source, bytes) = incoming.recv().await.ok_or(io::ErrorKind::UnexpectedEof)?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(ReceivedDatagram {
            source,
            length: bytes.len(),
        })
    }
}

fn policy() -> EffectiveInterfacePolicy {
    let mut policy =
        prns_core::interfaces::wifi_auto::policy_for_bitrate(BitrateBps::guess(8_000_000));
    policy.announce_bandwidth_cap = AnnounceBandwidthCap::Unlimited;
    policy
}
fn mac(last: u8) -> PeerMac {
    PeerMac::new(MacAddress::new([2, 0, 0, 0, 0, last])).unwrap()
}
async fn turns() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn running_node_broadcasts_without_peers_admits_first_frame_and_replies_directly() {
    let destination = PreConfiguredDestination::Single {
        app_name: "halow",
        aspects: &["test"],
        identity: Zeroizing::new([17; 64]),
        announce_app_data: b"lab",
        proof: ProofStrategy::ProveNone,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    };
    let address = destination.destination_hash().unwrap();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination],
        app_state: NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: prns_runtime::request_endpoints![],
        remote_control: prns_runtime::remote_control::RemoteControlService::Unavailable.into(),
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_, _: &NoRemoteControlHostControls| {},
    });
    let handle = node.handle();
    let (inject, incoming) = mpsc::channel(16);
    let (outgoing, mut emitted) = mpsc::channel(16);
    let scope = InstanceTag::new(b"radio").unwrap();
    let first_id = scope.peer_id(mac(1));
    let second_id = scope.peer_id(mac(2));
    let supervisor = handle.supervise(HaLow::new(
        TestRadio {
            incoming: Mutex::new(incoming),
            outgoing,
        },
        scope,
        policy(),
        policy(),
        HaLowLimits {
            peers: NonZeroU8::MIN,
            idle_seconds: std::num::NonZeroU32::new(10).unwrap(),
        },
    ));
    let exercise = async {
        turns().await;
        assert_eq!(
            handle.interfaces().len(),
            2,
            "supervisor and zero-peer broadcast channel"
        );
        inject
            .send((mac(1), b"unrelated protocol".to_vec()))
            .await
            .unwrap();
        turns().await;
        assert_eq!(
            handle.interfaces().len(),
            2,
            "bad envelope must not admit a peer"
        );
        handle
            .announce_now(AnnounceNow {
                destination: address,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })
            .await
            .unwrap();
        let (target, datagram) = emitted.recv().await.unwrap();
        assert_eq!(target, Destination::Broadcast);
        let frame_len = contract::decode(&datagram).unwrap().len();
        inject.send((mac(1), datagram.clone())).await.unwrap();
        turns().await;
        let first = handle
            .interfaces()
            .into_iter()
            .find(|s| s.id == first_id)
            .unwrap();
        assert_eq!(
            first.rx_bytes, frame_len as u64,
            "first frame survives attachment"
        );
        assert!(matches!(
            first.radio,
            prns_core::interfaces::RadioIndication::HaLow(_)
        ));
        inject.send((mac(2), datagram.clone())).await.unwrap();
        turns().await;
        assert!(
            !handle.interfaces().iter().any(|s| s.id == second_id),
            "peer cap rejects new sources"
        );

        tokio::time::advance(Duration::from_secs(6)).await;
        handle
            .announce_now(AnnounceNow {
                destination: address,
                target: AnnounceTarget::Interface(first_id),
                app_data: AnnounceAppData::Registered,
            })
            .await
            .unwrap();
        assert_eq!(emitted.recv().await.unwrap().0, Destination::Peer(mac(1)));
        handle
            .announce_now(AnnounceNow {
                destination: address,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })
            .await
            .unwrap();
        assert_eq!(emitted.recv().await.unwrap().0, Destination::Broadcast);
        turns().await;
        assert!(
            emitted.try_recv().is_err(),
            "no per-peer duplicate of a broadcast announce"
        );

        tokio::time::advance(Duration::from_secs(6)).await;
        turns().await;
        assert!(
            handle.interfaces().iter().any(|s| s.id == first_id),
            "successful TX refreshes peer expiry"
        );
        tokio::time::advance(Duration::from_secs(10)).await;
        turns().await;
        assert!(!handle.interfaces().iter().any(|s| s.id == first_id));
        inject.send((mac(2), datagram)).await.unwrap();
        turns().await;
        assert!(handle.interfaces().iter().any(|s| s.id == second_id));
        supervisor.teardown();
        turns().await;
        assert!(
            handle.interfaces().is_empty(),
            "teardown cascades to both kinds of child"
        );
    };
    tokio::select! {
        result = node.run() => panic!("node exited early: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(30), exercise) => result.unwrap(),
    }
}

#[tokio::test(start_paused = true)]
async fn send_backpressure_has_a_deadline_and_never_changes_the_destination() {
    let (_inject, incoming) = mpsc::channel(1);
    let (outgoing, mut emitted) = mpsc::channel(1);
    let scope = InstanceTag::new(b"deadline").unwrap();
    let channel = Channel::<_, Peer>::new(
        Arc::new(TestRadio {
            incoming: Mutex::new(incoming),
            outgoing,
        }),
        scope.peer_channel_tag(mac(1)).to_vec(),
        bounded_policy(policy()),
        Destination::Peer(mac(1)),
    );
    assert_eq!(channel.send(b"first").await, OutboundDisposition::Sent);
    assert_eq!(
        channel.send(b"backpressured").await,
        OutboundDisposition::Dropped(OutboundDropReason::TimedOut)
    );
    let (target, bytes) = emitted.recv().await.unwrap();
    assert_eq!(target, Destination::Peer(mac(1)));
    assert_eq!(contract::decode(&bytes), Ok(b"first".as_slice()));
    assert!(emitted.try_recv().is_err());
    drop(emitted);
    assert_eq!(
        channel.send(b"disconnected").await,
        OutboundDisposition::Dropped(OutboundDropReason::TransportFailure)
    );
    assert_eq!(
        channel.send(&[0; contract::FRAME_MTU + 1]).await,
        OutboundDisposition::Dropped(OutboundDropReason::Rejected)
    );
}
