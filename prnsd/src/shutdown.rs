use tokio::sync::oneshot;

pub(crate) struct ShutdownSignal {
    receiver: oneshot::Receiver<()>,
}

impl ShutdownSignal {
    pub(crate) async fn requested(self) {
        let _ = self.receiver.await;
    }
}

#[cfg(feature = "tray")]
pub(crate) struct ShutdownRequest {
    sender: Option<oneshot::Sender<()>>,
}

#[cfg(feature = "tray")]
impl ShutdownRequest {
    pub(crate) fn request(&mut self) -> bool {
        self.sender
            .take()
            .is_some_and(|sender| sender.send(()).is_ok())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn was_requested(&self) -> bool {
        self.sender.is_none()
    }
}

#[cfg(feature = "tray")]
pub(crate) fn channel() -> (ShutdownRequest, ShutdownSignal) {
    let (sender, receiver) = oneshot::channel();
    (
        ShutdownRequest {
            sender: Some(sender),
        },
        ShutdownSignal { receiver },
    )
}

#[cfg(all(test, feature = "tray"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tray_requests_are_one_shot_and_wake_the_daemon() {
        let (mut request, signal) = channel();

        assert!(request.request());
        assert!(!request.request());
        signal.requested().await;
    }
}
