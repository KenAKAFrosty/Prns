//! Bounded, caller-owned notification admission. CoreBluetooth owns the bytes only after it
//! accepts an update; a full notification queue leaves the same fragment with its sender.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::MacosBleError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionPhase {
    Handshaking = 0,
    Settled = 1,
    Retired = 2,
}

#[derive(Default)]
struct SessionState {
    reserved: AtomicBool,
    phase: AtomicU8,
    liveness_listener: AtomicBool,
}

#[derive(Clone, Default)]
pub(super) struct NotificationSession(Arc<SessionState>);

/// The one settled control owner, distinct from notification reservations and data pumps.
pub(super) struct SettledSessionOwner(NotificationSession);

impl Drop for SettledSessionOwner {
    fn drop(&mut self) {
        self.0.retire();
    }
}

impl NotificationSession {
    /// Set only while preparing a new, unpublished inbound owner. Runtime gate changes never
    /// upgrade an existing legacy session; its eligibility is immutable after admission.
    pub(super) fn enable_liveness_listener(&self) {
        self.0.liveness_listener.store(true, Ordering::Release);
    }

    pub(super) fn is_liveness_listener(&self) -> bool {
        self.0.liveness_listener.load(Ordering::Acquire)
    }

    pub(super) fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn phase(&self) -> SessionPhase {
        match self.0.phase.load(Ordering::Acquire) {
            0 => SessionPhase::Handshaking,
            1 => SessionPhase::Settled,
            _ => SessionPhase::Retired,
        }
    }

    pub(super) fn settle(&self) {
        // Retirement is terminal even if a delayed admission reaches this owner.
        let _ = self.0.phase.compare_exchange(
            SessionPhase::Handshaking as u8,
            SessionPhase::Settled as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub(super) fn settled_owner(&self) -> SettledSessionOwner {
        self.settle();
        SettledSessionOwner(self.clone())
    }

    pub(super) fn retire(&self) {
        self.0
            .phase
            .store(SessionPhase::Retired as u8, Ordering::Release);
    }

    fn reserve(&self) -> Result<NotificationReservation, MacosBleError> {
        if self.phase() == SessionPhase::Retired {
            return Err(MacosBleError::Closed);
        }
        self.0
            .reserved
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| MacosBleError::QueueFull)?;
        Ok(NotificationReservation(self.0.clone()))
    }
}

struct NotificationReservation(Arc<SessionState>);

impl Drop for NotificationReservation {
    fn drop(&mut self) {
        self.0.reserved.store(false, Ordering::Release);
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
    async fn retired_session_rejects_new_notifications_before_submission() {
        let session = NotificationSession::default();
        drop(session.settled_owner());
        let (_ready_tx, ready_rx) = watch::channel(());
        let attempts = Cell::new(0);
        let result = send_notification(session, ready_rx, Duration::from_secs(1), || {
            attempts.set(attempts.get() + 1);
            ready(NotificationAdmission::Accepted)
        })
        .await;
        assert!(matches!(result, Err(MacosBleError::Closed)));
        assert_eq!(attempts.get(), 0);
    }

    #[test]
    fn settled_control_owner_retires_only_its_session() {
        let original = NotificationSession::default();
        let replacement = NotificationSession::default();
        assert_eq!(original.phase(), SessionPhase::Handshaking);
        let owner = original.settled_owner();
        let replacement_owner = replacement.settled_owner();
        assert_eq!(original.phase(), SessionPhase::Settled);
        drop(owner);
        assert_eq!(original.phase(), SessionPhase::Retired);
        original.settle();
        assert_eq!(original.phase(), SessionPhase::Retired);
        assert_eq!(replacement.phase(), SessionPhase::Settled);
        drop(replacement_owner);
        assert_eq!(replacement.phase(), SessionPhase::Retired);
    }

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
