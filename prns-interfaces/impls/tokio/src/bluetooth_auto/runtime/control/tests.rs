use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
struct Closed;

struct Endpoint {
    mode: LivenessMode,
    incoming: mpsc::UnboundedReceiver<Control>,
    outgoing: mpsc::UnboundedSender<Control>,
}

impl BleControl for Endpoint {
    type Error = Closed;
    fn liveness_mode(&self) -> LivenessMode {
        self.mode
    }
    async fn send(&mut self, message: &Control) -> Result<(), Closed> {
        self.outgoing.send(*message).map_err(|_| Closed)
    }
    async fn recv(&mut self) -> Result<Control, Closed> {
        self.incoming.recv().await.ok_or(Closed)
    }
}

type Running = tokio::task::JoinHandle<Result<CloseReason, ControlError<Closed>>>;

fn run(
    mode: LivenessMode,
) -> (
    mpsc::UnboundedSender<Control>,
    mpsc::UnboundedReceiver<Control>,
    Running,
) {
    let (incoming, receiver) = mpsc::unbounded_channel();
    let (sender, outgoing) = mpsc::unbounded_channel();
    let mut control = Endpoint {
        mode,
        incoming: receiver,
        outgoing: sender,
    };
    let task = tokio::spawn(async move { observe_with_seed(&mut control, mode, 42).await });
    (incoming, outgoing, task)
}

async fn turn() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn first_probe_is_immediate_and_native_completion_is_not_a_reply() {
    let (_input, mut output, task) = run(LivenessMode::Initiator);
    assert!(matches!(
        output.recv().await,
        Some(Control::Probe { nonce: 42 })
    ));
    tokio::time::advance(Duration::from_millis(29_999)).await;
    turn().await;
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(ControlError::LivenessExpired)
    ));
}

#[tokio::test(start_paused = true)]
async fn matched_reply_starts_a_new_cadence_and_nonce() {
    let (input, mut output, task) = run(LivenessMode::Initiator);
    assert!(matches!(
        output.recv().await,
        Some(Control::Probe { nonce: 42 })
    ));
    tokio::time::advance(Duration::from_secs(5)).await;
    input.send(Control::ProbeReply { nonce: 42 }).unwrap();
    turn().await;
    tokio::time::advance(Duration::from_millis(29_999)).await;
    turn().await;
    assert!(output.try_recv().is_err());
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(matches!(
        output.recv().await,
        Some(Control::Probe { nonce: 43 })
    ));
    // A delayed duplicate from the preceding round cannot save this one.
    input.send(Control::ProbeReply { nonce: 42 }).unwrap();
    turn().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(ControlError::LivenessExpired)
    ));
}

#[tokio::test(start_paused = true)]
async fn queued_reply_after_local_suspension_does_not_renew_the_budget() {
    let (input, mut output, task) = run(LivenessMode::Initiator);
    output.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(31)).await;
    // Do not yield between advancing the clock and making input ready.
    let _ = input.send(Control::ProbeReply { nonce: 42 });
    assert!(matches!(
        task.await.unwrap(),
        Err(ControlError::LivenessExpired)
    ));
}

#[tokio::test(start_paused = true)]
async fn legacy_session_ignores_probes_and_has_no_timer() {
    let (input, mut output, task) = run(LivenessMode::Disabled);
    input.send(Control::Probe { nonce: 42 }).unwrap();
    turn().await;
    tokio::time::advance(Duration::from_secs(86_400)).await;
    turn().await;
    assert!(output.try_recv().is_err());
    assert!(!task.is_finished());
    input
        .send(Control::Close {
            reason: CloseReason::DuplicateLink,
        })
        .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Ok(CloseReason::DuplicateLink)
    ));
}

#[tokio::test(start_paused = true)]
async fn listener_does_not_require_entropy_until_peer_opts_in() {
    let (input, receiver) = mpsc::unbounded_channel();
    let (sender, mut output) = mpsc::unbounded_channel();
    let mut control = Endpoint {
        mode: LivenessMode::Listener,
        incoming: receiver,
        outgoing: sender,
    };
    let task = tokio::spawn(async move {
        observe_with_entropy(&mut control, LivenessMode::Listener, || None).await
    });
    input.send(Control::ProbeReply { nonce: 42 }).unwrap();
    turn().await;
    tokio::time::advance(Duration::from_secs(86_400)).await;
    turn().await;
    assert!(!task.is_finished());
    assert!(output.try_recv().is_err());
    input.send(Control::Probe { nonce: 42 }).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(ControlError::EntropyUnavailable)
    ));
}

#[tokio::test(start_paused = true)]
async fn listener_waits_for_a_probe_then_enforces_its_own_deadline() {
    let (input, mut output, task) = run(LivenessMode::Listener);
    turn().await;
    tokio::time::advance(Duration::from_secs(3_600)).await;
    turn().await;
    assert!(!task.is_finished());
    assert!(output.try_recv().is_err());
    input.send(Control::Probe { nonce: 99 }).unwrap();
    assert!(matches!(
        output.recv().await,
        Some(Control::ProbeReply { nonce: 99 })
    ));
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(matches!(
        output.recv().await,
        Some(Control::Probe { nonce: 42 })
    ));
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(ControlError::LivenessExpired)
    ));
}

#[tokio::test(start_paused = true)]
async fn excess_probes_are_bounded_and_do_not_postpone_own_expiration() {
    let (input, mut output, task) = run(LivenessMode::Initiator);
    output.recv().await.unwrap();
    for nonce in 100..200 {
        input.send(Control::Probe { nonce }).unwrap();
    }
    for _ in 0..120 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        output.try_recv().unwrap(),
        Control::ProbeReply { nonce: 100 }
    ));
    assert!(output.try_recv().is_err());
    tokio::time::advance(Duration::from_secs(29)).await;
    input.send(Control::Probe { nonce: 200 }).unwrap();
    input.send(Control::ProbeReply { nonce: 123 }).unwrap();
    turn().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(ControlError::LivenessExpired)
    ));
}

struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Blocked {
    endpoint: Endpoint,
    sends: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    finish: oneshot::Receiver<()>,
    block_probe: bool,
}

impl BleControl for Blocked {
    type Error = Closed;
    async fn send(&mut self, message: &Control) -> Result<(), Closed> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        self.endpoint.send(message).await?;
        if matches!(message, Control::Probe { .. }) == self.block_probe {
            let _drop = DropCount(self.drops.clone());
            (&mut self.finish).await.map_err(|_| Closed)?;
        }
        Ok(())
    }
    async fn recv(&mut self) -> Result<Control, Closed> {
        self.endpoint.recv().await
    }
}

#[tokio::test(start_paused = true)]
async fn blocked_send_is_not_restarted_and_uses_the_original_budget() {
    let (incoming, receiver) = mpsc::unbounded_channel();
    let (sender, mut outgoing) = mpsc::unbounded_channel();
    let (_finish, finished) = oneshot::channel();
    let sends = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let mut control = Blocked {
        endpoint: Endpoint {
            mode: LivenessMode::Initiator,
            incoming: receiver,
            outgoing: sender,
        },
        sends: sends.clone(),
        drops: drops.clone(),
        finish: finished,
        block_probe: true,
    };
    let task =
        tokio::spawn(
            async move { observe_with_seed(&mut control, LivenessMode::Initiator, 42).await },
        );
    outgoing.recv().await.unwrap();
    incoming.send(Control::ProbeReply { nonce: 42 }).unwrap();
    for _ in 0..10 {
        tokio::time::advance(Duration::from_secs(2)).await;
        turn().await;
    }
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(ControlError::LivenessExpired)
    ));
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn blocked_reply_cannot_hide_an_earlier_own_probe_deadline() {
    let (incoming, receiver) = mpsc::unbounded_channel();
    let (sender, mut outgoing) = mpsc::unbounded_channel();
    let (_finish, finished) = oneshot::channel();
    let mut control = Blocked {
        endpoint: Endpoint {
            mode: LivenessMode::Initiator,
            incoming: receiver,
            outgoing: sender,
        },
        sends: Arc::new(AtomicUsize::new(0)),
        drops: Arc::new(AtomicUsize::new(0)),
        finish: finished,
        block_probe: false,
    };
    let task =
        tokio::spawn(
            async move { observe_with_seed(&mut control, LivenessMode::Initiator, 42).await },
        );
    outgoing.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(29)).await;
    incoming.send(Control::Probe { nonce: 99 }).unwrap();
    assert!(matches!(
        outgoing.recv().await,
        Some(Control::ProbeReply { nonce: 99 })
    ));
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(
        task.await.unwrap(),
        Err(ControlError::LivenessExpired)
    ));
}

#[tokio::test(start_paused = true)]
async fn two_negotiated_peers_remain_live_through_multiple_rounds() {
    let (a_tx, a_rx) = mpsc::unbounded_channel();
    let (b_tx, b_rx) = mpsc::unbounded_channel();
    let mut a = Endpoint {
        mode: LivenessMode::Initiator,
        incoming: a_rx,
        outgoing: b_tx,
    };
    let mut b = Endpoint {
        mode: LivenessMode::Listener,
        incoming: b_rx,
        outgoing: a_tx,
    };
    let a =
        tokio::spawn(async move { observe_with_seed(&mut a, LivenessMode::Initiator, 42).await });
    let b =
        tokio::spawn(async move { observe_with_seed(&mut b, LivenessMode::Listener, 99).await });
    turn().await;
    for _ in 0..20 {
        tokio::time::advance(Duration::from_secs(30)).await;
        turn().await;
        assert!(!a.is_finished());
        assert!(!b.is_finished());
    }
    a.abort();
    b.abort();
}
