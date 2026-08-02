use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::crypto::sha256;
use crate::interfaces::{InterfaceGravity, InterfaceId};
use crate::storage::TablePushError;
use crate::units::{DurationMillis, InstantMillis};
use crate::wire::TransportId;

use super::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveredConnectionTable,
    DiscoveredEndpointSet, DiscoveredInterfaceId, DiscoveredInterfaceStatus, DiscoveryCatalog,
    DiscoveryCatalogTable, DiscoveryProvenance, HeapDiscoveredConnectionTable,
    InterfaceDiscoveryPolicy, InterfaceOrigin, StampValue,
};

pub const DISCOVERED_INTERFACE_DETACH_AFTER: DurationMillis = DurationMillis(12_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoveredConnectionEndpointId([u8; 32]);

impl DiscoveredConnectionEndpointId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn for_endpoint(host: &str, port: u16) -> Self {
        let specifier = format!("{host}:{port}");
        Self(sha256(specifier.as_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredConnectionEndpoint {
    host: String,
    port: u16,
}

impl DiscoveredConnectionEndpoint {
    fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }

    pub fn id(&self) -> DiscoveredConnectionEndpointId {
        DiscoveredConnectionEndpointId::for_endpoint(&self.host, self.port)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveredConnectionAccess {
    Open,
    PublishedIfac {
        network_name: Option<String>,
        passphrase: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveredConnectionKind {
    BackboneClient,
    TcpClient,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredConnectionPlan {
    discovery_id: DiscoveredInterfaceId,
    advertised_type: AdvertisedInterfaceType,
    connection_kind: DiscoveredConnectionKind,
    name: String,
    endpoint: DiscoveredConnectionEndpoint,
    transport_id: TransportId,
    access: DiscoveredConnectionAccess,
    provenance: DiscoveryProvenance,
    stamp_value: StampValue,
    gravity: InterfaceGravity,
    announces_to_internal: bool,
}

impl DiscoveredConnectionPlan {
    pub const fn discovery_id(&self) -> DiscoveredInterfaceId {
        self.discovery_id
    }

    pub const fn advertised_type(&self) -> AdvertisedInterfaceType {
        self.advertised_type
    }

    pub const fn connection_kind(&self) -> DiscoveredConnectionKind {
        self.connection_kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn endpoint(&self) -> &DiscoveredConnectionEndpoint {
        &self.endpoint
    }

    pub fn endpoint_id(&self) -> DiscoveredConnectionEndpointId {
        self.endpoint.id()
    }

    pub const fn transport_id(&self) -> TransportId {
        self.transport_id
    }

    pub const fn access(&self) -> &DiscoveredConnectionAccess {
        &self.access
    }

    pub const fn provenance(&self) -> DiscoveryProvenance {
        self.provenance
    }

    pub const fn origin(&self) -> InterfaceOrigin {
        InterfaceOrigin::Discovered(self.provenance)
    }

    pub const fn stamp_value(&self) -> StampValue {
        self.stamp_value
    }

    pub const fn gravity(&self) -> InterfaceGravity {
        self.gravity
    }

    pub const fn announces_to_internal(&self) -> bool {
        self.announces_to_internal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiscoveredConnectionSelection {
    Startup,
    Refill,
    NewlyObserved(DiscoveredInterfaceId),
}

pub(super) fn plan_discovered_connections<C, A, E>(
    catalog: &DiscoveryCatalog<C>,
    policy: &InterfaceDiscoveryPolicy,
    selection: DiscoveredConnectionSelection,
    now: InstantMillis,
    active_discovered: &DiscoveredConnectionRegistry<A>,
    occupied_by_other_interfaces: &E,
) -> Vec<DiscoveredConnectionPlan>
where
    C: DiscoveryCatalogTable,
    A: DiscoveredConnectionTable,
    E: DiscoveredEndpointSet,
{
    let Some(enabled) = policy.enabled_policy() else {
        return Vec::new();
    };
    let Some(maximum) = enabled.auto_connect().maximum() else {
        return Vec::new();
    };
    let remaining = maximum.saturating_sub(active_discovered.len());
    let available_slots = match selection {
        DiscoveredConnectionSelection::Startup => remaining,
        DiscoveredConnectionSelection::Refill => usize::from(remaining > maximum / 4),
        DiscoveredConnectionSelection::NewlyObserved(_) => remaining.min(1),
    };
    if available_slots == 0 {
        return Vec::new();
    }

    let mut occupied = active_discovered
        .endpoint_ids()
        .chain(occupied_by_other_interfaces.endpoints())
        .collect::<Vec<_>>();
    let records = match selection {
        DiscoveredConnectionSelection::NewlyObserved(id) => {
            catalog.get(id).into_iter().collect::<Vec<_>>()
        }
        DiscoveredConnectionSelection::Startup | DiscoveredConnectionSelection::Refill => {
            catalog.ranked_records(now)
        }
    };
    let mut plans = Vec::new();
    for record in records {
        let status = record.status(now);
        let status_is_eligible = match selection {
            DiscoveredConnectionSelection::Startup => {
                !matches!(status, DiscoveredInterfaceStatus::Expired)
            }
            DiscoveredConnectionSelection::Refill
            | DiscoveredConnectionSelection::NewlyObserved(_) => {
                matches!(status, DiscoveredInterfaceStatus::Available)
            }
        };
        if !status_is_eligible {
            continue;
        }
        let Some(plan) = connection_plan(
            record.interface(),
            enabled.auto_connect_gravity(),
            enabled.auto_connect_announces_to_internal(),
        ) else {
            continue;
        };
        let endpoint = plan.endpoint_id();
        if occupied.contains(&endpoint) {
            continue;
        }
        occupied.push(endpoint);
        plans.push(plan);
        if plans.len() == available_slots {
            break;
        }
    }
    plans
}

fn connection_plan(
    interface: &super::DiscoveredInterface,
    gravity: InterfaceGravity,
    announces_to_internal: bool,
) -> Option<DiscoveredConnectionPlan> {
    let transport_id = match interface.advertisement.transport {
        AdvertisedTransport::Enabled(transport_id) => transport_id,
        AdvertisedTransport::Disabled(_) => return None,
    };
    let advertised_type = interface.advertisement.interface_type;
    let connection_kind = match advertised_type {
        AdvertisedInterfaceType::Backbone => DiscoveredConnectionKind::BackboneClient,
        AdvertisedInterfaceType::TcpServer => DiscoveredConnectionKind::TcpClient,
        AdvertisedInterfaceType::TcpClient
        | AdvertisedInterfaceType::I2p
        | AdvertisedInterfaceType::RNode
        | AdvertisedInterfaceType::Weave
        | AdvertisedInterfaceType::Kiss => return None,
    };
    let AdvertisementDetails::Reachable { host, port } = &interface.advertisement.details else {
        return None;
    };
    let access = match &interface.advertisement.published_ifac {
        Some(ifac) if ifac.network_name.is_some() || ifac.passphrase.is_some() => {
            DiscoveredConnectionAccess::PublishedIfac {
                network_name: ifac.network_name.clone(),
                passphrase: ifac.passphrase.clone(),
            }
        }
        Some(_) | None => DiscoveredConnectionAccess::Open,
    };
    Some(DiscoveredConnectionPlan {
        discovery_id: interface.id,
        advertised_type,
        connection_kind,
        name: interface.name.clone(),
        endpoint: DiscoveredConnectionEndpoint::new(host.clone(), *port),
        transport_id,
        access,
        provenance: interface.provenance,
        stamp_value: interface.stamp_value,
        gravity,
        announces_to_internal,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct ActiveDiscoveredInterface {
    discovery_id: DiscoveredInterfaceId,
    endpoint_id: DiscoveredConnectionEndpointId,
    interface_id: InterfaceId,
    disconnected_since: Option<InstantMillis>,
}

impl ActiveDiscoveredInterface {
    pub(super) const fn new(
        discovery_id: DiscoveredInterfaceId,
        endpoint_id: DiscoveredConnectionEndpointId,
        interface_id: InterfaceId,
    ) -> Self {
        Self {
            discovery_id,
            endpoint_id,
            interface_id,
            disconnected_since: None,
        }
    }

    pub(super) const fn discovery_id(&self) -> DiscoveredInterfaceId {
        self.discovery_id
    }

    pub const fn endpoint_id(&self) -> DiscoveredConnectionEndpointId {
        self.endpoint_id
    }

    pub const fn interface_id(&self) -> InterfaceId {
        self.interface_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveredConnectionRegistrationError {
    InterfaceAlreadyTracked {
        interface: InterfaceId,
    },
    EndpointAlreadyTracked {
        endpoint: DiscoveredConnectionEndpointId,
    },
    CapacityReached {
        interface: InterfaceId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveredConnectionHealth {
    Online,
    Offline,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DiscoveredConnectionTransition {
    Untracked {
        interface: InterfaceId,
    },
    Unchanged,
    Disconnected {
        discovery: DiscoveredInterfaceId,
        interface: InterfaceId,
        since: InstantMillis,
    },
    Reconnected {
        discovery: DiscoveredInterfaceId,
        interface: InterfaceId,
    },
    Detach(ActiveDiscoveredInterface),
}

#[derive(Debug)]
pub(super) struct DiscoveredConnectionRegistry<
    T: DiscoveredConnectionTable = HeapDiscoveredConnectionTable,
> {
    active: T,
}

impl<T: DiscoveredConnectionTable> Default for DiscoveredConnectionRegistry<T> {
    fn default() -> Self {
        Self {
            active: T::default(),
        }
    }
}

#[cfg(test)]
impl DiscoveredConnectionRegistry<HeapDiscoveredConnectionTable> {
    pub(super) fn new() -> Self {
        Self::default()
    }
}

impl<T: DiscoveredConnectionTable> DiscoveredConnectionRegistry<T> {
    pub(super) fn with_table(active: T) -> Self {
        Self { active }
    }

    pub(super) fn register(
        &mut self,
        interface: ActiveDiscoveredInterface,
    ) -> Result<(), DiscoveredConnectionRegistrationError> {
        let interface_id = interface.interface_id;
        let endpoint_id = interface.endpoint_id;
        if self.active.contains_interface(interface_id) {
            return Err(
                DiscoveredConnectionRegistrationError::InterfaceAlreadyTracked {
                    interface: interface_id,
                },
            );
        }
        if self.active.contains_endpoint(endpoint_id) {
            return Err(
                DiscoveredConnectionRegistrationError::EndpointAlreadyTracked {
                    endpoint: endpoint_id,
                },
            );
        }
        let previous = self
            .active
            .try_insert(interface)
            .map_err(|TablePushError::TableFull| {
                DiscoveredConnectionRegistrationError::CapacityReached {
                    interface: interface_id,
                }
            })?;
        debug_assert!(previous.is_none());
        Ok(())
    }

    pub(super) fn observe_health(
        &mut self,
        interface: InterfaceId,
        health: DiscoveredConnectionHealth,
        now: InstantMillis,
    ) -> DiscoveredConnectionTransition {
        let Some(active) = self.active.get_mut(interface) else {
            return DiscoveredConnectionTransition::Untracked { interface };
        };
        match health {
            DiscoveredConnectionHealth::Online => match active.disconnected_since.take() {
                Some(_) => DiscoveredConnectionTransition::Reconnected {
                    discovery: active.discovery_id,
                    interface,
                },
                None => DiscoveredConnectionTransition::Unchanged,
            },
            DiscoveredConnectionHealth::Offline => match active.disconnected_since {
                None => {
                    active.disconnected_since = Some(now);
                    DiscoveredConnectionTransition::Disconnected {
                        discovery: active.discovery_id,
                        interface,
                        since: now,
                    }
                }
                Some(since)
                    if now.duration_since(since).0 >= DISCOVERED_INTERFACE_DETACH_AFTER.0 =>
                {
                    match self.active.remove(interface) {
                        Some(detached) => DiscoveredConnectionTransition::Detach(detached),
                        None => DiscoveredConnectionTransition::Untracked { interface },
                    }
                }
                Some(_) => DiscoveredConnectionTransition::Unchanged,
            },
        }
    }

    pub(super) fn endpoint_ids(&self) -> impl Iterator<Item = DiscoveredConnectionEndpointId> + '_ {
        self.active
            .connections()
            .map(ActiveDiscoveredInterface::endpoint_id)
    }

    pub(super) fn len(&self) -> usize {
        self.active.len()
    }
}

#[cfg(test)]
mod tests;
