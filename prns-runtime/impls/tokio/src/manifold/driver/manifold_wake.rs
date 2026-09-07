use std::sync::atomic::{fence, AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

struct ManifoldWakeState {
    armed: AtomicBool,
    senders: AtomicUsize,
    notify: Notify,
}

pub struct ManifoldWakeSender {
    state: Arc<ManifoldWakeState>,
}

pub struct ManifoldWakeReceiver {
    state: Arc<ManifoldWakeState>,
}

pub(crate) enum ManifoldWakeEvent {
    Signaled,
    SendersDropped,
}

pub fn manifold_wake() -> (ManifoldWakeSender, ManifoldWakeReceiver) {
    let state = Arc::new(ManifoldWakeState {
        armed: AtomicBool::new(false),
        senders: AtomicUsize::new(1),
        notify: Notify::new(),
    });
    (
        ManifoldWakeSender {
            state: state.clone(),
        },
        ManifoldWakeReceiver { state },
    )
}

impl Clone for ManifoldWakeSender {
    fn clone(&self) -> Self {
        self.state.senders.fetch_add(1, Ordering::Relaxed);
        Self {
            state: self.state.clone(),
        }
    }
}

impl Drop for ManifoldWakeSender {
    fn drop(&mut self) {
        if self.state.senders.fetch_sub(1, Ordering::Release) == 1 {
            fence(Ordering::Acquire);
            self.state.notify.notify_one();
        }
    }
}

impl ManifoldWakeSender {
    pub fn signal(&self) {
        if self.state.armed.swap(false, Ordering::AcqRel) {
            self.state.notify.notify_one();
        }
    }
}

impl ManifoldWakeReceiver {
    pub(crate) fn arm(&self) {
        self.state.armed.swap(true, Ordering::AcqRel);
    }

    pub(crate) fn disarm(&self) {
        self.state.armed.store(false, Ordering::Release);
    }

    pub(crate) async fn wait(&self) -> ManifoldWakeEvent {
        if self.state.senders.load(Ordering::Acquire) == 0 {
            return ManifoldWakeEvent::SendersDropped;
        }
        self.state.notify.notified().await;
        match self.state.senders.load(Ordering::Acquire) {
            0 => ManifoldWakeEvent::SendersDropped,
            _ => ManifoldWakeEvent::Signaled,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_signal_only_notifies_an_armed_receiver() {
        let (sender, receiver) = manifold_wake();

        sender.signal();
        receiver.arm();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), receiver.wait())
                .await
                .is_err(),
        );

        sender.signal();
        let event = tokio::time::timeout(Duration::from_millis(20), receiver.wait())
            .await
            .expect("the armed receiver wakes");
        assert!(matches!(event, ManifoldWakeEvent::Signaled));
    }

    #[tokio::test]
    async fn one_signal_consumes_one_arm() {
        let (sender, receiver) = manifold_wake();

        receiver.arm();
        sender.signal();
        sender.signal();
        let event = tokio::time::timeout(Duration::from_millis(20), receiver.wait())
            .await
            .expect("the first signal wakes");
        assert!(matches!(event, ManifoldWakeEvent::Signaled));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), receiver.wait())
                .await
                .is_err(),
        );
    }

    #[tokio::test]
    async fn the_last_sender_closes_the_receiver() {
        let (sender, receiver) = manifold_wake();

        receiver.arm();
        drop(sender);
        let event = tokio::time::timeout(Duration::from_millis(20), receiver.wait())
            .await
            .expect("sender closure wakes the receiver");
        assert!(matches!(event, ManifoldWakeEvent::SendersDropped));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_arm_signal_handoffs_do_not_strand() {
        let (sender, receiver) = manifold_wake();

        for _ in 0..1_024 {
            receiver.arm();
            let signaler = sender.clone();
            let signal = tokio::spawn(async move {
                tokio::task::yield_now().await;
                signaler.signal();
            });
            let event = tokio::time::timeout(Duration::from_millis(20), receiver.wait())
                .await
                .expect("the concurrent signal wakes the receiver");
            assert!(matches!(event, ManifoldWakeEvent::Signaled));
            signal.await.unwrap();
        }
    }
}
