use super::*;

struct TrackedSeam {
    inner: MockSeam,
    dispositions: Arc<Mutex<Vec<OutboundDisposition>>>,
}

impl InterfaceSeam for TrackedSeam {
    fn fill_random(&mut self, bytes: &mut [u8]) {
        self.inner.fill_random(bytes);
    }
    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        self.inner.inbound_sink().await
    }
    async fn commit_inbound(&mut self) {
        self.inner.commit_inbound().await;
    }
    async fn next_outbound(&mut self) -> &[u8] {
        self.inner.next_outbound().await
    }
    fn complete_outbound(&mut self, disposition: OutboundDisposition) {
        self.dispositions.lock().unwrap().push(disposition);
    }
}

#[tokio::test]
async fn settled_close_interrupts_a_blocked_send_and_drops_all_owners_before_reporting() {
    let address = BleAddress::new([0x11; 6]);
    let (link, mut remote) = link_pair(address, BleAddress::new([0x22; 6]));
    let parts = link.into_parts();
    let (started, sending) = oneshot::channel();
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();
    let member = BluetoothPeer::new(
        BleIdentity::new([1; 16]),
        parts.source,
        BlockingSink {
            started: Some(started),
        },
    )
    .with_control(parts.control)
    .report_close_to(address, closed_tx);
    drop(parts.sink);
    let status = member.status();
    let (mut outbound, incoming) = tokio_grant_lane(TEST_FRAME_CAP, 2);
    let dispositions = Arc::new(Mutex::new(Vec::new()));
    let seam = TrackedSeam {
        inner: MockSeam {
            outbound: incoming,
            ..idle_seam()
        },
        dispositions: dispositions.clone(),
    };
    let task = tokio::spawn(member.run(seam));
    outbound.try_grant().unwrap().fill(b"blocked");
    outbound.commit();
    tokio::time::timeout(Duration::from_secs(1), sending)
        .await
        .unwrap()
        .unwrap();
    remote
        .control_send(&Control::Close {
            reason: CloseReason::DuplicateLink,
        })
        .await
        .unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(1), closed_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(closed.matches(address, &status));
    assert!(remote.control.control_tx.is_closed());
    assert!(remote.data_tx.is_closed());
    assert_eq!(
        *dispositions.lock().unwrap(),
        [OutboundDisposition::Dropped(
            OutboundDropReason::TransportFailure
        )]
    );
    task.abort();
}

#[tokio::test]
async fn settled_control_eof_retires_a_member_even_while_data_is_still_open() {
    let address = BleAddress::new([0x11; 6]);
    let (link, remote) = link_pair(address, BleAddress::new([0x22; 6]));
    let parts = link.into_parts();
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();
    let member = BluetoothPeer::new(BleIdentity::new([1; 16]), parts.source, parts.sink)
        .with_control(parts.control)
        .report_close_to(address, closed_tx);
    let status = member.status();
    let task = tokio::spawn(member.run(idle_seam()));
    drop(remote.control);
    let closed = tokio::time::timeout(Duration::from_secs(1), closed_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(closed.matches(address, &status));
    assert!(remote.data_tx.is_closed());
    task.abort();
}

struct GreetingFlood {
    received: Arc<std::sync::atomic::AtomicUsize>,
}

impl BleControl for GreetingFlood {
    type Error = Closed;
    async fn send(&mut self, _message: &Control) -> Result<(), Closed> {
        panic!("settlement must not introduce control writes")
    }
    async fn recv(&mut self) -> Result<Control, Closed> {
        self.received.fetch_add(1, Ordering::Relaxed);
        Ok(Control::Hello {
            identity: BleIdentity::new([2; 16]),
            endpoint: linux(),
            capabilities: caps(0x80),
            peer_rssi: None,
            discovery_groups: contract::PeerDiscoveryGroups::Explicit(
                DiscoveryGroupSet::reticulum().hashes(),
            ),
        })
    }
}

#[tokio::test]
async fn settled_greeting_flood_does_not_evict_or_starve_the_keeper() {
    let (link, remote) = link_pair(BleAddress::new([1; 6]), BleAddress::new([2; 6]));
    let parts = link.into_parts();
    let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (closed_tx, mut closed_rx) = mpsc::unbounded_channel();
    let peer = BluetoothPeer::new(BleIdentity::new([2; 16]), parts.source, parts.sink)
        .with_control(Some(GreetingFlood {
            received: received.clone(),
        }))
        .report_close_to(BleAddress::new([2; 6]), closed_tx);
    let (captured, mut capture) = mpsc::unbounded_channel();
    let task = tokio::spawn(peer.run(MockSeam {
        inbound: captured,
        ..idle_seam()
    }));
    for _ in 0..16 {
        remote.data_tx.send(b"healthy".to_vec()).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), capture.recv())
                .await
                .unwrap()
                .unwrap(),
            b"healthy"
        );
    }
    assert!(received.load(Ordering::Relaxed) > 0);
    assert!(closed_rx.try_recv().is_err());
    assert!(!task.is_finished());
    task.abort();
}

struct ReceiveLifetime {
    started: Arc<std::sync::atomic::AtomicUsize>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}

struct ControlledSink {
    started: Arc<std::sync::atomic::AtomicUsize>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
    finish: oneshot::Receiver<()>,
}

impl BleSink for ControlledSink {
    type Error = Closed;
    async fn send_frame(&mut self, _frame: &[u8]) -> Result<(), Closed> {
        self.started.fetch_add(1, Ordering::Relaxed);
        let _pending = CountDrop(self.dropped.clone());
        (&mut self.finish).await.map_err(|_| Closed)
    }
}

#[tokio::test]
async fn settled_greetings_do_not_cancel_a_pending_data_send() {
    let (link, mut remote) = link_pair(BleAddress::new([1; 6]), BleAddress::new([2; 6]));
    let parts = link.into_parts();
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (finish, finishing) = oneshot::channel();
    let peer = BluetoothPeer::new(
        BleIdentity::new([2; 16]),
        parts.source,
        ControlledSink {
            started: started.clone(),
            dropped: dropped.clone(),
            finish: finishing,
        },
    )
    .with_control(parts.control);
    let (mut outgoing, incoming) = tokio_grant_lane(TEST_FRAME_CAP, 2);
    let (captured, mut capture) = mpsc::unbounded_channel();
    let dispositions = Arc::new(Mutex::new(Vec::new()));
    let task = tokio::spawn(peer.run(TrackedSeam {
        inner: MockSeam {
            inbound: captured,
            outbound: incoming,
            ..idle_seam()
        },
        dispositions: dispositions.clone(),
    }));
    outgoing.try_grant().unwrap().fill(b"pending");
    outgoing.commit();
    tokio::time::timeout(Duration::from_secs(1), async {
        while started.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
        let mut greetings = GreetingFlood {
            received: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        for _ in 0..16 {
            remote
                .control_send(&greetings.recv().await.unwrap())
                .await
                .unwrap();
        }
        remote.data_tx.send(b"duplex".to_vec()).await.unwrap();
        assert_eq!(capture.recv().await.unwrap(), b"duplex");
    })
    .await
    .unwrap();
    assert_eq!(started.load(Ordering::Relaxed), 1);
    assert_eq!(dropped.load(Ordering::Relaxed), 0);
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while dispositions.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*dispositions.lock().unwrap(), [OutboundDisposition::Sent]);
    assert_eq!(started.load(Ordering::Relaxed), 1);
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    task.abort();
}

struct CountDrop(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for CountDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl BleControl for ReceiveLifetime {
    type Error = Closed;
    async fn send(&mut self, _message: &Control) -> Result<(), Closed> {
        panic!("receive-only")
    }
    async fn recv(&mut self) -> Result<Control, Closed> {
        self.started.fetch_add(1, Ordering::Relaxed);
        let _pending = CountDrop(self.dropped.clone());
        core::future::pending().await
    }
}

#[tokio::test]
async fn data_activity_preserves_the_in_flight_control_receive() {
    let (link, remote) = link_pair(BleAddress::new([1; 6]), BleAddress::new([2; 6]));
    let parts = link.into_parts();
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peer = BluetoothPeer::new(BleIdentity::new([2; 16]), parts.source, parts.sink)
        .with_control(Some(ReceiveLifetime {
            started: started.clone(),
            dropped: dropped.clone(),
        }));
    let (captured, mut capture) = mpsc::unbounded_channel();
    let task = tokio::spawn(peer.run(MockSeam {
        inbound: captured,
        ..idle_seam()
    }));
    for _ in 0..16 {
        remote.data_tx.send(b"data".to_vec()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), capture.recv())
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(started.load(Ordering::Relaxed), 1);
    assert_eq!(dropped.load(Ordering::Relaxed), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
}
