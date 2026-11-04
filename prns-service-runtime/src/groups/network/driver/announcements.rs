use super::*;
use prns_identities::announces::{self, AnnounceKey, FileAnnounceScheduler, Settings};
use prns_identities_runtime::announce::{Fault, Reservation};
use std::{
    future::Future,
    path::Path,
    pin::Pin,
    task::Poll,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;

const CHANNELS: u64 = 3;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct Shared<const I: usize> {
    scheduler: FileAnnounceScheduler<I>,
    origin: Instant,
    alarm: Option<JoinHandle<()>>,
    probe: Option<JoinHandle<Result<bool, SessionError>>>,
    attempt: Option<Attempt>,
    failure: Option<core::ApiError>,
}
struct Attempt {
    owner: runtime::OwnerToken,
    reservation: Reservation,
    task: JoinHandle<u64>,
}
impl<const I: usize> Shared<I> {
    pub(super) fn open(root: &Path) -> Result<Self, DriverError> {
        let path = root.join("announces");
        let exists = path.try_exists().map_err(|e| {
            DriverError::Announce(announces::Error::Storage(prns_identities::Error::Io(e)))
        })?;
        let scheduler = if exists {
            FileAnnounceScheduler::open(path, 0, wall())
        } else {
            FileAnnounceScheduler::create_new(path, 0, wall())
        }
        .map_err(DriverError::Announce)?;
        Ok(Self {
            scheduler,
            origin: Instant::now(),
            alarm: None,
            probe: None,
            attempt: None,
            failure: None,
        })
    }
    fn now(&self) -> u64 {
        self.origin.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
    fn settings(&self, key: AnnounceKey, default: Option<u64>) -> Settings {
        Settings {
            interval_millis: self
                .scheduler
                .snapshot()
                .entries
                .iter()
                .flatten()
                .find(|entry| entry.key == key)
                .map_or(default, |entry| entry.configuration.interval_ms),
        }
    }
    fn configure(
        &mut self,
        key: AnnounceKey,
        settings: Settings,
        enabled: bool,
    ) -> Result<(), core::ApiError> {
        settings.validate()?;
        let changed = self
            .scheduler
            .snapshot()
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.key == key)
            .is_none_or(|entry| {
                entry.configuration.interval_ms != settings.interval_millis
                    || entry.configuration.enabled != enabled
            });
        self.scheduler
            .configure(
                key,
                CHANNELS,
                settings.interval_millis,
                enabled,
                self.now(),
                wall(),
            )
            .map_err(api_error)?;
        if changed {
            self.clear_alarm();
        }
        Ok(())
    }
    fn clear_alarm(&mut self) {
        if let Some(alarm) = self.alarm.take() {
            alarm.abort();
        }
    }
    fn wait(&mut self, network: &Context, duration: Duration, context: &mut TaskContext<'_>) {
        self.alarm = Some(
            network
                .executor()
                .spawn(async move { tokio::time::sleep(duration).await }),
        );
        if let Some(alarm) = &mut self.alarm {
            if Pin::new(alarm).poll(context).is_ready() {
                self.alarm = None;
                context.waker().wake_by_ref();
            }
        }
    }

    pub(super) fn pending(&self, owner: runtime::OwnerToken) -> bool {
        self.attempt
            .as_ref()
            .is_some_and(|attempt| attempt.owner == owner)
    }
    pub(super) fn detach(&mut self) {
        if let Some(alarm) = self.alarm.take() {
            alarm.abort();
        }
        if let Some(probe) = self.probe.take() {
            probe.abort();
        }
    }
}
impl<const I: usize> Drop for Shared<I> {
    fn drop(&mut self) {
        self.detach();
        // Normal Close drains an attempt before the owning identity/host. If
        // unwinding interrupts that path the durable reservation stays spent.
    }
}
fn wall() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
}
fn key(identity: core::IdentityReference) -> AnnounceKey {
    AnnounceKey {
        identity: identity.id,
        service: SERVICE,
    }
}
fn api_error(error: announces::Error<prns_identities::Error>) -> core::ApiError {
    use prns_identities_core::{ApiError, RuntimeFailure, StorageFailure};
    match error {
        announces::Error::Storage(error) => ApiError::from(&error),
        announces::Error::Fault(Fault::InvalidConfiguration | Fault::InvalidOutcome) => {
            ApiError::InvalidInput
        }
        announces::Error::Fault(Fault::InvalidSnapshot) => ApiError::Storage {
            reason: StorageFailure::Corrupt,
        },
        announces::Error::Fault(Fault::Capacity) => ApiError::Runtime {
            reason: RuntimeFailure::Capacity,
        },
        announces::Error::Fault(Fault::UnknownKey) => ApiError::Runtime {
            reason: RuntimeFailure::UnknownIdentity,
        },
        announces::Error::Fault(Fault::Exhausted) => ApiError::Runtime {
            reason: RuntimeFailure::Exhausted,
        },
        announces::Error::Fault(Fault::ReloadRequired) => ApiError::Runtime {
            reason: RuntimeFailure::StorageNotReady,
        },
        announces::Error::Fault(Fault::StaleReservation) => ApiError::Runtime {
            reason: RuntimeFailure::StaleAuthority,
        },
    }
}
fn admission_error(error: owned::AdmissionError) -> core::ApiError {
    let error: owned::Error<std::convert::Infallible> = match error {
        owned::AdmissionError::Store(error) => owned::Error::Store(error),
        owned::AdmissionError::Authority(error) => owned::Error::Authority(error),
        owned::AdmissionError::Runtime(error) => owned::Error::Runtime(error),
    };
    error.api_error(|never| match *never {})
}
// Updating public payload is temporary dispatch unavailability, not Pause:
// retain already accepted manual intent and the scheduler's global budget.
fn payload_ready(entry: &Resources) -> bool {
    #[cfg(feature = "contact-card")]
    if !entry.contact.current() {
        return false;
    }
    online(entry)
}
fn online(entry: &Resources) -> bool {
    entry.opened
        && entry
            .ingress_ready
            .load(std::sync::atomic::Ordering::Acquire)
        && entry.dispatch.is_open()
        && !entry.quiesced
        && entry.failure.is_none()
        && entry
            .route
            .as_ref()
            .is_some_and(|route| route.scope().check_open().is_ok())
}
fn eligible(interfaces: &[prns_runtime::node_introspection::InterfaceTimingSnapshot]) -> bool {
    use prns_core::interfaces::InterfaceKind;
    interfaces.iter().any(|interface| {
        interface.connection.is_online()
            && interface.capabilities.allows_transmit()
            && interface
                .id
                .kind()
                .is_some_and(|kind| kind != InterfaceKind::Loopback && kind.member_kind().is_none())
    })
}
impl<const I: usize, const S: usize, J: Send + 'static> Driver<I, S, J> {
    pub(super) fn announce_command(
        &mut self,
        job: announces::Job,
        admission: &mut dyn owned::Admission,
    ) {
        job.execute(|identity, request| {
            let access = match request {
                announces::Request::Settings => owned::Access::History,
                announces::Request::SetSettings(_) | announces::Request::Manual => {
                    owned::Access::LocalMutation
                }
            };
            let permit = admission
                .acquire(identity, access)
                .map_err(admission_error)?;
            let result = self.announce_request(identity, request);
            admission
                .settle(permit, runtime::Settlement::Committed)
                .map_err(admission_error)?;
            result
        });
    }
    fn announce_request(
        &mut self,
        identity: core::IdentityReference,
        request: announces::Request,
    ) -> Result<announces::Reply, core::ApiError> {
        if let Some(error) = self.announcements.failure {
            return Err(error);
        }
        let key = key(identity);
        let enabled = self
            .owners
            .iter()
            .any(|entry| entry.identity == Some(identity) && online(entry));
        let settings = self
            .announcements
            .settings(key, self.options.announce_interval_millis);
        match request {
            announces::Request::Settings => Ok(announces::Reply::Settings(settings)),
            announces::Request::SetSettings(settings) => {
                self.announcements.configure(key, settings, enabled)?;
                self.wake.wake_by_ref();
                Ok(announces::Reply::Settings(settings))
            }
            announces::Request::Manual => {
                self.announcements.configure(key, settings, enabled)?;
                let result = self
                    .announcements
                    .scheduler
                    .request_manual(key, self.announcements.now(), wall())
                    .map_err(api_error)?;
                self.announcements.clear_alarm();
                self.wake.wake_by_ref();
                Ok(announces::Reply::Manual(result))
            }
        }
    }
    pub(super) fn poll_announcements(&mut self, context: &mut TaskContext<'_>) {
        if let Err(error) = self.poll_announcements_inner(context) {
            self.announcements.failure = Some(error);
        }
    }
    fn poll_announcements_inner(
        &mut self,
        context: &mut TaskContext<'_>,
    ) -> Result<(), core::ApiError> {
        // Completion precedes every new reservation and is retained after the
        // requesting client disappears, Pause, or global Close.
        if let Some(attempt) = &mut self.announcements.attempt {
            match Pin::new(&mut attempt.task).poll(context) {
                Poll::Pending => return Ok(()),
                Poll::Ready(result) => {
                    let requested = result.unwrap_or(0);
                    let reservation = attempt.reservation;
                    self.announcements.attempt = None;
                    self.announcements
                        .scheduler
                        .complete(reservation, requested, CHANNELS & !requested)
                        .map_err(api_error)?;
                }
            }
        }
        if self.announcements.failure.is_some() {
            return Ok(());
        }
        // The immutable dispatch lease closes synchronously with identity
        // admission. Clear paused manual/automatic work before considering a
        // later activation; persisted preferences and spent budget survive.
        for index in 0..I {
            if let Some(entry) = self.announcements.scheduler.snapshot().entries[index] {
                let enabled = self.owners.iter().any(|owner| {
                    owner
                        .identity
                        .is_some_and(|identity| key(identity) == entry.key)
                        && online(owner)
                });
                self.announcements.configure(
                    entry.key,
                    Settings {
                        interval_millis: entry.configuration.interval_ms,
                    },
                    enabled,
                )?;
            }
        }
        for entry in &self.owners {
            if let Some(identity) = entry.identity.filter(|_| online(entry)) {
                let key = key(identity);
                let settings = self
                    .announcements
                    .settings(key, self.options.announce_interval_millis);
                self.announcements.configure(key, settings, true)?;
            }
        }
        let Some(network) = &self.network else {
            return Ok(());
        };
        if !self.owners.iter().any(online) {
            return Ok(());
        }
        let now = self.announcements.now();
        let Some(deadline) = self
            .announcements
            .scheduler
            .next_ready_at(now)
            .map_err(api_error)?
        else {
            self.announcements.detach();
            return Ok(());
        };
        if let Some(alarm) = &mut self.announcements.alarm {
            if Pin::new(alarm).poll(context).is_pending() {
                return Ok(());
            }
            self.announcements.alarm = None;
        }
        if deadline > now {
            self.announcements
                .wait(network, Duration::from_millis(deadline - now), context);
            return Ok(());
        }
        if self.announcements.probe.is_none() {
            let host = network.host().clone();
            self.announcements.probe = Some(network.executor().spawn(async move {
                host.protocol_operation(|handle| async move {
                    eligible(&handle.interface_timing_inventory())
                })
                .await
            }));
        }
        let probe = self
            .announcements
            .probe
            .as_mut()
            .ok_or(core::ApiError::WorkerStopped)?;
        let eligible = match Pin::new(probe).poll(context) {
            Poll::Pending => return Ok(()),
            Poll::Ready(result) => matches!(result, Ok(Ok(true))),
        };
        self.announcements.probe = None;
        self.announcements.wait(network, POLL_INTERVAL, context);
        if !eligible {
            return Ok(());
        }
        let mut entropy = [0; 2];
        getrandom::fill(&mut entropy).map_err(|_| core::ApiError::Storage {
            reason: core::StorageFailure::Entropy,
        })?;
        let owners = &self.owners;
        let now = self.announcements.now();
        let reservation = self
            .announcements
            .scheduler
            .reserve_next_matching(now, wall(), eligible, u16::from_le_bytes(entropy), |key| {
                owners.iter().any(|entry| {
                    payload_ready(entry)
                        && entry
                            .identity
                            .is_some_and(|identity| identity.id == key.identity)
                })
            })
            .map_err(api_error)?;
        let Some(reservation) = reservation else {
            return Ok(());
        };
        let entry = self
            .owners
            .iter()
            .find(|entry| {
                payload_ready(entry)
                    && entry
                        .identity
                        .is_some_and(|identity| identity.id == reservation.key().identity)
            })
            .ok_or(core::ApiError::WorkerStopped)?;
        let route = entry.route.as_ref().ok_or(core::ApiError::WorkerStopped)?;
        let scope = route.scope().clone();
        let destinations = [route.sync_destination(), route.enrollment_destination()];
        let task = network.executor().spawn(async move {
            let mut requested = 0;
            for (index, destination) in destinations.into_iter().enumerate() {
                let result = tokio::time::timeout(
                    ATTEMPT_TIMEOUT,
                    scope.execute(prns_host::HostCommand::Announce {
                        destination,
                        interface: None,
                    }),
                )
                .await;
                if matches!(result, Ok(Ok(Ok(prns_host::CommandOutcome::Announced)))) {
                    requested |= 1 << index;
                }
            }
            requested
        });
        self.announcements.attempt = Some(Attempt {
            owner: entry.owner,
            reservation,
            task,
        });
        context.waker().wake_by_ref();
        Ok(())
    }
}

#[cfg(all(test, feature = "contact-card"))]
impl<const I: usize, const S: usize, J: Send + 'static> Driver<I, S, J> {
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    pub(super) fn check_contact_refresh_retains_manual_intent(&mut self) {
        let index = self
            .owners
            .iter()
            .position(online)
            .expect("active identity");
        let identity = self.owners[index].identity.unwrap();
        let installed = self.owners[index].contact.installed;
        self.owners[index].contact.installed = None;
        assert!(online(&self.owners[index]));
        assert!(!payload_ready(&self.owners[index]));
        assert!(matches!(
            self.announce_request(identity, announces::Request::Manual)
                .unwrap(),
            announces::Reply::Manual(
                announces::ManualRequest::Queued | announces::ManualRequest::Throttled { .. }
            )
        ));
        self.poll_announcements_inner(&mut TaskContext::from_waker(Waker::noop()))
            .unwrap();
        let pending = self.announcements.scheduler.snapshot();
        assert!(
            pending
                .entries
                .iter()
                .flatten()
                .find(|entry| entry.key == key(identity))
                .unwrap()
                .configuration
                .enabled
        );
        let sequence = pending.last_sequence;
        let now = self.announcements.now();
        let due = self
            .announcements
            .scheduler
            .next_ready_at(now)
            .unwrap()
            .expect("manual request remains queued");
        assert!(self
            .announcements
            .scheduler
            .reserve_next_matching(due, wall(), true, 0, |_| false)
            .unwrap()
            .is_none());
        assert_eq!(
            self.announcements.scheduler.snapshot().last_sequence,
            sequence
        );
        self.owners[index].contact.installed = installed;
        assert!(payload_ready(&self.owners[index]));
        let reservation = self
            .announcements
            .scheduler
            .reserve_next_matching(due, wall(), true, 0, |_| true)
            .unwrap()
            .expect("same intent becomes eligible after payload update");
        self.announcements
            .scheduler
            .complete(reservation, 0, CHANNELS)
            .unwrap();
        // An existing name that is legal identity metadata but ineligible for
        // the optional card codec does not stop or misrepresent the service.
        let name = self.owners[index].contact.name;
        self.owners[index].contact.name = Some(core::BoundedText::new("old\u{202e}name").unwrap());
        assert!(super::contact::card(&self.owners[index])
            .unwrap()
            .is_empty());
        assert!(online(&self.owners[index]));
        self.owners[index].contact.name = name;
    }
}
