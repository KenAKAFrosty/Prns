//! Bounded, caller-owned notification admission. CoreBluetooth owns the bytes only after it
//! accepts an update; a full notification queue leaves the same fragment with its sender.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::MacosBleError;

#[derive(Clone, Default)]
pub(super) struct NotificationSession(Arc<AtomicBool>);

impl NotificationSession {
    pub(super) fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    fn reserve(&self) -> Result<NotificationReservation, MacosBleError> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| MacosBleError::QueueFull)?;
        Ok(NotificationReservation(self.0.clone()))
    }
}

struct NotificationReservation(Arc<AtomicBool>);

impl Drop for NotificationReservation {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotificationAdmission {
    Accepted,
    Backpressured,
    Closed,
}

pub(super) async fn send_notification<F: Future<Output = NotificationAdmission>>(
    session: NotificationSession,
    mut ready: watch::Receiver<()>,
    timeout: Duration,
    mut attempt: impl FnMut() -> F,
) -> Result<(), MacosBleError> {
    // A link's handshake and data writers are sequential. Refuse unexpected concurrent writers
    // instead of adding an unbounded queue or allowing their fragments to interleave.
    let _reservation = session.reserve()?;
    tokio::time::timeout(timeout, async {
        loop {
            match attempt().await {
                NotificationAdmission::Accepted => return Ok(()),
                NotificationAdmission::Closed => return Err(MacosBleError::Closed),
                NotificationAdmission::Backpressured => {
                    // The caller subscribed before the first attempt. A callback between a full
                    // queue result and this await changes the watch version and cannot be lost.
                    ready.changed().await.map_err(|_| MacosBleError::Closed)?;
                }
            }
        }
    })
    .await
    .map_err(|_| MacosBleError::GattWriteTimeout)?
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::ready;

    use super::*;

    #[tokio::test]
    async fn a_ready_callback_during_admission_retries_every_fragment_in_order() {
        let session = NotificationSession::default();
        let (signal, _) = watch::channel(());
        let sent = RefCell::new(Vec::new());
        for fragment in [vec![1, 2], vec![3, 4], vec![5]] {
            let full = Cell::new(true);
            send_notification(
                session.clone(),
                signal.subscribe(),
                Duration::from_secs(1),
                || {
                    ready(if full.replace(false) {
                        // Deliberately fire ready before the sender starts awaiting it.
                        signal.send_replace(());
                        NotificationAdmission::Backpressured
                    } else {
                        sent.borrow_mut().push(fragment.clone());
                        NotificationAdmission::Accepted
                    })
                },
            )
            .await
            .unwrap();
        }
        assert_eq!(*sent.borrow(), vec![vec![1, 2], vec![3, 4], vec![5]]);
    }

    #[tokio::test]
    async fn notification_capacity_is_one_and_cancellation_releases_it() {
        let session = NotificationSession::default();
        let (signal, receiver) = watch::channel(());
        let (started, waiting) = tokio::sync::oneshot::channel();
        let mut started = Some(started);
        let pending_session = session.clone();
        let pending = tokio::spawn(async move {
            send_notification(pending_session, receiver, Duration::from_secs(5), || {
                if let Some(started) = started.take() {
                    let _ = started.send(());
                }
                ready(NotificationAdmission::Backpressured)
            })
            .await
        });
        waiting.await.unwrap();
        assert!(matches!(
            send_notification(
                session.clone(),
                signal.subscribe(),
                Duration::from_secs(1),
                || async { unreachable!("a second writer must not reach CoreBluetooth") }
            )
            .await,
            Err(MacosBleError::QueueFull)
        ));
        pending.abort();
        let _ = pending.await;
        send_notification(session, signal.subscribe(), Duration::from_secs(1), || {
            ready(NotificationAdmission::Accepted)
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_missing_ready_callback_times_out_without_false_success() {
        let session = NotificationSession::default();
        let (_signal, receiver) = watch::channel(());
        let result =
            send_notification(session.clone(), receiver, Duration::from_millis(10), || {
                ready(NotificationAdmission::Backpressured)
            })
            .await;
        assert!(matches!(result, Err(MacosBleError::GattWriteTimeout)));
        assert!(session.reserve().is_ok());
    }

    #[tokio::test]
    async fn off_close_or_replacement_wakes_and_fences_a_waiting_sender() {
        for replace in [false, true] {
            let original = NotificationSession::default();
            let current = RefCell::new(Some(original.clone()));
            let (signal, receiver) = watch::channel(());
            let attempts = Cell::new(0);
            let result =
                send_notification(original.clone(), receiver, Duration::from_secs(1), || {
                    attempts.set(attempts.get() + 1);
                    let same = current
                        .borrow()
                        .as_ref()
                        .is_some_and(|live| live.same_session(&original));
                    ready(if !same {
                        NotificationAdmission::Closed
                    } else {
                        // This is the exact state transition performed by Off/close or a fresh Hello.
                        *current.borrow_mut() = replace.then(NotificationSession::default);
                        signal.send_replace(());
                        NotificationAdmission::Backpressured
                    })
                })
                .await;
            assert!(matches!(result, Err(MacosBleError::Closed)));
            assert_eq!(attempts.get(), 2);
            assert!(original.reserve().is_ok());
        }
    }
}
