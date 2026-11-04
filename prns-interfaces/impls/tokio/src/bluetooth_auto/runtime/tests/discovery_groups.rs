use super::*;

/// Exercise a valid selection order where incoming traffic wins over a pending group event.
struct IncomingFirst;

impl super::super::super::BleEventSelector for IncomingFirst {
    async fn select<T>(
        &mut self,
        sources: super::super::super::BleEventSources<
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
        >,
    ) -> T {
        tokio::select! {
            biased;
            event = sources.disabled => event,
            event = sources.backend => event,
            event = sources.handshake => event,
            event = sources.closed => event,
            event = sources.groups => event,
        }
    }
}

struct IncomingBackend {
    incoming: mpsc::UnboundedReceiver<LoopbackLink>,
    listening: Option<oneshot::Sender<()>>,
}

impl BleBackend<7> for IncomingBackend {
    type Error = Closed;
    type Link = LoopbackLink;

    async fn set_advertising(&mut self, _mode: AdvertisingMode) -> Result<(), Closed> {
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<LoopbackLink> {
        if let Some(listening) = self.listening.take() {
            let _ = listening.send(());
        }
        match self.incoming.recv().await {
            Some(link) => BleEvent::Inbound(link),
            None => std::future::pending().await,
        }
    }

    async fn dial(&mut self, _address: BleAddress) -> contract::DialOutcome {
        contract::DialOutcome::UnknownPeer
    }
}

fn trial_groups() -> DiscoveryGroupSet {
    DiscoveryGroupSet::singleton(prns_core::interfaces::DiscoveryGroupId::parse("trial-a").unwrap())
        .unwrap()
}

fn peer(groups: DiscoveryGroupSet) -> LocalPeer {
    LocalPeer {
        identity: BleIdentity::new([2; 16]),
        endpoint: Endpoint::CoreBluetooth(AppleHost::Ios),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: contract::BLE_HW_MTU as u16,
        },
        discovery_groups: groups.hashes(),
    }
}

async fn rejects_old_group_before_groups_event(resume: bool) {
    let (incoming, incoming_rx) = mpsc::unbounded_channel();
    let (listening, listening_rx) = oneshot::channel();
    let old_peer = peer(DiscoveryGroupSet::reticulum());
    let new_groups = trial_groups();
    let bluetooth = BluetoothAuto::<_, 7>::new(
        IncomingBackend {
            incoming: incoming_rx,
            listening: Some(listening),
        },
        BleIdentity::new([1; 16]),
        old_peer.endpoint,
        old_peer.capabilities,
    )
    .with_event_selector(IncomingFirst);
    let status = bluetooth.status();
    let (fleet, _detached_fleet) = Fleet::detached(status.id());
    let (old_link, mut old_remote) = link_pair(BleAddress::new([1; 6]), BleAddress::new([2; 6]));
    let (_, greeting) = Handshake::begin(HandshakeRole::Dialer, old_peer, None);
    old_remote.control_send(&greeting.unwrap()).await.unwrap();
    if !resume {
        // The constructor captured the default group; preparation can update status before run.
        assert_eq!(
            status.replace_discovery_groups(new_groups).await,
            DiscoveryGroupApplyOutcome::Applied
        );
    }
    let observe = async {
        if resume {
            listening_rx.await.unwrap();
            status.disable();
            let mut disabled = status.shared.enabled.subscribe();
            disabled.wait_for(|current| current.disabled).await.unwrap();
            assert_eq!(
                status.replace_discovery_groups(new_groups).await,
                DiscoveryGroupApplyOutcome::Applied
            );
        }
        // Queue before enable so a backend event is ready as soon as the supervisor resumes.
        incoming.send(old_link).ok().unwrap();
        status.enable();
        assert_eq!(
            old_remote.control_recv().await.unwrap(),
            Control::Close {
                reason: CloseReason::Incompatible,
            }
        );
        assert!(status.shared.members.lock().unwrap().is_empty());

        // The replacement group must remain usable, not merely reject every connection.
        let (new_link, mut new_remote) =
            link_pair(BleAddress::new([1; 6]), BleAddress::new([3; 6]));
        incoming.send(new_link).ok().unwrap();
        let established = drive_handshake(
            &mut new_remote,
            HandshakeRole::Dialer,
            peer(new_groups),
            None,
        )
        .await
        .unwrap();
        assert_eq!(established.identity, BleIdentity::new([1; 16]));
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(1), observe) => result.unwrap(),
        () = bluetooth.run(fleet) => unreachable!("the supervisor runs until cancelled"),
    }
}

#[tokio::test]
async fn startup_uses_groups_replaced_after_construction() {
    rejects_old_group_before_groups_event(false).await;
}

#[tokio::test]
async fn resume_rejects_old_group_inbound_before_selecting_group_change() {
    rejects_old_group_before_groups_event(true).await;
}

#[tokio::test]
async fn resume_acknowledges_a_replacement_requested_after_enable() {
    let (incoming, incoming_rx) = mpsc::unbounded_channel();
    let (listening, listening_rx) = oneshot::channel();
    let old_peer = peer(DiscoveryGroupSet::reticulum());
    let bluetooth = BluetoothAuto::<_, 7>::new(
        IncomingBackend {
            incoming: incoming_rx,
            listening: Some(listening),
        },
        BleIdentity::new([1; 16]),
        old_peer.endpoint,
        old_peer.capabilities,
    )
    .with_event_selector(IncomingFirst);
    let status = bluetooth.status();
    let (fleet, _detached_fleet) = Fleet::detached(status.id());
    let observe = async {
        listening_rx.await.unwrap();
        status.disable();
        let mut disabled = status.shared.enabled.subscribe();
        disabled.wait_for(|current| current.disabled).await.unwrap();
        status.enable();
        let replacement = status.replace_discovery_groups(trial_groups());
        tokio::pin!(replacement);
        assert!(std::future::poll_fn(|cx| {
            std::task::Poll::Ready(replacement.as_mut().poll(cx))
        })
        .await
        .is_pending());

        // Resume consumes the desired group directly, so a later Groups event cannot ack it.
        assert_eq!(replacement.await, DiscoveryGroupApplyOutcome::Applied);
        let (link, mut remote) = link_pair(BleAddress::new([1; 6]), BleAddress::new([2; 6]));
        let (_, greeting) = Handshake::begin(HandshakeRole::Dialer, old_peer, None);
        remote.control_send(&greeting.unwrap()).await.unwrap();
        incoming.send(link).ok().unwrap();
        assert_eq!(
            remote.control_recv().await.unwrap(),
            Control::Close {
                reason: CloseReason::Incompatible,
            }
        );
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(1), observe) => result.unwrap(),
        () = bluetooth.run(fleet) => unreachable!("the supervisor runs until cancelled"),
    }
}
