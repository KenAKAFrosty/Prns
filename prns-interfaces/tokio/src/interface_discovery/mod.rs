use std::collections::BTreeMap;
use std::net::IpAddr;
use std::string::String;
use std::time::Duration;

use prns_core::identity::in_memory::InMemoryNodeIdentity;
use prns_core::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use prns_core::interface_discovery::{
    DiscoveredConnectionAccess, DiscoveredConnectionHealth, DiscoveredConnectionKind,
    DiscoveredConnectionPlan, DiscoveredConnectionRegistrationError, DiscoveredInterfaceId,
    DiscoveryCatalog, DiscoveryCatalogStoreError, DiscoveryCatalogUpdate, DiscoveryCoordinator,
    DiscoveryCoordinatorAction, DiscoveryCoordinatorEvent, DiscoveryCoordinatorOutput,
    DiscoveryDecryptionError, DiscoveryEndpointReservationError, DiscoveryIngressEligibility,
    DiscoveryIngressFilter, DiscoveryNotApplicable, DiscoveryRecord, DiscoveryRejection,
    InterfaceDiscoveryPolicy,
};
use prns_core::interfaces::ifac::{IfacContext, IfacSize};
use prns_core::interfaces::{BitrateBps, InterfaceId, InterfaceStatus, ReportsStatus};
use prns_core::routing::announce::AnnounceObservation;
use prns_core::units::{HopCount, InstantMillis};
use prns_core::wire::DestinationHash;
use prns_runtime::reactor::impls::tokio_reactor::{TokioHost, TokioInterfaceStatus};
use prns_runtime::reactor::interface_seam::Interface;
use prns_runtime::reactor::Host;
use prns_runtime::runtime::{AttachedInterface, InterfaceAttachmentMetadata, TokioPrnsHandle};
use tokio::sync::mpsc::{self, error::TrySendError, Receiver, Sender};

use crate::backbone::client::BackboneClientInterface;
use crate::tcp::client::TcpClientInterface;

mod publication;

pub use publication::{
    RunningTokioInterfaceDiscoveryPublisher, TokioDiscoveryPublicationEvent,
    TokioDiscoveryPublicationFramingFailure, TokioDiscoveryPublicationPreparationFailure,
    TokioDiscoveryPublisherConstructionError, TokioInterfaceDiscoveryPublisher,
    DISCOVERY_PUBLICATION_JOB_INTERVAL,
};

const OBSERVATION_QUEUE_DEPTH: usize = 64;
const MONITOR_INTERVAL: Duration = Duration::from_secs(5);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
const AUTOCONNECT_BITRATE: BitrateBps = BitrateBps::guess(5_000_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryIngressOutcome {
    Disabled,
    NotDiscovery,
    Queued,
    QueueFull,
    Closed,
}

#[derive(Clone)]
pub struct TokioDiscoveryIngress {
    filter: DiscoveryIngressFilter,
    observations: Sender<OwnedAnnounceObservation>,
}

impl TokioDiscoveryIngress {
    pub fn observe(&self, observation: AnnounceObservation<'_>) -> DiscoveryIngressOutcome {
        match self.filter.classify(&observation) {
            DiscoveryIngressEligibility::Disabled => return DiscoveryIngressOutcome::Disabled,
            DiscoveryIngressEligibility::NotDiscovery => {
                return DiscoveryIngressOutcome::NotDiscovery;
            }
            DiscoveryIngressEligibility::Candidate => {}
        }
        match self
            .observations
            .try_send(OwnedAnnounceObservation::from_borrowed(observation))
        {
            Ok(()) => DiscoveryIngressOutcome::Queued,
            Err(TrySendError::Full(_)) => DiscoveryIngressOutcome::QueueFull,
            Err(TrySendError::Closed(_)) => DiscoveryIngressOutcome::Closed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveredConnectionFailure {
    InvalidPublishedIfac,
    Registry(DiscoveredConnectionRegistrationError),
}

pub enum TokioDiscoveryEvent<'a> {
    IntakeNotApplicable(DiscoveryNotApplicable),
    IntakeRejected(&'a DiscoveryRejection),
    CatalogStoreRejected(DiscoveryCatalogStoreError),
    CatalogUpdated {
        update: DiscoveryCatalogUpdate,
        record: &'a DiscoveryRecord,
    },
    CatalogExpired(&'a DiscoveryRecord),
    ConnectionAttached {
        plan: &'a DiscoveredConnectionPlan,
        interface: InterfaceId,
    },
    ConnectionAttachFailed {
        plan: &'a DiscoveredConnectionPlan,
        failure: DiscoveredConnectionFailure,
    },
    ConnectionDisconnected {
        discovery: DiscoveredInterfaceId,
        interface: InterfaceId,
        since: InstantMillis,
    },
    ConnectionReconnected {
        discovery: DiscoveredInterfaceId,
        interface: InterfaceId,
    },
    ConnectionDetached {
        discovery: DiscoveredInterfaceId,
        interface: InterfaceId,
    },
}

pub struct TokioInterfaceDiscovery {
    coordinator: DiscoveryCoordinator,
    network_identity: Option<InMemoryNodeIdentity>,
    statuses: BTreeMap<InterfaceId, TokioInterfaceStatus>,
    observations: Receiver<OwnedAnnounceObservation>,
}

impl TokioInterfaceDiscovery {
    pub fn new(
        policy: InterfaceDiscoveryPolicy,
        network_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
    ) -> (Self, TokioDiscoveryIngress) {
        let coordinator = DiscoveryCoordinator::new(policy);
        let filter = coordinator.ingress_filter();
        let (observations_tx, observations) = mpsc::channel(OBSERVATION_QUEUE_DEPTH);
        (
            Self {
                coordinator,
                network_identity: network_identity
                    .as_deref()
                    .map(InMemoryNodeIdentity::from_secret_key_bytes),
                statuses: BTreeMap::new(),
                observations,
            },
            TokioDiscoveryIngress {
                filter,
                observations: observations_tx,
            },
        )
    }

    pub fn seed_catalog(&mut self, catalog: DiscoveryCatalog) {
        self.coordinator.seed_catalog(catalog);
    }

    pub fn reserve_endpoint(
        &mut self,
        host: &str,
        port: u16,
    ) -> Result<(), DiscoveryEndpointReservationError> {
        self.coordinator
            .reserve_network_endpoint(host, port)
            .map(|_| ())
    }

    pub fn catalog(&self) -> &DiscoveryCatalog {
        self.coordinator.catalog()
    }

    pub async fn run(
        mut self,
        handle: TokioPrnsHandle,
        clock: TokioHost,
        mut report: impl for<'a> FnMut(TokioDiscoveryEvent<'a>) + Send,
    ) {
        let outputs = self.coordinator.startup(clock.now());
        self.process_outputs(&handle, outputs, &mut report);
        let mut monitor = tokio::time::interval(MONITOR_INTERVAL);
        monitor.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        monitor.tick().await;
        loop {
            tokio::select! {
                observation = self.observations.recv() => {
                    let Some(observation) = observation else {
                        return;
                    };
                    let outputs = self.ingest_observation(observation, clock.now());
                    self.process_outputs(&handle, outputs, &mut report);
                }
                _ = monitor.tick() => {
                    let outputs = self.maintenance_outputs(clock.now());
                    self.process_outputs(&handle, outputs, &mut report);
                }
            }
        }
    }

    fn ingest_observation(
        &mut self,
        observation: OwnedAnnounceObservation,
        now: InstantMillis,
    ) -> Vec<DiscoveryCoordinatorOutput> {
        let network_identity = &self.network_identity;
        self.coordinator
            .observe_announce(observation.borrowed(), now, |ciphertext| {
                let Some(identity) = network_identity else {
                    return Err(DiscoveryDecryptionError::NetworkIdentityUnavailable);
                };
                let mut plaintext = vec![0; ciphertext.len()];
                let written = identity
                    .decrypt(ciphertext, &mut plaintext)
                    .map_err(DiscoveryDecryptionError::Identity)?;
                plaintext.truncate(written);
                Ok(plaintext)
            })
    }

    fn maintenance_outputs(&mut self, now: InstantMillis) -> Vec<DiscoveryCoordinatorOutput> {
        let health = self
            .statuses
            .iter()
            .map(|(interface, status)| {
                let health = if status.connection().is_online() {
                    DiscoveredConnectionHealth::Online
                } else {
                    DiscoveredConnectionHealth::Offline
                };
                (*interface, health)
            })
            .collect::<Vec<_>>();
        self.coordinator.maintain(now, health)
    }

    fn process_outputs(
        &mut self,
        handle: &TokioPrnsHandle,
        outputs: Vec<DiscoveryCoordinatorOutput>,
        report: &mut impl for<'a> FnMut(TokioDiscoveryEvent<'a>),
    ) {
        for output in outputs {
            match output {
                DiscoveryCoordinatorOutput::Event(event) => {
                    self.report_coordinator_event(&event, report);
                }
                DiscoveryCoordinatorOutput::Action(DiscoveryCoordinatorAction::Attach(plan)) => {
                    match attach_discovered(handle, &plan) {
                        Ok(attached) => match self
                            .coordinator
                            .attachment_succeeded(plan, attached.interface)
                        {
                            Ok(event) => {
                                self.statuses.insert(attached.interface, attached.status);
                                self.report_coordinator_event(&event, report);
                            }
                            Err(registration) => {
                                handle.remove_interface(attached.interface);
                                let registration = *registration;
                                let failure =
                                    DiscoveredConnectionFailure::Registry(registration.error());
                                let plan = registration.into_plan();
                                report(TokioDiscoveryEvent::ConnectionAttachFailed {
                                    plan: &plan,
                                    failure,
                                });
                            }
                        },
                        Err(failure) => report(TokioDiscoveryEvent::ConnectionAttachFailed {
                            plan: &plan,
                            failure,
                        }),
                    }
                }
                DiscoveryCoordinatorOutput::Action(DiscoveryCoordinatorAction::Detach {
                    interface,
                }) => {
                    self.statuses.remove(&interface);
                    handle.remove_interface(interface);
                }
            }
        }
    }

    fn report_coordinator_event(
        &self,
        event: &DiscoveryCoordinatorEvent,
        report: &mut impl for<'a> FnMut(TokioDiscoveryEvent<'a>),
    ) {
        match event {
            DiscoveryCoordinatorEvent::IntakeNotApplicable(reason) => {
                report(TokioDiscoveryEvent::IntakeNotApplicable(*reason));
            }
            DiscoveryCoordinatorEvent::IntakeRejected(rejection) => {
                report(TokioDiscoveryEvent::IntakeRejected(rejection));
            }
            DiscoveryCoordinatorEvent::CatalogStoreRejected(error) => {
                report(TokioDiscoveryEvent::CatalogStoreRejected(*error));
            }
            DiscoveryCoordinatorEvent::CatalogUpdated(update) => {
                let Some(record) = self.coordinator.catalog().get(update.id()) else {
                    return;
                };
                report(TokioDiscoveryEvent::CatalogUpdated {
                    update: *update,
                    record,
                });
            }
            DiscoveryCoordinatorEvent::CatalogExpired(record) => {
                report(TokioDiscoveryEvent::CatalogExpired(record));
            }
            DiscoveryCoordinatorEvent::ConnectionAttached { plan, interface } => {
                report(TokioDiscoveryEvent::ConnectionAttached {
                    plan,
                    interface: *interface,
                });
            }
            DiscoveryCoordinatorEvent::ConnectionDisconnected {
                discovery,
                interface,
                since,
            } => report(TokioDiscoveryEvent::ConnectionDisconnected {
                discovery: *discovery,
                interface: *interface,
                since: *since,
            }),
            DiscoveryCoordinatorEvent::ConnectionReconnected {
                discovery,
                interface,
            } => report(TokioDiscoveryEvent::ConnectionReconnected {
                discovery: *discovery,
                interface: *interface,
            }),
            DiscoveryCoordinatorEvent::ConnectionDetached {
                discovery,
                interface,
            } => report(TokioDiscoveryEvent::ConnectionDetached {
                discovery: *discovery,
                interface: *interface,
            }),
        }
    }
}

struct AttachedDiscoveredInterface {
    interface: InterfaceId,
    status: TokioInterfaceStatus,
}

fn attach_discovered(
    handle: &TokioPrnsHandle,
    plan: &DiscoveredConnectionPlan,
) -> Result<AttachedDiscoveredInterface, DiscoveredConnectionFailure> {
    let target = dial_target(plan.endpoint().host(), plan.endpoint().port());
    match plan.connection_kind() {
        DiscoveredConnectionKind::BackboneClient => {
            let interface =
                BackboneClientInterface::new(target, AUTOCONNECT_BITRATE, RECONNECT_INTERVAL);
            let status = interface.status();
            let attached = attach_with_access(handle, interface, plan)?;
            Ok(AttachedDiscoveredInterface {
                interface: attached.id(),
                status,
            })
        }
        DiscoveredConnectionKind::TcpClient => {
            let interface =
                TcpClientInterface::new(target, AUTOCONNECT_BITRATE, RECONNECT_INTERVAL);
            let status = interface.status();
            let attached = attach_with_access(handle, interface, plan)?;
            Ok(AttachedDiscoveredInterface {
                interface: attached.id(),
                status,
            })
        }
    }
}

fn attach_with_access<I>(
    handle: &TokioPrnsHandle,
    interface: I,
    plan: &DiscoveredConnectionPlan,
) -> Result<AttachedInterface, DiscoveredConnectionFailure>
where
    I: Interface + ReportsStatus + Send + 'static,
{
    let metadata = InterfaceAttachmentMetadata {
        name: Some(String::from(plan.name())),
        origin: plan.origin().kind(),
    };
    match plan.access() {
        DiscoveredConnectionAccess::Open => {
            Ok(handle.add_interface_with_metadata(interface, metadata))
        }
        DiscoveredConnectionAccess::PublishedIfac {
            network_name,
            passphrase,
        } => {
            let Some(ifac) = IfacContext::derive(
                network_name.as_deref(),
                passphrase.as_deref(),
                IfacSize::WIDE,
            ) else {
                return Err(DiscoveredConnectionFailure::InvalidPublishedIfac);
            };
            Ok(handle.add_interface_with_metadata_and_ifac_name(
                interface,
                metadata,
                ifac,
                network_name.clone(),
            ))
        }
    }
}

fn dial_target(host: &str, port: u16) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]:{port}"),
        Ok(IpAddr::V4(_)) | Err(_) => format!("{host}:{port}"),
    }
}

struct OwnedAnnounceObservation {
    destination: DestinationHash,
    announced_identity: IdentityHash,
    hops: HopCount,
    source_interface: InterfaceId,
    arrived_at: InstantMillis,
    app_data: Vec<u8>,
    is_path_response: bool,
}

impl OwnedAnnounceObservation {
    fn from_borrowed(observation: AnnounceObservation<'_>) -> Self {
        Self {
            destination: observation.destination,
            announced_identity: observation.announced_identity,
            hops: observation.hops,
            source_interface: observation.source_interface,
            arrived_at: observation.arrived_at,
            app_data: observation.app_data.to_vec(),
            is_path_response: observation.is_path_response,
        }
    }

    fn borrowed(&self) -> AnnounceObservation<'_> {
        AnnounceObservation {
            destination: self.destination,
            announced_identity: self.announced_identity,
            hops: self.hops,
            source_interface: self.source_interface,
            arrived_at: self.arrived_at,
            app_data: &self.app_data,
            is_path_response: self.is_path_response,
        }
    }
}

#[cfg(test)]
mod tests;
