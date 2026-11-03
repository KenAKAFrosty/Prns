//! One bounded announcement owner per device, separate from content polling.
use crate::Error;
use prns_host::{CommandOutcome, DestinationHash, HostCommand};
use prns_host_runtime::owner::HostClient;
use prns_host_runtime::service_scope::HostServiceScope;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

pub const MANUAL_ANNOUNCE_COOLDOWN_MILLIS: u64 = 60_000;
const MAX_INTERVAL: u64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceOutcome {
    /// Both destination commands were accepted by the local host, not received by peers.
    Requested,
    Throttled {
        retry_after_millis: u64,
    },
    Busy,
    Unavailable,
}

#[derive(Clone)]
pub struct AnnounceControl {
    settings: watch::Sender<Option<u64>>,
    requests: mpsc::Sender<oneshot::Sender<AnnounceOutcome>>,
}

/// The caller passes this to exactly one multi-group runner. Dropping the runner
/// closes pending manual waiters; announcements never create another node/task.
pub struct AnnounceRequests {
    settings: watch::Receiver<Option<u64>>,
    requests: mpsc::Receiver<oneshot::Sender<AnnounceOutcome>>,
    // Keep channels open when a caller chooses only automatic announcements.
    _control: AnnounceControl,
}

pub fn announce_control(
    interval_millis: Option<u64>,
) -> Result<(AnnounceControl, AnnounceRequests), Error> {
    validate(interval_millis)?;
    let (settings, receiver) = watch::channel(interval_millis);
    let (requests, request_receiver) = mpsc::channel(1);
    let control = AnnounceControl { settings, requests };
    Ok((
        control.clone(),
        AnnounceRequests {
            settings: receiver,
            requests: request_receiver,
            _control: control,
        },
    ))
}

fn validate(interval: Option<u64>) -> Result<(), Error> {
    if interval
        .is_some_and(|value| !(MANUAL_ANNOUNCE_COOLDOWN_MILLIS..=MAX_INTERVAL).contains(&value))
    {
        Err(Error::Configuration)
    } else {
        Ok(())
    }
}

impl AnnounceControl {
    /// None disables startup and periodic announces. Changes do not announce.
    pub fn set_interval(&self, interval_millis: Option<u64>) -> Result<(), Error> {
        validate(interval_millis)?;
        self.settings.send_if_modified(|current| {
            if *current == interval_millis {
                false
            } else {
                *current = interval_millis;
                true
            }
        });
        Ok(())
    }

    pub async fn announce(&self) -> AnnounceOutcome {
        let (reply, receive) = oneshot::channel();
        match self.requests.try_send(reply) {
            Ok(()) => receive.await.unwrap_or(AnnounceOutcome::Unavailable),
            Err(mpsc::error::TrySendError::Full(_)) => AnnounceOutcome::Busy,
            Err(mpsc::error::TrySendError::Closed(_)) => AnnounceOutcome::Unavailable,
        }
    }
}

struct Schedule {
    interval: Option<u64>,
    due: Option<Instant>,
    last: Option<Instant>,
}
impl Schedule {
    fn new(interval: Option<u64>, now: Instant) -> Self {
        Self {
            interval,
            due: interval.map(|_| now),
            last: None,
        }
    }
    fn delay(interval: u64, jitter: u32) -> Duration {
        Duration::from_millis(interval + u64::from(jitter) % (interval / 4 + 1))
    }
    fn changed(&mut self, interval: Option<u64>, now: Instant, jitter: u32) {
        self.interval = interval;
        // Editing preferences is not a network event, even when re-enabling.
        self.due = interval.map(|value| now + Self::delay(value, jitter));
    }
    fn attempt(&mut self, now: Instant, jitter: u32) -> Result<(), u64> {
        if let Some(last) = self.last {
            let remaining = (last + Duration::from_millis(MANUAL_ANNOUNCE_COOLDOWN_MILLIS))
                .saturating_duration_since(now);
            if !remaining.is_zero() {
                return Err(remaining.as_millis().min(u128::from(u64::MAX)) as u64);
            }
        }
        // Failures and timeouts consume the cooldown too: host admission can
        // have succeeded even when the result is uncertain.
        self.last = Some(now);
        self.due = self.interval.map(|value| now + Self::delay(value, jitter));
        Ok(())
    }
}

async fn wait_due(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

pub(crate) async fn run(
    host: &HostClient,
    scope: Option<&HostServiceScope>,
    destinations: [DestinationHash; 2],
    input: AnnounceRequests,
) -> Result<(), Error> {
    let identity = match scope {
        Some(scope) => scope.identity_hash(),
        None => host.identity_hash().map_err(|_| Error::Unavailable)?,
    };
    let jitter = u32::from_be_bytes(
        identity.as_bytes()[..4]
            .try_into()
            .map_err(|_| Error::Configuration)?,
    ) | 1;
    drive(input, jitter, || async {
        let mut accepted = true;
        for destination in destinations {
            accepted &= matches!(
                tokio::time::timeout(Duration::from_secs(10), async {
                    let command = HostCommand::Announce {
                        destination,
                        interface: None,
                    };
                    match scope {
                        Some(scope) => scope.execute(command).await.map_err(|_| ()),
                        None => host.execute(command).await.map_err(|_| ()),
                    }
                })
                .await,
                Ok(Ok(Ok(CommandOutcome::Announced)))
            );
        }
        accepted
    })
    .await
}

async fn drive<F, R>(mut input: AnnounceRequests, mut jitter: u32, mut emit: F) -> Result<(), Error>
where
    F: FnMut() -> R,
    R: std::future::Future<Output = bool>,
{
    let mut schedule = Schedule::new(*input.settings.borrow_and_update(), Instant::now());
    loop {
        let reply = tokio::select! {
            // Apply disable/interval changes before a concurrently due timer.
            biased;
            changed = input.settings.changed() => {
                if changed.is_err() { return Ok(()); }
                let interval = *input.settings.borrow_and_update();
                schedule.changed(interval, Instant::now(), crate::hub::next_jitter(&mut jitter));
                continue;
            },
            reply = input.requests.recv() => {
                let Some(reply) = reply else { return Ok(()); };
                Some(reply)
            },
            () = wait_due(schedule.due) => None,
        };
        let outcome = match schedule.attempt(Instant::now(), crate::hub::next_jitter(&mut jitter)) {
            Err(retry_after_millis) => AnnounceOutcome::Throttled { retry_after_millis },
            Ok(()) => {
                let accepted = emit().await;
                if accepted {
                    AnnounceOutcome::Requested
                } else {
                    AnnounceOutcome::Unavailable
                }
            }
        };
        if let Some(reply) = reply {
            let _ = reply.send(outcome);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn startup_manual_and_periodic_share_cooldown_without_announcement_on_edit() {
        let now = Instant::now();
        let mut schedule = Schedule::new(Some(3_600_000), now);
        assert_eq!(schedule.due, Some(now));
        schedule.attempt(now, 0).unwrap();
        assert_eq!(
            schedule.attempt(now + Duration::from_secs(1), 0),
            Err(59_000)
        );
        let due = schedule.due;
        assert_eq!(due, Some(now + Duration::from_secs(3600)));
        schedule.changed(None, now, 0);
        assert_eq!(schedule.due, None);
        assert_eq!(
            schedule.attempt(now + Duration::from_secs(30), 0),
            Err(30_000)
        );
        schedule.attempt(now + Duration::from_secs(60), 0).unwrap();
        assert_eq!(schedule.due, None);
        schedule.changed(Some(900_000), now + Duration::from_secs(61), 0);
        assert_eq!(schedule.due, Some(now + Duration::from_secs(961)));
    }

    #[test]
    fn disabled_startup_and_positive_jitter_are_bounded() {
        assert_eq!(Schedule::new(None, Instant::now()).due, None);
        for jitter in [0, 1, u32::MAX] {
            let delay = Schedule::delay(3_600_000, jitter).as_millis();
            assert!((3_600_000..=4_500_000).contains(&delay));
        }
        assert!(announce_control(Some(0)).is_err());
        assert!(announce_control(Some(MAX_INTERVAL + 1)).is_err());
    }

    #[tokio::test]
    async fn manual_queue_is_bounded_and_closing_resolves_waiters() {
        let (control, input) = announce_control(None).unwrap();
        let first = control.announce();
        tokio::pin!(first);
        assert!(futures_util::poll!(&mut first).is_pending());
        assert_eq!(control.announce().await, AnnounceOutcome::Busy);
        drop(input);
        assert_eq!(first.await, AnnounceOutcome::Unavailable);
        assert_eq!(control.announce().await, AnnounceOutcome::Unavailable);
    }
    #[tokio::test(start_paused = true)]
    async fn live_driver_changes_timers_without_extra_batches_and_reports_failures() {
        use std::sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        };
        let count = Arc::new(AtomicUsize::new(0));
        let success = Arc::new(AtomicBool::new(true));
        let (control, input) = announce_control(Some(3_600_000)).unwrap();
        let sent = count.clone();
        let accepted = success.clone();
        let task = tokio::spawn(drive(input, 1, move || {
            sent.fetch_add(1, Ordering::SeqCst);
            std::future::ready(accepted.load(Ordering::SeqCst))
        }));
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            control.announce().await,
            AnnounceOutcome::Throttled {
                retry_after_millis: 60_000
            }
        );
        control.set_interval(None).unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(7_200)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(control.announce().await, AnnounceOutcome::Requested);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        control.set_interval(Some(900_000)).unwrap();
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
        tokio::time::advance(Duration::from_secs(1_126)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 3);
        success.store(false, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(control.announce().await, AnnounceOutcome::Unavailable);
        assert_eq!(
            control.announce().await,
            AnnounceOutcome::Throttled {
                retry_after_millis: 60_000
            }
        );
        task.abort();
        let _ = task.await;
        assert_eq!(control.announce().await, AnnounceOutcome::Unavailable);
    }
}
