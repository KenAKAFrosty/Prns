use super::*;
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

/// The peer can observe a submitted write before the platform reports its result.
struct CompletingLink {
    inner: LoopbackLink,
    completion: oneshot::Receiver<Result<(), Closed>>,
    submissions: Arc<std::sync::atomic::AtomicUsize>,
}

impl BleLink for CompletingLink {
    type Error = Closed;
    type Source = LoopbackSource;
    type Sink = LoopbackSink;
    type Control = LoopbackControl;

    fn peer_protocol(&self) -> PeerProtocol {
        self.inner.peer_protocol()
    }

    fn address(&self) -> BleAddress {
        self.inner.address()
    }

    async fn control_send(&mut self, message: &Control) -> Result<(), Closed> {
        self.submissions.fetch_add(1, Ordering::Relaxed);
        self.inner.control_send(message).await?;
        (&mut self.completion).await.map_err(|_| Closed)?
    }

    async fn control_recv(&mut self) -> Result<Control, Closed> {
        self.inner.control_recv().await
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), Closed> {
        self.inner.upgrade(plan).await
    }

    fn into_parts(self) -> BleLinkParts<Self::Source, Self::Sink, Self::Control> {
        self.inner.into_parts()
    }
}

fn peer(id: u8) -> LocalPeer {
    LocalPeer {
        identity: BleIdentity::new([id; 16]),
        endpoint: linux(),
        capabilities: caps(0x0080 + u16::from(id)),
        discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
    }
}

async fn exercise_completion(role: HandshakeRole, success: bool) {
    let (inner, mut remote) = link_pair(BleAddress::new([1; 6]), BleAddress::new([2; 6]));
    let (complete, completion) = oneshot::channel();
    let submissions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let local = peer(1);
    let other = peer(2);
    let (origin, remote_role) = match role {
        HandshakeRole::Dialer => (Origin::Dialed, HandshakeRole::Listener),
        HandshakeRole::Listener => (Origin::Accepted, HandshakeRole::Dialer),
    };
    let (mut remote_handshake, opening) = Handshake::begin(remote_role, other, None);
    if let Some(opening) = opening {
        remote.control_send(&opening).await.unwrap();
    }
    let link = CompletingLink {
        inner,
        completion,
        submissions: submissions.clone(),
    };
    let task = run_handshake_task(link, role, local, BleAddress::new([2; 6]), origin, None);
    tokio::pin!(task);
    assert!(poll_once(task.as_mut()).await.is_pending());
    let submitted = remote.control_recv().await.unwrap();
    let reaction = remote_handshake.absorb(other, submitted);
    if let Some(reply) = reaction.reply {
        remote.control_send(&reply).await.unwrap();
    }

    // Neither a peer reply nor repeated polling can replace local completion.
    assert!(poll_once(task.as_mut()).await.is_pending());
    assert!(poll_once(task.as_mut()).await.is_pending());
    assert_eq!(submissions.load(Ordering::Relaxed), 1);
    complete
        .send(if success { Ok(()) } else { Err(Closed) })
        .unwrap();
    let finished = task.await;
    if success {
        let (established, _link) = finished.outcome.unwrap();
        assert_eq!(established.identity, other.identity);
    } else {
        assert!(matches!(
            (role, finished.outcome),
            (HandshakeRole::Dialer, Err(HandshakeFailure::InitialSend))
                | (HandshakeRole::Listener, Err(HandshakeFailure::ReplySend))
        ));
    }
    assert!(remote.control.control_tx.is_closed());
    assert!(remote.data_tx.is_closed());
}

#[tokio::test]
async fn dialer_waits_for_native_completion_even_after_peer_reply() {
    exercise_completion(HandshakeRole::Dialer, true).await;
}

#[tokio::test]
async fn listener_waits_for_its_native_reply_completion_before_settling() {
    exercise_completion(HandshakeRole::Listener, true).await;
}

#[tokio::test]
async fn native_failure_rejects_both_roles_and_drops_their_owners() {
    exercise_completion(HandshakeRole::Dialer, false).await;
    exercise_completion(HandshakeRole::Listener, false).await;
}

#[tokio::test(start_paused = true)]
async fn missing_native_completion_keeps_original_deadline_and_drops_owners() {
    let (inner, mut remote) = link_pair(BleAddress::new([1; 6]), BleAddress::new([2; 6]));
    let (_complete, completion) = oneshot::channel();
    let submissions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let link = CompletingLink {
        inner,
        completion,
        submissions: submissions.clone(),
    };
    let task = run_handshake_task(
        link,
        HandshakeRole::Dialer,
        peer(1),
        BleAddress::new([2; 6]),
        Origin::Dialed,
        None,
    );
    tokio::pin!(task);
    assert!(poll_once(task.as_mut()).await.is_pending());
    let submitted = remote.control_recv().await.unwrap();
    let (mut listener, _) = Handshake::begin(HandshakeRole::Listener, peer(2), None);
    let reply = listener.absorb(peer(2), submitted).reply.unwrap();
    remote.control_send(&reply).await.unwrap();
    tokio::time::advance(HANDSHAKE_TIMEOUT - Duration::from_millis(1)).await;
    assert!(poll_once(task.as_mut()).await.is_pending());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(matches!(task.await.outcome, Err(HandshakeFailure::Timeout)));
    assert_eq!(submissions.load(Ordering::Relaxed), 1);
    assert!(remote.control.control_tx.is_closed());
    assert!(remote.data_tx.is_closed());
}
