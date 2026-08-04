use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::announce::AnnounceObservation;
use crate::storage::TablePushError;
use crate::units::InstantMillis;

use super::autoconnect::{
    plan_discovered_connections, ActiveDiscoveredInterface, DiscoveredConnectionEndpointId,
    DiscoveredConnectionRegistrationError, DiscoveredConnectionRegistry,
    DiscoveredConnectionSelection, DiscoveredConnectionTransition,
};
use super::{
    discovery_destination_hash, DiscoveredConnectionHealth, DiscoveredConnectionPlan,
    DiscoveredEndpointSet, DiscoveryCatalog, DiscoveryCatalogStoreError, DiscoveryCatalogUpdate,
    DiscoveryDecryptionError, DiscoveryIdentityRole, DiscoveryIntake, DiscoveryNotApplicable,
    DiscoveryRecord, DiscoveryRejection, GrowableInterfaceDiscoveryStorage,
    InterfaceDiscoveryPolicy, InterfaceDiscoveryStorage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryIngressEligibility {
    Disabled,
    NotDiscovery,
    Candidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryIngressFilter {
    enabled: bool,
}

impl DiscoveryIngressFilter {
    pub const fn from_policy(policy: &InterfaceDiscoveryPolicy) -> Self {
        Self {
            enabled: policy.enabled_policy().is_some(),
        }
    }

    pub fn classify(&self, observation: &AnnounceObservation<'_>) -> DiscoveryIngressEligibility {
        if !self.enabled {
            return DiscoveryIngressEligibility::Disabled;
        }
        if observation.is_path_response
            || observation.destination
                != discovery_destination_hash(&observation.announced_identity)
        {
            return DiscoveryIngressEligibility::NotDiscovery;
        }
        DiscoveryIngressEligibility::Candidate
    }
}

#[derive(Debug, PartialEq)]
pub enum DiscoveryCoordinatorEvent {
    IntakeNotApplicable(DiscoveryNotApplicable),
    IntakeRejected(DiscoveryRejection),
    CatalogStoreRejected(DiscoveryCatalogStoreError),
    CatalogUpdated(DiscoveryCatalogUpdate),
    CatalogExpired(DiscoveryRecord),
    CatalogBlackholed(DiscoveryRecord),
    ConnectionAttached {
        plan: DiscoveredConnectionPlan,
        interface: InterfaceId,
    },
    ConnectionDisconnected {
        discovery: super::DiscoveredInterfaceId,
        interface: InterfaceId,
        since: InstantMillis,
    },
    ConnectionReconnected {
        discovery: super::DiscoveredInterfaceId,
        interface: InterfaceId,
    },
    ConnectionDetached {
        discovery: super::DiscoveredInterfaceId,
        interface: InterfaceId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryEndpointReservation {
    Added,
    AlreadyReserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryEndpointReservationError {
    CapacityReached(DiscoveredConnectionEndpointId),
}

impl core::fmt::Display for DiscoveryEndpointReservationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CapacityReached(endpoint) => write!(
                formatter,
                "interface discovery has no capacity to reserve endpoint {:?}",
                endpoint.as_bytes()
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DiscoveryEndpointReservationError {}

#[derive(Debug, PartialEq)]
pub enum DiscoveryCoordinatorAction {
    Attach(DiscoveredConnectionPlan),
    Detach { interface: InterfaceId },
}

#[derive(Debug, PartialEq)]
pub enum DiscoveryCoordinatorOutput {
    Event(DiscoveryCoordinatorEvent),
    Action(DiscoveryCoordinatorAction),
}

#[derive(Debug, PartialEq)]
pub struct DiscoveryAttachmentRegistrationFailure {
    plan: DiscoveredConnectionPlan,
    error: DiscoveredConnectionRegistrationError,
}

impl DiscoveryAttachmentRegistrationFailure {
    pub const fn plan(&self) -> &DiscoveredConnectionPlan {
        &self.plan
    }

    pub const fn error(&self) -> DiscoveredConnectionRegistrationError {
        self.error
    }

    pub fn into_plan(self) -> DiscoveredConnectionPlan {
        self.plan
    }
}

pub struct DiscoveryCoordinator<S: InterfaceDiscoveryStorage = GrowableInterfaceDiscoveryStorage> {
    policy: InterfaceDiscoveryPolicy,
    validation_cache: S::ValidationCache,
    catalog: DiscoveryCatalog<S::Catalog>,
    connections: DiscoveredConnectionRegistry<S::Connections>,
    reserved_endpoints: S::ReservedEndpoints,
}

impl DiscoveryCoordinator<GrowableInterfaceDiscoveryStorage> {
    pub fn new(policy: InterfaceDiscoveryPolicy) -> Self {
        Self::with_storage(policy)
    }
}

impl<S: InterfaceDiscoveryStorage> DiscoveryCoordinator<S> {
    pub fn with_storage(policy: InterfaceDiscoveryPolicy) -> Self {
        Self {
            policy,
            validation_cache: S::ValidationCache::default(),
            catalog: DiscoveryCatalog::with_table(S::Catalog::default()),
            connections: DiscoveredConnectionRegistry::with_table(S::Connections::default()),
            reserved_endpoints: S::ReservedEndpoints::default(),
        }
    }

    pub const fn ingress_filter(&self) -> DiscoveryIngressFilter {
        DiscoveryIngressFilter::from_policy(&self.policy)
    }

    pub fn seed_catalog(&mut self, mut catalog: DiscoveryCatalog<S::Catalog>) {
        if let Some(policy) = self.policy.enabled_policy() {
            catalog.remove_below_stamp_cost(policy.required_stamp_cost());
        }
        self.catalog = catalog;
    }

    pub fn reserve_endpoint(
        &mut self,
        endpoint: DiscoveredConnectionEndpointId,
    ) -> Result<DiscoveryEndpointReservation, DiscoveryEndpointReservationError> {
        self.reserved_endpoints
            .try_insert(endpoint)
            .map(|inserted| {
                if inserted {
                    DiscoveryEndpointReservation::Added
                } else {
                    DiscoveryEndpointReservation::AlreadyReserved
                }
            })
            .map_err(|TablePushError::TableFull| {
                DiscoveryEndpointReservationError::CapacityReached(endpoint)
            })
    }

    pub fn reserve_network_endpoint(
        &mut self,
        host: &str,
        port: u16,
    ) -> Result<DiscoveryEndpointReservation, DiscoveryEndpointReservationError> {
        self.reserve_endpoint(DiscoveredConnectionEndpointId::for_endpoint(host, port))
    }

    pub const fn catalog(&self) -> &DiscoveryCatalog<S::Catalog> {
        &self.catalog
    }

    pub fn startup(&self, now: InstantMillis) -> Vec<DiscoveryCoordinatorOutput> {
        self.connection_actions(DiscoveredConnectionSelection::Startup, now)
    }

    pub fn observe_announce(
        &mut self,
        observation: AnnounceObservation<'_>,
        now: InstantMillis,
        decrypt: impl FnOnce(&[u8]) -> Result<Vec<u8>, DiscoveryDecryptionError>,
    ) -> Vec<DiscoveryCoordinatorOutput> {
        self.observe_announce_with_blackholes(observation, now, decrypt, &[])
    }

    pub fn observe_announce_with_blackholes(
        &mut self,
        observation: AnnounceObservation<'_>,
        now: InstantMillis,
        decrypt: impl FnOnce(&[u8]) -> Result<Vec<u8>, DiscoveryDecryptionError>,
        blackholed: &[IdentityHash],
    ) -> Vec<DiscoveryCoordinatorOutput> {
        if blackholed.contains(&observation.announced_identity) {
            return vec![DiscoveryCoordinatorOutput::Event(
                DiscoveryCoordinatorEvent::IntakeRejected(DiscoveryRejection::BlackholedIdentity {
                    identity: observation.announced_identity,
                    role: DiscoveryIdentityRole::Announcing,
                }),
            )];
        }
        match super::intake::ingest_discovery_announce_cached(
            &self.policy,
            observation,
            decrypt,
            &mut self.validation_cache,
        ) {
            DiscoveryIntake::NotApplicable(reason) => vec![DiscoveryCoordinatorOutput::Event(
                DiscoveryCoordinatorEvent::IntakeNotApplicable(reason),
            )],
            DiscoveryIntake::Rejected(rejection) => vec![DiscoveryCoordinatorOutput::Event(
                DiscoveryCoordinatorEvent::IntakeRejected(rejection),
            )],
            DiscoveryIntake::Discovered(interface) => {
                let advertised_transport =
                    IdentityHash::new(*interface.advertisement.transport.transport_id().as_bytes());
                if blackholed.contains(&advertised_transport) {
                    return vec![DiscoveryCoordinatorOutput::Event(
                        DiscoveryCoordinatorEvent::IntakeRejected(
                            DiscoveryRejection::BlackholedIdentity {
                                identity: advertised_transport,
                                role: DiscoveryIdentityRole::AdvertisedTransport,
                            },
                        ),
                    )];
                }
                let discovery = interface.id;
                let update = match self.catalog.observe(*interface) {
                    Ok(update) => update,
                    Err(error) => {
                        return vec![DiscoveryCoordinatorOutput::Event(
                            DiscoveryCoordinatorEvent::CatalogStoreRejected(error),
                        )];
                    }
                };
                let mut outputs = vec![DiscoveryCoordinatorOutput::Event(
                    DiscoveryCoordinatorEvent::CatalogUpdated(update),
                )];
                if !matches!(update, DiscoveryCatalogUpdate::IgnoredOutOfOrder { .. }) {
                    outputs.extend(self.connection_actions(
                        DiscoveredConnectionSelection::NewlyObserved(discovery),
                        now,
                    ));
                }
                outputs
            }
        }
    }

    pub fn reconcile_blackholes(
        &mut self,
        blackholed: &[IdentityHash],
    ) -> Vec<DiscoveryCoordinatorOutput> {
        let removed = self.catalog.remove_blackholed(blackholed);
        let discoveries = removed.iter().map(DiscoveryRecord::id).collect::<Vec<_>>();
        let detached = self.connections.remove_discoveries(&discoveries);
        let mut outputs = removed
            .into_iter()
            .map(|record| {
                DiscoveryCoordinatorOutput::Event(DiscoveryCoordinatorEvent::CatalogBlackholed(
                    record,
                ))
            })
            .collect::<Vec<_>>();
        for active in detached {
            let discovery = active.discovery_id();
            let interface = active.interface_id();
            outputs.push(DiscoveryCoordinatorOutput::Action(
                DiscoveryCoordinatorAction::Detach { interface },
            ));
            outputs.push(DiscoveryCoordinatorOutput::Event(
                DiscoveryCoordinatorEvent::ConnectionDetached {
                    discovery,
                    interface,
                },
            ));
        }
        outputs
    }

    pub fn maintain(
        &mut self,
        now: InstantMillis,
        health: impl IntoIterator<Item = (InterfaceId, DiscoveredConnectionHealth)>,
    ) -> Vec<DiscoveryCoordinatorOutput> {
        let mut outputs = self
            .catalog
            .remove_expired(now)
            .into_iter()
            .map(|record| {
                DiscoveryCoordinatorOutput::Event(DiscoveryCoordinatorEvent::CatalogExpired(record))
            })
            .collect::<Vec<_>>();
        outputs.extend(self.connection_actions(DiscoveredConnectionSelection::Refill, now));
        for (interface, health) in health {
            match self.connections.observe_health(interface, health, now) {
                DiscoveredConnectionTransition::Untracked { .. }
                | DiscoveredConnectionTransition::Unchanged => {}
                DiscoveredConnectionTransition::Disconnected {
                    discovery,
                    interface,
                    since,
                } => outputs.push(DiscoveryCoordinatorOutput::Event(
                    DiscoveryCoordinatorEvent::ConnectionDisconnected {
                        discovery,
                        interface,
                        since,
                    },
                )),
                DiscoveredConnectionTransition::Reconnected {
                    discovery,
                    interface,
                } => outputs.push(DiscoveryCoordinatorOutput::Event(
                    DiscoveryCoordinatorEvent::ConnectionReconnected {
                        discovery,
                        interface,
                    },
                )),
                DiscoveredConnectionTransition::Detach(detached) => {
                    outputs.push(DiscoveryCoordinatorOutput::Action(
                        DiscoveryCoordinatorAction::Detach { interface },
                    ));
                    outputs.push(DiscoveryCoordinatorOutput::Event(
                        DiscoveryCoordinatorEvent::ConnectionDetached {
                            discovery: detached.discovery_id(),
                            interface,
                        },
                    ));
                }
            }
        }
        outputs
    }

    pub fn attachment_succeeded(
        &mut self,
        plan: DiscoveredConnectionPlan,
        interface: InterfaceId,
    ) -> Result<DiscoveryCoordinatorEvent, Box<DiscoveryAttachmentRegistrationFailure>> {
        let active =
            ActiveDiscoveredInterface::new(plan.discovery_id(), plan.endpoint_id(), interface);
        if let Err(error) = self.connections.register(active) {
            return Err(Box::new(DiscoveryAttachmentRegistrationFailure {
                plan,
                error,
            }));
        }
        Ok(DiscoveryCoordinatorEvent::ConnectionAttached { plan, interface })
    }

    fn connection_actions(
        &self,
        selection: DiscoveredConnectionSelection,
        now: InstantMillis,
    ) -> Vec<DiscoveryCoordinatorOutput> {
        plan_discovered_connections(
            &self.catalog,
            &self.policy,
            selection,
            now,
            &self.connections,
            &self.reserved_endpoints,
        )
        .into_iter()
        .map(|plan| DiscoveryCoordinatorOutput::Action(DiscoveryCoordinatorAction::Attach(plan)))
        .collect()
    }
}

#[cfg(test)]
mod tests;
