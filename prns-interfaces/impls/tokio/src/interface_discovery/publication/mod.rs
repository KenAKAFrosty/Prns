use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use prns_core::crypto::{sealed_len, X25519SecretKey};
use prns_core::engine::{AnnounceAppData, AnnounceAppDataBytes, AnnounceNow, AnnounceTarget};
use prns_core::identity::{RemoteIdentity, ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN, ENCRYPTION_IV_LEN};
use prns_core::interface_discovery::{
    frame_discovery_publication, prepare_discovery_publication_with_stamp_cache,
    DiscoveryAdvertisement, DiscoveryEncodeError, DiscoveryPublicationFrameError,
    DiscoveryPublicationPreparation, DiscoveryPublicationRegistration,
    DiscoveryPublicationSchedule, DiscoveryPublicationScheduleError, DiscoveryPublicationSecurity,
    StampValue,
};
use prns_core::interfaces::InterfaceId;
use prns_core::routing::announce::emit::MAX_ANNOUNCE_APP_DATA_LEN;
use prns_core::wire::DestinationHash;
use prns_runtime::manifold::driver::TokioHost;
use prns_runtime::manifold::Host;
use prns_runtime::runtime::{AnnounceNowError, PrnsNodeHandle};
use tokio::sync::Notify;
use tokio::task::{JoinError, JoinHandle};

pub const DISCOVERY_PUBLICATION_JOB_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokioDiscoveryPublisherConstructionError {
    NoPublications,
    InvalidSchedule(DiscoveryPublicationScheduleError),
    MissingNetworkIdentity { interface: InterfaceId },
}

#[derive(Debug)]
pub enum TokioDiscoveryPublicationPreparationFailure {
    Encode(DiscoveryEncodeError),
    InvalidReachableOn { value: String },
    Entropy(getrandom::Error),
    AppDataTooLong { required: usize, maximum: usize },
    Worker(JoinError),
}

#[derive(Debug)]
pub enum TokioDiscoveryPublicationFramingFailure {
    Entropy(getrandom::Error),
    Frame(DiscoveryPublicationFrameError),
    AppDataTooLong { actual: usize, maximum: usize },
}

pub enum TokioDiscoveryPublicationEvent<E> {
    AdvertisementUnavailable {
        interface: InterfaceId,
        error: E,
    },
    PreparationFailed {
        interface: InterfaceId,
        failure: TokioDiscoveryPublicationPreparationFailure,
    },
    Prepared {
        interface: InterfaceId,
        stamp_value: StampValue,
        stamp_attempts: u64,
        cache_hit: bool,
    },
    FramingFailed {
        interface: InterfaceId,
        failure: TokioDiscoveryPublicationFramingFailure,
    },
    AnnounceFailed {
        interface: InterfaceId,
        failure: AnnounceNowError,
    },
    Announced {
        interface: InterfaceId,
        app_data_bytes: usize,
    },
}

pub struct TokioInterfaceDiscoveryPublisher {
    destination: DestinationHash,
    network_identity: Option<RemoteIdentity>,
    registrations: Vec<DiscoveryPublicationRegistration>,
    schedule: DiscoveryPublicationSchedule,
    stamp_cache: BTreeMap<InterfaceId, CachedDiscoveryStamp>,
    job_interval: Duration,
}

#[derive(Clone, Copy)]
struct CachedDiscoveryStamp {
    advertisement_hash: [u8; 32],
    stamp: [u8; 32],
}

impl TokioInterfaceDiscoveryPublisher {
    pub fn new(
        destination: DestinationHash,
        registrations: impl IntoIterator<Item = DiscoveryPublicationRegistration>,
        network_identity: Option<RemoteIdentity>,
    ) -> Result<Self, TokioDiscoveryPublisherConstructionError> {
        Self::with_job_interval(
            destination,
            registrations,
            network_identity,
            DISCOVERY_PUBLICATION_JOB_INTERVAL,
        )
    }

    fn with_job_interval(
        destination: DestinationHash,
        registrations: impl IntoIterator<Item = DiscoveryPublicationRegistration>,
        network_identity: Option<RemoteIdentity>,
        job_interval: Duration,
    ) -> Result<Self, TokioDiscoveryPublisherConstructionError> {
        let registrations = registrations.into_iter().collect::<Vec<_>>();
        if registrations.is_empty() {
            return Err(TokioDiscoveryPublisherConstructionError::NoPublications);
        }
        let schedule = DiscoveryPublicationSchedule::new(
            registrations
                .iter()
                .copied()
                .map(DiscoveryPublicationRegistration::timing),
        )
        .map_err(TokioDiscoveryPublisherConstructionError::InvalidSchedule)?;
        if let Some(registration) = registrations.iter().find(|registration| {
            registration.security == DiscoveryPublicationSecurity::NetworkEncrypted
                && network_identity.is_none()
        }) {
            return Err(
                TokioDiscoveryPublisherConstructionError::MissingNetworkIdentity {
                    interface: registration.interface,
                },
            );
        }
        Ok(Self {
            destination,
            network_identity,
            registrations,
            schedule,
            stamp_cache: BTreeMap::new(),
            job_interval,
        })
    }

    pub fn spawn<E, Resolve, ResolveFuture, Report>(
        self,
        handle: PrnsNodeHandle,
        clock: TokioHost,
        resolve: Resolve,
        report: Report,
    ) -> RunningTokioInterfaceDiscoveryPublisher
    where
        E: Send + 'static,
        Resolve: FnMut(InterfaceId) -> ResolveFuture + Send + 'static,
        ResolveFuture: Future<Output = Result<DiscoveryAdvertisement, E>> + Send + 'static,
        Report: FnMut(TokioDiscoveryPublicationEvent<E>) + Send + 'static,
    {
        let cancellation = PublicationCancellation::new();
        let task_cancellation = cancellation.clone();
        let destination = self.destination;
        let task = tokio::spawn(async move {
            self.run_with(
                clock,
                resolve,
                move |app_data| {
                    let handle = handle.clone();
                    async move {
                        handle
                            .announce_now(AnnounceNow {
                                destination,
                                target: AnnounceTarget::AllInterfaces,
                                app_data: AnnounceAppData::Data(app_data),
                            })
                            .await
                    }
                },
                report,
                task_cancellation,
            )
            .await;
        });
        RunningTokioInterfaceDiscoveryPublisher { cancellation, task }
    }

    async fn run_with<E, Resolve, ResolveFuture, Send, SendFuture, Report>(
        mut self,
        clock: TokioHost,
        mut resolve: Resolve,
        mut send: Send,
        mut report: Report,
        cancellation: PublicationCancellation,
    ) where
        Resolve: FnMut(InterfaceId) -> ResolveFuture,
        ResolveFuture: Future<Output = Result<DiscoveryAdvertisement, E>>,
        Send: FnMut(AnnounceAppDataBytes) -> SendFuture,
        SendFuture: Future<Output = Result<(), AnnounceNowError>>,
        Report: FnMut(TokioDiscoveryPublicationEvent<E>),
    {
        loop {
            tokio::select! {
                () = tokio::time::sleep(self.job_interval) => {}
                () = cancellation.wait() => return,
            }
            let now = clock.now();
            let Some(interface) = self.schedule.next_due(now) else {
                continue;
            };
            if self.schedule.record_attempt(interface, now).is_err() {
                return;
            }
            let advertisement = tokio::select! {
                resolved = resolve(interface) => match resolved {
                    Ok(advertisement) => advertisement,
                    Err(error) => {
                        report(TokioDiscoveryPublicationEvent::AdvertisementUnavailable {
                            interface,
                            error,
                        });
                        continue;
                    }
                },
                () = cancellation.wait() => return,
            };
            let Some(registration) = self
                .registrations
                .iter()
                .find(|registration| registration.interface == interface)
                .copied()
            else {
                return;
            };
            let cached_stamp = self.stamp_cache.get(&interface).copied();
            let worker_cancellation = cancellation.clone();
            let mut worker = tokio::task::spawn_blocking(move || {
                prepare_discovery_publication_with_stamp_cache(
                    &advertisement,
                    registration.stamp_cost,
                    registration.security,
                    |hash| {
                        cached_stamp
                            .filter(|cached| cached.advertisement_hash == *hash.as_bytes())
                            .map(|cached| cached.stamp)
                    },
                    |candidate| getrandom::getrandom(candidate),
                    || worker_cancellation.is_cancelled(),
                )
            });
            let prepared = tokio::select! {
                joined = &mut worker => match joined {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        report(TokioDiscoveryPublicationEvent::PreparationFailed {
                            interface,
                            failure: TokioDiscoveryPublicationPreparationFailure::Worker(error),
                        });
                        continue;
                    }
                },
                () = cancellation.wait() => return,
            };
            if let DiscoveryPublicationPreparation::Prepared(prepared) = &prepared {
                self.stamp_cache.insert(
                    interface,
                    CachedDiscoveryStamp {
                        advertisement_hash: *prepared.advertisement_hash().as_bytes(),
                        stamp: *prepared.stamp(),
                    },
                );
            }
            let prepared = match prepared {
                DiscoveryPublicationPreparation::Prepared(prepared) => prepared,
                DiscoveryPublicationPreparation::Cancelled => return,
                DiscoveryPublicationPreparation::EncodeFailed(error) => {
                    report(TokioDiscoveryPublicationEvent::PreparationFailed {
                        interface,
                        failure: TokioDiscoveryPublicationPreparationFailure::Encode(error),
                    });
                    continue;
                }
                DiscoveryPublicationPreparation::InvalidReachableOn { value } => {
                    report(TokioDiscoveryPublicationEvent::PreparationFailed {
                        interface,
                        failure: TokioDiscoveryPublicationPreparationFailure::InvalidReachableOn {
                            value,
                        },
                    });
                    continue;
                }
                DiscoveryPublicationPreparation::EntropyFailed(error) => {
                    report(TokioDiscoveryPublicationEvent::PreparationFailed {
                        interface,
                        failure: TokioDiscoveryPublicationPreparationFailure::Entropy(error),
                    });
                    continue;
                }
                DiscoveryPublicationPreparation::AppDataTooLong { required, maximum } => {
                    report(TokioDiscoveryPublicationEvent::PreparationFailed {
                        interface,
                        failure: TokioDiscoveryPublicationPreparationFailure::AppDataTooLong {
                            required,
                            maximum,
                        },
                    });
                    continue;
                }
            };
            report(TokioDiscoveryPublicationEvent::Prepared {
                interface,
                stamp_value: prepared.stamp_value(),
                stamp_attempts: prepared.stamp_attempts(),
                cache_hit: prepared.stamp_attempts() == 0,
            });
            let encryption_entropy = match prepared.security() {
                DiscoveryPublicationSecurity::Plaintext => None,
                DiscoveryPublicationSecurity::NetworkEncrypted => {
                    let mut entropy = [0u8; X25519SecretKey::LEN + ENCRYPTION_IV_LEN];
                    if let Err(error) = getrandom::getrandom(&mut entropy) {
                        report(TokioDiscoveryPublicationEvent::FramingFailed {
                            interface,
                            failure: TokioDiscoveryPublicationFramingFailure::Entropy(error),
                        });
                        continue;
                    }
                    Some(entropy)
                }
            };
            let framed = frame_discovery_publication(&prepared, |plaintext| {
                let Some(identity) = self.network_identity else {
                    return Err(prns_core::interface_discovery::DiscoveryPublicationEncryptionError::NetworkIdentityUnavailable);
                };
                let Some(entropy) = encryption_entropy else {
                    return Err(prns_core::interface_discovery::DiscoveryPublicationEncryptionError::NetworkIdentityUnavailable);
                };
                let mut secret = [0u8; X25519SecretKey::LEN];
                secret.copy_from_slice(&entropy[..X25519SecretKey::LEN]);
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                iv.copy_from_slice(&entropy[X25519SecretKey::LEN..]);
                let secret = X25519SecretKey::new(secret);
                let mut ciphertext =
                    vec![0; ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN + sealed_len(plaintext.len())];
                let written = identity
                    .encrypt(&secret, &iv, plaintext, &mut ciphertext)
                    .map_err(
                    prns_core::interface_discovery::DiscoveryPublicationEncryptionError::Identity,
                )?;
                ciphertext.truncate(written);
                Ok(ciphertext)
            });
            let framed = match framed {
                Ok(framed) => framed,
                Err(error) => {
                    report(TokioDiscoveryPublicationEvent::FramingFailed {
                        interface,
                        failure: TokioDiscoveryPublicationFramingFailure::Frame(error),
                    });
                    continue;
                }
            };
            let app_data_bytes = framed.len();
            let app_data = match AnnounceAppDataBytes::from_slice(&framed) {
                Ok(app_data) => app_data,
                Err(()) => {
                    report(TokioDiscoveryPublicationEvent::FramingFailed {
                        interface,
                        failure: TokioDiscoveryPublicationFramingFailure::AppDataTooLong {
                            actual: app_data_bytes,
                            maximum: MAX_ANNOUNCE_APP_DATA_LEN,
                        },
                    });
                    continue;
                }
            };
            let announced = tokio::select! {
                announced = send(app_data) => announced,
                () = cancellation.wait() => return,
            };
            match announced {
                Ok(()) => report(TokioDiscoveryPublicationEvent::Announced {
                    interface,
                    app_data_bytes,
                }),
                Err(failure) => {
                    report(TokioDiscoveryPublicationEvent::AnnounceFailed { interface, failure })
                }
            }
        }
    }
}

pub struct RunningTokioInterfaceDiscoveryPublisher {
    cancellation: PublicationCancellation,
    task: JoinHandle<()>,
}

impl RunningTokioInterfaceDiscoveryPublisher {
    pub async fn shutdown(mut self) -> Result<(), JoinError> {
        self.cancellation.cancel();
        (&mut self.task).await
    }
}

impl Drop for RunningTokioInterfaceDiscoveryPublisher {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
struct PublicationCancellation {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl PublicationCancellation {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    async fn wait(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests;
