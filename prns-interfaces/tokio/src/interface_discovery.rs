use std::collections::BTreeMap;
use std::net::IpAddr;
use std::string::String;
use std::time::Duration;

use prns_core::identity::in_memory::InMemoryNodeIdentity;
use prns_core::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use prns_core::interface_discovery::{
    discovery_destination_hash, ingest_discovery_announce, plan_discovered_connections,
    ActiveDiscoveredInterface, DiscoveredConnectionAccess, DiscoveredConnectionEndpointId,
    DiscoveredConnectionHealth, DiscoveredConnectionKind, DiscoveredConnectionPlan,
    DiscoveredConnectionRegistrationError, DiscoveredConnectionRegistry,
    DiscoveredConnectionSelection, DiscoveredConnectionState, DiscoveredConnectionTransition,
    DiscoveredInterfaceId, DiscoveryCatalog, DiscoveryCatalogUpdate, DiscoveryDecryptionError,
    DiscoveryIntake, DiscoveryNotApplicable, DiscoveryRecord, DiscoveryRejection,
    InterfaceDiscoveryPolicy,
};
use prns_core::interfaces::ifac::{IfacContext, IfacSize};
use prns_core::interfaces::{BitrateBps, InterfaceId, InterfaceStatus, ReportsStatus};
use prns_core::reactor::interface_seam::Interface;
use prns_core::reactor::Host;
use prns_core::routing::announce::AnnounceObservation;
use prns_core::units::{HopCount, InstantMillis};
use prns_core::wire::DestinationHash;
use prns_runtime::reactor::impls::tokio_reactor::{TokioHost, TokioInterfaceStatus};
use prns_runtime::runtime::{AttachedInterface, TokioPrnsHandle};
use tokio::sync::mpsc::{self, error::TrySendError, Receiver, Sender};

use crate::backbone::client::BackboneClientInterface;
use crate::tcp::client::TcpClientInterface;

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
    enabled: bool,
    observations: Sender<OwnedAnnounceObservation>,
}

impl TokioDiscoveryIngress {
    pub fn observe(&self, observation: AnnounceObservation<'_>) -> DiscoveryIngressOutcome {
        if !self.enabled {
            return DiscoveryIngressOutcome::Disabled;
        }
        if observation.is_path_response
            || observation.destination
                != discovery_destination_hash(&observation.announced_identity)
        {
            return DiscoveryIngressOutcome::NotDiscovery;
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
    policy: InterfaceDiscoveryPolicy,
    network_identity: Option<InMemoryNodeIdentity>,
    catalog: DiscoveryCatalog,
    registry: DiscoveredConnectionRegistry,
    occupied_by_other_interfaces: Vec<DiscoveredConnectionEndpointId>,
    statuses: BTreeMap<InterfaceId, TokioInterfaceStatus>,
    observations: Receiver<OwnedAnnounceObservation>,
}

impl TokioInterfaceDiscovery {
    pub fn new(
        policy: InterfaceDiscoveryPolicy,
        network_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
    ) -> (Self, TokioDiscoveryIngress) {
        let enabled = policy.enabled_policy().is_some();
        let (observations_tx, observations) = mpsc::channel(OBSERVATION_QUEUE_DEPTH);
        (
            Self {
                policy,
                network_identity: network_identity
                    .as_deref()
                    .map(InMemoryNodeIdentity::from_secret_key_bytes),
                catalog: DiscoveryCatalog::new(),
                registry: DiscoveredConnectionRegistry::new(),
                occupied_by_other_interfaces: Vec::new(),
                statuses: BTreeMap::new(),
                observations,
            },
            TokioDiscoveryIngress {
                enabled,
                observations: observations_tx,
            },
        )
    }

    pub fn seed_catalog(&mut self, catalog: DiscoveryCatalog) {
        self.catalog = catalog;
    }

    pub fn reserve_endpoint(&mut self, host: &str, port: u16) {
        let endpoint = DiscoveredConnectionEndpointId::for_endpoint(host, port);
        if !self.occupied_by_other_interfaces.contains(&endpoint) {
            self.occupied_by_other_interfaces.push(endpoint);
        }
    }

    pub fn catalog(&self) -> &DiscoveryCatalog {
        &self.catalog
    }

    pub async fn run(
        mut self,
        handle: TokioPrnsHandle,
        clock: TokioHost,
        mut report: impl for<'a> FnMut(TokioDiscoveryEvent<'a>) + Send,
    ) {
        let now = clock.now();
        self.attach_selection(
            &handle,
            DiscoveredConnectionSelection::Startup,
            now,
            &mut report,
        );
        let mut monitor = tokio::time::interval(MONITOR_INTERVAL);
        monitor.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        monitor.tick().await;
        loop {
            tokio::select! {
                observation = self.observations.recv() => {
                    let Some(observation) = observation else {
                        return;
                    };
                    if let Some(discovery) = self.ingest_observation(observation, &mut report) {
                        self.attach_selection(
                            &handle,
                            DiscoveredConnectionSelection::NewlyObserved(discovery),
                            clock.now(),
                            &mut report,
                        );
                    }
                }
                _ = monitor.tick() => {
                    self.monitor_connections(&handle, clock.now(), &mut report);
                }
            }
        }
    }

    fn ingest_observation(
        &mut self,
        observation: OwnedAnnounceObservation,
        report: &mut impl for<'a> FnMut(TokioDiscoveryEvent<'a>),
    ) -> Option<DiscoveredInterfaceId> {
        let intake =
            ingest_discovery_announce(&self.policy, observation.borrowed(), |ciphertext| {
                let Some(identity) = &self.network_identity else {
                    return Err(DiscoveryDecryptionError::NetworkIdentityUnavailable);
                };
                let mut plaintext = vec![0; ciphertext.len()];
                let written = identity
                    .decrypt(ciphertext, &mut plaintext)
                    .map_err(DiscoveryDecryptionError::Identity)?;
                plaintext.truncate(written);
                Ok(plaintext)
            });
        match intake {
            DiscoveryIntake::NotApplicable(reason) => {
                report(TokioDiscoveryEvent::IntakeNotApplicable(reason));
                None
            }
            DiscoveryIntake::Rejected(rejection) => {
                report(TokioDiscoveryEvent::IntakeRejected(&rejection));
                None
            }
            DiscoveryIntake::Discovered(interface) => {
                let id = interface.id;
                let update = self.catalog.observe(*interface);
                if let Some(record) = self.catalog.get(id) {
                    report(TokioDiscoveryEvent::CatalogUpdated { update, record });
                }
                if matches!(update, DiscoveryCatalogUpdate::IgnoredOutOfOrder { .. }) {
                    None
                } else {
                    Some(id)
                }
            }
        }
    }

    fn monitor_connections(
        &mut self,
        handle: &TokioPrnsHandle,
        now: InstantMillis,
        report: &mut impl for<'a> FnMut(TokioDiscoveryEvent<'a>),
    ) {
        for record in self.catalog.remove_expired(now) {
            report(TokioDiscoveryEvent::CatalogExpired(&record));
        }
        self.attach_selection(handle, DiscoveredConnectionSelection::Refill, now, report);
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
        for (interface, health) in health {
            match self.registry.observe_health(interface, health, now) {
                DiscoveredConnectionTransition::Untracked { .. }
                | DiscoveredConnectionTransition::Unchanged => {}
                DiscoveredConnectionTransition::Disconnected { since, .. } => {
                    if let Some(active) = self.registry.get(interface) {
                        report(TokioDiscoveryEvent::ConnectionDisconnected {
                            discovery: active.discovery_id(),
                            interface,
                            since,
                        });
                    }
                }
                DiscoveredConnectionTransition::Reconnected { .. } => {
                    if let Some(active) = self.registry.get(interface) {
                        report(TokioDiscoveryEvent::ConnectionReconnected {
                            discovery: active.discovery_id(),
                            interface,
                        });
                    }
                }
                DiscoveredConnectionTransition::Detach(detached) => {
                    self.statuses.remove(&interface);
                    handle.remove_interface(interface);
                    report(TokioDiscoveryEvent::ConnectionDetached {
                        discovery: detached.discovery_id(),
                        interface,
                    });
                }
            }
        }
    }

    fn attach_selection(
        &mut self,
        handle: &TokioPrnsHandle,
        selection: DiscoveredConnectionSelection,
        now: InstantMillis,
        report: &mut impl for<'a> FnMut(TokioDiscoveryEvent<'a>),
    ) {
        let plans = plan_discovered_connections(
            &self.catalog,
            &self.policy,
            selection,
            now,
            DiscoveredConnectionState::new(&self.registry, &self.occupied_by_other_interfaces),
        );
        for plan in plans {
            match attach_discovered(handle, &plan) {
                Ok(attached) => {
                    let active = ActiveDiscoveredInterface::new(
                        plan.discovery_id(),
                        plan.endpoint_id(),
                        attached.interface,
                    );
                    if let Err(error) = self.registry.register(active) {
                        handle.remove_interface(attached.interface);
                        report(TokioDiscoveryEvent::ConnectionAttachFailed {
                            plan: &plan,
                            failure: DiscoveredConnectionFailure::Registry(error),
                        });
                        continue;
                    }
                    self.statuses.insert(attached.interface, attached.status);
                    report(TokioDiscoveryEvent::ConnectionAttached {
                        plan: &plan,
                        interface: attached.interface,
                    });
                }
                Err(failure) => report(TokioDiscoveryEvent::ConnectionAttachFailed {
                    plan: &plan,
                    failure,
                }),
            }
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
            let attached = attach_with_access(handle, interface, plan.access())?;
            let _ = handle.set_interface_name(attached.id(), plan.name());
            Ok(AttachedDiscoveredInterface {
                interface: attached.id(),
                status,
            })
        }
        DiscoveredConnectionKind::TcpClient => {
            let interface =
                TcpClientInterface::new(target, AUTOCONNECT_BITRATE, RECONNECT_INTERVAL);
            let status = interface.status();
            let attached = attach_with_access(handle, interface, plan.access())?;
            let _ = handle.set_interface_name(attached.id(), plan.name());
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
    access: &DiscoveredConnectionAccess,
) -> Result<AttachedInterface, DiscoveredConnectionFailure>
where
    I: Interface + ReportsStatus + Send + 'static,
{
    match access {
        DiscoveredConnectionAccess::Open => Ok(handle.add_interface(interface)),
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
            Ok(handle.add_interface_with_ifac_name(interface, ifac, network_name.clone()))
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
