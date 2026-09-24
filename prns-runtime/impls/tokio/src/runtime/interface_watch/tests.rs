use super::*;
use crate::engine::{
    DeliveryEvidence, DeliveryProof, PacketReceiptDelivered, PrnsCommand, Settlement,
};
use crate::manifold::driver::HostCommand;
use crate::remote_control::FixedRemoteControlControllerGrantTable;
use crate::routing::dedup::PacketHash;
use crate::routing::links::channel::byte_stream::parse;
use crate::units::RttMillis;
use tokio::sync::mpsc;

struct PanickingStatus;

impl crate::interfaces::ReportsStatus for PanickingStatus {
    fn status_view(&self) -> Option<crate::interfaces::StatusView> {
        Some(Arc::new(|| std::panic::panic_any("status callback")))
    }
}

impl crate::manifold::interface_seam::Interface for PanickingStatus {
    const HW_MTU: usize = crate::wire::BROADCAST_MTU;
    const KIND: crate::interfaces::InterfaceKind = crate::interfaces::InterfaceKind::Loopback;

    fn channel_tag(&self) -> &[u8] {
        b"panicking-watch-status"
    }

    fn descriptor(&self) -> crate::interfaces::InterfaceDescriptor {
        use crate::interfaces::*;
        InterfaceDescriptor {
            id: InterfaceId::from_channel_tag(Self::KIND, self.channel_tag()),
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Disabled,
                egress: EgressCapability::Disabled,
            },
            mode: InterfaceMode::Full,
            gravity: InterfaceGravity::ZERO,
            bitrate: BitrateBps::guess(1_000_000),
            hardware_mtu: Some(Self::HW_MTU),
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::Unlimited,
            airtime_duty_cycle: None,
            common: InterfaceCommonPolicy::RNS_DEFAULT,
        }
    }

    async fn run<S: crate::manifold::interface_seam::InterfaceSeam>(self, _: S) {
        std::future::pending::<()>().await;
    }
}

#[tokio::test]
async fn panicking_status_callback_closes_its_link_and_releases_the_watch_lease() {
    let (commands, mut receiver) = mpsc::unbounded_channel();
    let node = PrnsNodeHandle::over(commands);
    node.add_interface(PanickingStatus);
    let watches = InterfaceWatchRegistry::default();
    let link_id = LinkId::new([7; 16]);
    watches
        .reserve(
            node,
            link_id,
            StreamId::new(3).unwrap(),
            IdentityHash::new([8; 16]),
        )
        .unwrap()
        .start();
    watches.next_completion().await;
    assert!(watches.watches.lock().unwrap().is_empty());
    let mut closed = Vec::new();
    while let Ok(command) = receiver.try_recv() {
        if let HostCommand::Engine(crate::engine::IssuedCommand {
            command: PrnsCommand::CloseLink(close),
            ..
        }) = command
        {
            closed.push(close.link_id);
        }
    }
    assert_eq!(closed, [link_id]);
}

#[tokio::test]
async fn revoked_pending_watch_cannot_start_and_old_reservation_cannot_cancel_its_replacement() {
    let (commands, mut receiver) = mpsc::unbounded_channel();
    let node = PrnsNodeHandle::over(commands);
    let watches = InterfaceWatchRegistry::default();
    let link_id = LinkId::new([7; 16]);
    let stream_id = StreamId::new(3).unwrap();
    let controller = IdentityHash::new([8; 16]);
    let original = watches
        .reserve(node.clone(), link_id, stream_id, controller)
        .unwrap();
    watches.reconcile_grants(&FixedRemoteControlControllerGrantTable::<8>::default());
    watches.next_completion().await;
    let replacement = watches
        .reserve(node, link_id, stream_id, controller)
        .unwrap();
    assert!(matches!(
        watches.admission(&original),
        WatchAdmission::Withdrawn
    ));
    watches.cancel(&original);
    assert!(matches!(
        watches.admission(&replacement),
        WatchAdmission::Admitted
    ));
    original.start();
    tokio::task::yield_now().await;
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test(start_paused = true)]
async fn blocked_writer_finishes_within_the_cleanup_deadline() {
    let (commands, mut receiver) = mpsc::unbounded_channel();
    let node = PrnsNodeHandle::over(commands);
    let watches = InterfaceWatchRegistry::default();
    let link_id = LinkId::new([7; 16]);
    let stream_id = StreamId::new(3).unwrap();
    watches
        .reserve(node, link_id, stream_id, IdentityHash::new([8; 16]))
        .unwrap()
        .start();
    let exercise = async {
        let initial = receiver.recv().await.unwrap(); // Keep the settlement pending.
        watches.cancel_link(link_id);
        tokio::task::yield_now().await;
        tokio::time::advance(WATCH_WRITE_DEADLINE).await;
        assert!(matches!(
            receiver.recv().await,
            Some(HostCommand::Engine(crate::engine::IssuedCommand {
                command: PrnsCommand::CloseLink(_),
                ..
            }))
        ));
        drop(initial);
    };
    tokio::join!(watches.next_completion(), exercise);
    assert!(watches.watches.lock().unwrap().is_empty());
}

#[tokio::test]
async fn watch_registry_bounds_subscriptions_and_releases_revoked_slots() {
    let (commands, _receiver) = mpsc::unbounded_channel();
    let node = PrnsNodeHandle::over(commands);
    let watches = InterfaceWatchRegistry::default();
    let link_id = LinkId::new([7; 16]);
    let controller = IdentityHash::new([8; 16]);
    let mut reservations = Vec::new();
    for index in 0..MAX_INTERFACE_WATCHES {
        let stream_id = StreamId::new(index as u16).unwrap();
        reservations.push(
            watches
                .reserve(node.clone(), link_id, stream_id, controller)
                .unwrap(),
        );
    }
    assert!(matches!(
        watches.reserve(node.clone(), link_id, StreamId::new(0).unwrap(), controller),
        Err(WatchReserveFailure::Duplicate),
    ));
    assert!(matches!(
        watches.reserve(node.clone(), link_id, StreamId::new(9).unwrap(), controller),
        Err(WatchReserveFailure::Full),
    ));
    let grants = FixedRemoteControlControllerGrantTable::<8>::default();
    watches.reconcile_grants(&grants);
    // Retiring workers still occupy capacity until they actually stop.
    assert!(matches!(
        watches.reserve(node.clone(), link_id, StreamId::new(9).unwrap(), controller),
        Err(WatchReserveFailure::Full)
    ));
    for _ in 0..MAX_INTERFACE_WATCHES {
        watches.next_completion().await;
    }
    let replacement = watches
        .reserve(node.clone(), link_id, StreamId::new(9).unwrap(), controller)
        .unwrap();
    watches.cancel_link(link_id);
    let another_link = LinkId::new([9; 16]);
    assert!(watches
        .reserve(node, another_link, StreamId::new(9).unwrap(), controller)
        .is_ok());
    drop(replacement);
    drop(reservations);
}

#[tokio::test]
async fn closing_a_link_ends_its_watch_stream() {
    let (commands, mut receiver) = mpsc::unbounded_channel();
    let node = PrnsNodeHandle::over(commands);
    let watches = InterfaceWatchRegistry::default();
    let link_id = LinkId::new([0x41; 16]);
    let stream_id = StreamId::new(0x123).unwrap();
    watches
        .reserve(node, link_id, stream_id, IdentityHash::new([0x42; 16]))
        .unwrap()
        .start();
    let exercise = async {
        let Some(HostCommand::AwaitedEngine { issued, completion }) =
            tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
        else {
            panic!("initial stream event");
        };
        let PrnsCommand::SendToChannel(initial) = issued.command else {
            panic!("initial stream frame");
        };
        assert!(!parse(initial.body.as_slice()).unwrap().header.eof);
        completion
            .send(Settlement::SendToChannel(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(0),
                evidence: DeliveryEvidence::Proof(DeliveryProof::Implicit(PacketHash::new(
                    [0; 32],
                ))),
            })))
            .unwrap();
        watches.cancel_link(link_id);
        let Some(HostCommand::AwaitedEngine { issued, completion }) =
            tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
        else {
            panic!("watch stream closes with EOF");
        };
        let PrnsCommand::SendToChannel(closing) = issued.command else {
            panic!("stream close frame");
        };
        assert!(parse(closing.body.as_slice()).unwrap().header.eof);
        completion
            .send(Settlement::SendToChannel(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(0),
                evidence: DeliveryEvidence::Proof(DeliveryProof::Implicit(PacketHash::new(
                    [0; 32],
                ))),
            })))
            .unwrap();
    };
    tokio::join!(watches.next_completion(), exercise);
    assert!(watches.watches.lock().unwrap().is_empty());
}
