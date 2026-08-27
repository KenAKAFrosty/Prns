use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::num::NonZeroU8;
use std::time::{Duration, Instant};

use mdns_sd::{
    IfKind, Receiver, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo,
};

use prns_core::interfaces::local_network::{
    is_local_address, local_address_scope, LocalAddressScope,
};
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::wifi_auto::{
    AdvertisementInsertion, AdvertisementRemoval, CandidateInsertion, CandidateInsertionError,
    DiscoveryEndpoint, DiscoveryServiceName, DiscoveryServiceNameError, DiscoverySnapshot,
    DiscoveryTransport, DiscoveryVersion, DiscoveryVersionError, EphemeralDiscoveryInstanceName,
    ServiceAdvertisement,
};

use crate::network_device::AutoWifiDevicePolicy;

use super::publication_absence::{
    EmptyPublicationDisposition, PublicationAbsence, PublicationPresence,
};
use super::{
    DiscoveryLifecycleError, DiscoveryParticipation, ServiceDiscovery, ServiceDiscoveryPublisher,
};

const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
pub(crate) const DISCOVERY_CAPACITY: NonZeroU8 = contract::DEFAULT_DISCOVERY_SERVICE_CAPACITY;

/// Starts the native DNS-SD provider behind a bounded AutoWifi discovery channel.
///
/// The provider remains dormant until the associated AutoWifi runtime owns the
/// shared TCP listener and reports [`DiscoveryParticipation::Central`].
pub fn native_service_discovery(auto_wifi_device_policy: AutoWifiDevicePolicy) -> ServiceDiscovery {
    let (service_discovery, service_discovery_publisher) =
        ServiceDiscovery::channel(DISCOVERY_CAPACITY);
    spawn_service_discovery(auto_wifi_device_policy, service_discovery_publisher);
    service_discovery
}

fn spawn_service_discovery(
    auto_wifi_device_policy: AutoWifiDevicePolicy,
    service_discovery_publisher: ServiceDiscoveryPublisher,
) {
    tokio::spawn(run_service_discovery(
        auto_wifi_device_policy,
        service_discovery_publisher,
    ));
}

async fn run_service_discovery(
    auto_wifi_device_policy: AutoWifiDevicePolicy,
    mut service_discovery_publisher: ServiceDiscoveryPublisher,
) -> NativeDiscoveryExit {
    loop {
        if service_discovery_publisher.participation() != DiscoveryParticipation::Central {
            service_discovery_publisher.clear_snapshot();
            match service_discovery_publisher
                .wait_for_participation(DiscoveryParticipation::Central)
                .await
            {
                Ok(()) => {}
                Err(DiscoveryLifecycleError::Closed) => {
                    return NativeDiscoveryExit::RuntimeDropped;
                }
            }
        }

        let central_session_follow_up =
            match run_central_session(&auto_wifi_device_policy, &mut service_discovery_publisher)
                .await
            {
                Ok(central_session_end) => CentralSessionFollowUp::from(central_session_end),
                Err(mdns_error) => {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: mDNS discovery unavailable: {mdns_error}"
                    );
                    CentralSessionFollowUp::RetryBackend
                }
            };

        service_discovery_publisher.clear_snapshot();
        match central_session_follow_up {
            CentralSessionFollowUp::ReevaluateParticipation => continue,
            CentralSessionFollowUp::RetryBackend => {}
            CentralSessionFollowUp::ExitDiscovery => {
                return NativeDiscoveryExit::RuntimeDropped;
            }
        }
        if service_discovery_publisher.participation() != DiscoveryParticipation::Central {
            continue;
        }

        let restart_trigger = tokio::select! {
            () = tokio::time::sleep(RETRY_INTERVAL) => DiscoveryRestartTrigger::RetryElapsed,
            participation_change = service_discovery_publisher.wait_for_participation_change() => {
                match participation_change {
                    Ok(
                        DiscoveryParticipation::Inactive
                        | DiscoveryParticipation::Satellite
                        | DiscoveryParticipation::Central,
                    ) => DiscoveryRestartTrigger::ParticipationChanged,
                    Err(DiscoveryLifecycleError::Closed) => {
                        DiscoveryRestartTrigger::RuntimeDropped
                    }
                }
            }
        };
        match restart_trigger {
            DiscoveryRestartTrigger::RetryElapsed
            | DiscoveryRestartTrigger::ParticipationChanged => {}
            DiscoveryRestartTrigger::RuntimeDropped => return NativeDiscoveryExit::RuntimeDropped,
        }
    }
}

async fn run_central_session(
    auto_wifi_device_policy: &AutoWifiDevicePolicy,
    service_discovery_publisher: &mut ServiceDiscoveryPublisher,
) -> Result<CentralDiscoverySessionEnd, MdnsDiscoveryError> {
    let central_publications = CentralPublications::fresh()?;
    let mut native_mdns_session = NativeMdnsSession::start()?;
    let tcp_service_events = native_mdns_session.browse(DiscoveryTransport::Tcp)?;
    let udp_service_events = native_mdns_session.browse(DiscoveryTransport::Udp)?;
    let mut eligible_ip_addresses = BTreeSet::new();
    let mut discovery_snapshot = DiscoverySnapshot::new(service_discovery_publisher.capacity());
    let mut reconciliation_interval = tokio::time::interval(RECONCILE_INTERVAL);
    reconciliation_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let central_session_end = loop {
        tokio::select! {
            participation_change = service_discovery_publisher.wait_for_participation_change() => {
                match participation_change {
                    Ok(DiscoveryParticipation::Central) => {}
                    Ok(DiscoveryParticipation::Inactive) => {
                        break CentralDiscoverySessionEnd::BecameInactive;
                    }
                    Ok(DiscoveryParticipation::Satellite) => {
                        break CentralDiscoverySessionEnd::BecameSatellite;
                    }
                    Err(DiscoveryLifecycleError::Closed) => {
                        break CentralDiscoverySessionEnd::RuntimeDropped;
                    }
                }
            }
            _ = reconciliation_interval.tick() => {
                let current_eligible_ip_addresses =
                    collect_eligible_ip_addresses(auto_wifi_device_policy)?;
                match InterfaceReconciliation::between(
                    &eligible_ip_addresses,
                    &current_eligible_ip_addresses,
                ) {
                    InterfaceReconciliation::Unchanged => {}
                    InterfaceReconciliation::Changed(interface_changes) => {
                        interface_changes.apply(native_mdns_session.daemon())?;
                        eligible_ip_addresses = current_eligible_ip_addresses;
                        discovery_snapshot =
                            DiscoverySnapshot::new(service_discovery_publisher.capacity());
                        let _ = service_discovery_publisher
                            .replace_snapshot(discovery_snapshot.clone());
                    }
                }
                native_mdns_session.reconcile_publications(
                    &central_publications,
                    &eligible_ip_addresses,
                )?;
            }
            received_service_event = next_service_event(
                &tcp_service_events,
                &udp_service_events,
            ) => {
                let service_event = match received_service_event {
                    Ok(service_event) => service_event,
                    Err(_backend_stopped) => break CentralDiscoverySessionEnd::BackendStopped,
                };
                match apply_service_event(
                    &mut discovery_snapshot,
                    &service_event,
                    &central_publications,
                ) {
                    ServiceEventOutcome::SnapshotChanged => {
                        let _ = service_discovery_publisher
                            .replace_snapshot(discovery_snapshot.clone());
                    }
                    ServiceEventOutcome::SnapshotUnchanged
                    | ServiceEventOutcome::RejectedAtCapacity => {}
                }
            }
        }
    };
    Ok(central_session_end)
}

async fn next_service_event(
    tcp_service_events: &Receiver<ServiceEvent>,
    udp_service_events: &Receiver<ServiceEvent>,
) -> Result<ServiceEvent, MdnsBackendStopped> {
    tokio::select! {
        received_service_event = tcp_service_events.recv_async() => {
            received_service_event.map_err(|_backend_stopped| MdnsBackendStopped)
        }
        received_service_event = udp_service_events.recv_async() => {
            received_service_event.map_err(|_backend_stopped| MdnsBackendStopped)
        }
    }
}

fn apply_service_event(
    discovery_snapshot: &mut DiscoverySnapshot,
    service_event: &ServiceEvent,
    central_publications: &CentralPublications,
) -> ServiceEventOutcome {
    match service_event {
        ServiceEvent::ServiceResolved(resolved_service) => {
            apply_resolved_service(discovery_snapshot, resolved_service, central_publications)
        }
        ServiceEvent::ServiceRemoved(removed_service_type, removed_service_fullname) => {
            match classify_service_type(removed_service_type) {
                ServiceTypeClassification::Supported(discovery_transport) => {
                    match DiscoveryServiceName::from_fullname(
                        removed_service_fullname,
                        discovery_transport,
                    ) {
                        Ok(discovery_service_name)
                            if discovery_snapshot.remove(&discovery_service_name)
                                == AdvertisementRemoval::Removed =>
                        {
                            ServiceEventOutcome::SnapshotChanged
                        }
                        Ok(_unknown_service_name) => ServiceEventOutcome::SnapshotUnchanged,
                        Err(
                            DiscoveryServiceNameError::Empty
                            | DiscoveryServiceNameError::TooLong { .. }
                            | DiscoveryServiceNameError::WrongServiceType { .. },
                        ) => ServiceEventOutcome::SnapshotUnchanged,
                    }
                }
                ServiceTypeClassification::Unsupported => ServiceEventOutcome::SnapshotUnchanged,
            }
        }
        ServiceEvent::SearchStarted(_)
        | ServiceEvent::SearchStopped(_)
        | ServiceEvent::ServiceFound(_, _)
        | _ => ServiceEventOutcome::SnapshotUnchanged,
    }
}

fn apply_resolved_service(
    discovery_snapshot: &mut DiscoverySnapshot,
    resolved_service: &ResolvedService,
    central_publications: &CentralPublications,
) -> ServiceEventOutcome {
    let discovery_transport = match classify_service_type(&resolved_service.ty_domain) {
        ServiceTypeClassification::Supported(discovery_transport) => discovery_transport,
        ServiceTypeClassification::Unsupported => return ServiceEventOutcome::SnapshotUnchanged,
    };
    let service_advertisement =
        match build_service_advertisement(resolved_service, central_publications) {
            Ok(service_advertisement) => service_advertisement,
            Err(
                ServiceAdvertisementRejection::WrongServiceType
                | ServiceAdvertisementRejection::OwnService
                | ServiceAdvertisementRejection::InvalidServiceName(_),
            ) => return ServiceEventOutcome::SnapshotUnchanged,
            Err(
                ServiceAdvertisementRejection::InvalidVersion(_)
                | ServiceAdvertisementRejection::CandidateTransport(_)
                | ServiceAdvertisementRejection::NoEligibleEndpoints,
            ) => {
                let Ok(discovery_service_name) = DiscoveryServiceName::from_fullname(
                    resolved_service.get_fullname(),
                    discovery_transport,
                ) else {
                    return ServiceEventOutcome::SnapshotUnchanged;
                };
                return match discovery_snapshot.remove(&discovery_service_name) {
                    AdvertisementRemoval::Removed => ServiceEventOutcome::SnapshotChanged,
                    AdvertisementRemoval::NotPresent => ServiceEventOutcome::SnapshotUnchanged,
                };
            }
        };
    if discovery_snapshot.get(service_advertisement.service()) == Some(&service_advertisement) {
        return ServiceEventOutcome::SnapshotUnchanged;
    }
    match discovery_snapshot.insert(service_advertisement) {
        AdvertisementInsertion::Inserted | AdvertisementInsertion::Replaced => {
            ServiceEventOutcome::SnapshotChanged
        }
        AdvertisementInsertion::AtCapacity => ServiceEventOutcome::RejectedAtCapacity,
    }
}

fn collect_eligible_ip_addresses(
    auto_wifi_device_policy: &AutoWifiDevicePolicy,
) -> Result<BTreeSet<IpAddr>, MdnsDiscoveryError> {
    let network_interfaces = if_addrs::get_if_addrs().map_err(MdnsDiscoveryError::Interfaces)?;
    let mut eligible_ip_addresses = BTreeSet::new();
    for network_interface in network_interfaces {
        if !auto_wifi_device_policy.allows(&network_interface.name, network_interface.is_loopback())
        {
            continue;
        }
        let ip_address = network_interface.ip();
        if !is_local_address(ip_address) || ip_address.is_loopback() {
            continue;
        }
        eligible_ip_addresses.insert(ip_address);
    }
    eligible_ip_addresses.extend(
        super::link_local_nics(auto_wifi_device_policy)
            .into_iter()
            .map(|network_interface| IpAddr::V6(network_interface.link_local)),
    );
    Ok(eligible_ip_addresses)
}

fn build_service_advertisement(
    resolved_service: &ResolvedService,
    central_publications: &CentralPublications,
) -> Result<ServiceAdvertisement, ServiceAdvertisementRejection> {
    let discovery_transport = match classify_service_type(&resolved_service.ty_domain) {
        ServiceTypeClassification::Supported(discovery_transport) => discovery_transport,
        ServiceTypeClassification::Unsupported => {
            return Err(ServiceAdvertisementRejection::WrongServiceType);
        }
    };
    if central_publications.is_own_service(resolved_service.get_fullname(), discovery_transport) {
        return Err(ServiceAdvertisementRejection::OwnService);
    }
    let version_metadata = match resolved_service.get_property_val(contract::TXT_VERSION_KEY) {
        None => None,
        Some(Some(value)) => Some(value),
        Some(None) => Some(&[][..]),
    };
    DiscoveryVersion::parse(version_metadata)
        .map_err(ServiceAdvertisementRejection::InvalidVersion)?;
    let discovery_service_name =
        DiscoveryServiceName::from_fullname(resolved_service.get_fullname(), discovery_transport)
            .map_err(ServiceAdvertisementRejection::InvalidServiceName)?;
    let mut discovery_endpoints = BTreeSet::new();
    for scoped_ip_address in resolved_service.get_addresses() {
        if let Some(discovery_endpoint) = validated_discovery_endpoint(
            scoped_ip_address,
            resolved_service.get_port(),
            discovery_transport,
        ) {
            discovery_endpoints.insert(discovery_endpoint);
        }
    }
    let mut service_advertisement = ServiceAdvertisement::new(discovery_service_name);
    for discovery_endpoint in discovery_endpoints {
        match service_advertisement.insert(discovery_endpoint) {
            Ok(CandidateInsertion::RejectedLowerPriority) => break,
            Ok(
                CandidateInsertion::Inserted
                | CandidateInsertion::AlreadyPresent
                | CandidateInsertion::ReplacedLowerPriority,
            ) => {}
            Err(candidate_error) => {
                return Err(ServiceAdvertisementRejection::CandidateTransport(
                    candidate_error,
                ));
            }
        }
    }
    if service_advertisement.is_empty() {
        return Err(ServiceAdvertisementRejection::NoEligibleEndpoints);
    }
    Ok(service_advertisement)
}

fn validated_discovery_endpoint(
    scoped_ip_address: &ScopedIp,
    service_port: u16,
    discovery_transport: DiscoveryTransport,
) -> Option<DiscoveryEndpoint> {
    let socket_address = match scoped_ip_address {
        ScopedIp::V4(ipv4_address) => {
            SocketAddr::new(IpAddr::V4(*ipv4_address.addr()), service_port)
        }
        ScopedIp::V6(ipv6_address) => {
            let ip_address = *ipv6_address.addr();
            let scope_id = if ip_address.is_unicast_link_local() {
                ipv6_address.scope_id().index
            } else {
                0
            };
            SocketAddr::V6(SocketAddrV6::new(ip_address, service_port, 0, scope_id))
        }
        _ => return None,
    };
    validated_socket_endpoint(socket_address, discovery_transport)
}

fn validated_socket_endpoint(
    socket_address: SocketAddr,
    discovery_transport: DiscoveryTransport,
) -> Option<DiscoveryEndpoint> {
    DiscoveryEndpoint::try_from((discovery_transport, socket_address)).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceTypeClassification {
    Supported(DiscoveryTransport),
    Unsupported,
}

fn classify_service_type(service_type: &str) -> ServiceTypeClassification {
    for discovery_transport in [DiscoveryTransport::Tcp, DiscoveryTransport::Udp] {
        if service_type.eq_ignore_ascii_case(discovery_transport.dns_sd_service_type()) {
            return ServiceTypeClassification::Supported(discovery_transport);
        }
    }
    ServiceTypeClassification::Unsupported
}

struct CentralPublications {
    tcp: EphemeralPublication,
    udp: EphemeralPublication,
}

impl CentralPublications {
    fn fresh() -> Result<Self, MdnsDiscoveryError> {
        let mut tcp_random_bytes = [0u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES];
        getrandom::getrandom(&mut tcp_random_bytes)
            .map_err(MdnsDiscoveryError::RandomnessUnavailable)?;
        let mut udp_random_bytes = [0u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES];
        getrandom::getrandom(&mut udp_random_bytes)
            .map_err(MdnsDiscoveryError::RandomnessUnavailable)?;
        Self::from_random_bytes(tcp_random_bytes, udp_random_bytes)
    }

    fn from_random_bytes(
        tcp_random_bytes: [u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
        udp_random_bytes: [u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
    ) -> Result<Self, MdnsDiscoveryError> {
        Ok(Self {
            tcp: EphemeralPublication::from_random_bytes(
                DiscoveryTransport::Tcp,
                tcp_random_bytes,
            )?,
            udp: EphemeralPublication::from_random_bytes(
                DiscoveryTransport::Udp,
                udp_random_bytes,
            )?,
        })
    }

    fn get(&self, discovery_transport: DiscoveryTransport) -> &EphemeralPublication {
        match discovery_transport {
            DiscoveryTransport::Tcp => &self.tcp,
            DiscoveryTransport::Udp => &self.udp,
        }
    }

    fn iter(&self) -> impl Iterator<Item = &EphemeralPublication> {
        [&self.tcp, &self.udp].into_iter()
    }

    fn is_own_service(
        &self,
        service_fullname: &str,
        discovery_transport: DiscoveryTransport,
    ) -> bool {
        service_fullname.eq_ignore_ascii_case(self.get(discovery_transport).service_name.as_str())
    }
}

struct EphemeralPublication {
    transport: DiscoveryTransport,
    instance_name: EphemeralDiscoveryInstanceName,
    hostname: String,
    service_name: DiscoveryServiceName,
}

impl EphemeralPublication {
    fn from_random_bytes(
        transport: DiscoveryTransport,
        random_bytes: [u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
    ) -> Result<Self, MdnsDiscoveryError> {
        let instance_name = EphemeralDiscoveryInstanceName::from_random_bytes(random_bytes);
        let service_name = DiscoveryServiceName::from_instance(instance_name.as_str(), transport)
            .map_err(MdnsDiscoveryError::GeneratedServiceName)?;
        Ok(Self {
            transport,
            hostname: format!("{instance_name}.{}", contract::DNS_SD_LOCAL_DOMAIN),
            service_name,
            instance_name,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegisteredService {
    service_fullname: String,
    advertised_ip_addresses: BTreeSet<IpAddr>,
}

struct NativeMdnsSession {
    daemon: ServiceDaemon,
    browsed_transports: BTreeSet<DiscoveryTransport>,
    registered_services: BTreeMap<DiscoveryTransport, RegisteredService>,
    publication_absence: PublicationAbsence,
}

impl NativeMdnsSession {
    fn start() -> Result<Self, MdnsDiscoveryError> {
        let native_mdns_session = Self {
            daemon: ServiceDaemon::new().map_err(MdnsDiscoveryError::Mdns)?,
            browsed_transports: BTreeSet::new(),
            registered_services: BTreeMap::new(),
            publication_absence: PublicationAbsence::new(),
        };
        native_mdns_session
            .daemon
            .disable_interface(IfKind::All)
            .map_err(MdnsDiscoveryError::Mdns)?;
        Ok(native_mdns_session)
    }

    const fn daemon(&self) -> &ServiceDaemon {
        &self.daemon
    }

    fn browse(
        &mut self,
        discovery_transport: DiscoveryTransport,
    ) -> Result<Receiver<ServiceEvent>, MdnsDiscoveryError> {
        let service_events = self
            .daemon
            .browse(discovery_transport.dns_sd_service_type())
            .map_err(MdnsDiscoveryError::Mdns)?;
        self.browsed_transports.insert(discovery_transport);
        Ok(service_events)
    }

    fn reconcile_publications(
        &mut self,
        central_publications: &CentralPublications,
        eligible_ip_addresses: &BTreeSet<IpAddr>,
    ) -> Result<(), MdnsDiscoveryError> {
        let observed_at = Instant::now();
        for ephemeral_publication in central_publications.iter() {
            self.reconcile_publication(ephemeral_publication, eligible_ip_addresses, observed_at)?;
        }
        Ok(())
    }

    fn reconcile_publication(
        &mut self,
        ephemeral_publication: &EphemeralPublication,
        eligible_ip_addresses: &BTreeSet<IpAddr>,
        observed_at: Instant,
    ) -> Result<(), MdnsDiscoveryError> {
        let advertised_ip_addresses =
            advertised_ip_addresses(ephemeral_publication.transport, eligible_ip_addresses);
        if advertised_ip_addresses.is_empty() {
            let publication_presence = match self
                .registered_services
                .get(&ephemeral_publication.transport)
            {
                Some(_) => PublicationPresence::Registered,
                None => PublicationPresence::Unregistered,
            };
            match self.publication_absence.observe_empty(
                ephemeral_publication.transport,
                publication_presence,
                observed_at,
            ) {
                EmptyPublicationDisposition::AlreadyAbsent
                | EmptyPublicationDisposition::RetainDuringGrace => {
                    return Ok(());
                }
                EmptyPublicationDisposition::Withdraw => {}
            }
            if let Some(previously_registered_service) = self
                .registered_services
                .remove(&ephemeral_publication.transport)
            {
                let _ = self
                    .daemon
                    .unregister(&previously_registered_service.service_fullname);
            }
            return Ok(());
        }
        self.publication_absence
            .observe_available(ephemeral_publication.transport);

        let desired_service = RegisteredService {
            service_fullname: ephemeral_publication.service_name.as_str().to_owned(),
            advertised_ip_addresses: advertised_ip_addresses.clone(),
        };
        if self
            .registered_services
            .get(&ephemeral_publication.transport)
            == Some(&desired_service)
        {
            return Ok(());
        }
        if let Some(previously_registered_service) = self
            .registered_services
            .remove(&ephemeral_publication.transport)
        {
            let _ = self
                .daemon
                .unregister(&previously_registered_service.service_fullname);
        }

        let service_info = service_info_for(ephemeral_publication, &advertised_ip_addresses)?;
        self.daemon
            .register(service_info)
            .map_err(MdnsDiscoveryError::Mdns)?;
        self.registered_services
            .insert(ephemeral_publication.transport, desired_service);
        Ok(())
    }
}

impl Drop for NativeMdnsSession {
    fn drop(&mut self) {
        for (_discovery_transport, registered_service) in
            std::mem::take(&mut self.registered_services)
        {
            let _ = self.daemon.unregister(&registered_service.service_fullname);
        }
        for discovery_transport in std::mem::take(&mut self.browsed_transports) {
            let _ = self
                .daemon
                .stop_browse(discovery_transport.dns_sd_service_type());
        }
        let _ = self.daemon.shutdown();
    }
}

fn advertised_ip_addresses(
    discovery_transport: DiscoveryTransport,
    eligible_ip_addresses: &BTreeSet<IpAddr>,
) -> BTreeSet<IpAddr> {
    let mut advertised_ip_addresses = eligible_ip_addresses
        .iter()
        .copied()
        .filter(|ip_address| match (discovery_transport, ip_address) {
            (DiscoveryTransport::Tcp, _) => true,
            (DiscoveryTransport::Udp, IpAddr::V6(ipv6_address)) => {
                ipv6_address.is_unicast_link_local()
            }
            (DiscoveryTransport::Udp, IpAddr::V4(_)) => false,
        })
        .collect::<Vec<_>>();
    advertised_ip_addresses
        .sort_by_key(|ip_address| (publication_address_preference(*ip_address), *ip_address));
    advertised_ip_addresses.truncate(usize::from(
        contract::SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY,
    ));
    advertised_ip_addresses.into_iter().collect()
}

fn publication_address_preference(ip_address: IpAddr) -> u8 {
    match ip_address {
        IpAddr::V4(ipv4_address) if ipv4_address.is_private() => 0,
        IpAddr::V6(_) if local_address_scope(ip_address) == Some(LocalAddressScope::Private) => 1,
        IpAddr::V4(_) => 2,
        IpAddr::V6(_) => 3,
    }
}

fn service_info_for(
    ephemeral_publication: &EphemeralPublication,
    advertised_ip_addresses: &BTreeSet<IpAddr>,
) -> Result<ServiceInfo, MdnsDiscoveryError> {
    let txt_properties = [(contract::TXT_VERSION_KEY, contract::TXT_VERSION_VALUE)];
    let advertised_ip_addresses = advertised_ip_addresses.iter().copied().collect::<Vec<_>>();
    let mut service_info = ServiceInfo::new(
        ephemeral_publication.transport.dns_sd_service_type(),
        ephemeral_publication.instance_name.as_str(),
        &ephemeral_publication.hostname,
        advertised_ip_addresses.as_slice(),
        ephemeral_publication.transport.port(),
        &txt_properties[..],
    )
    .map_err(MdnsDiscoveryError::Mdns)?;
    service_info.set_interfaces(
        advertised_ip_addresses
            .iter()
            .copied()
            .map(IfKind::Addr)
            .collect(),
    );
    Ok(service_info)
}

struct MdnsBackendStopped;

enum NativeDiscoveryExit {
    RuntimeDropped,
}

enum CentralDiscoverySessionEnd {
    BecameInactive,
    BecameSatellite,
    RuntimeDropped,
    BackendStopped,
}

#[derive(Debug, PartialEq, Eq)]
enum CentralSessionFollowUp {
    ReevaluateParticipation,
    RetryBackend,
    ExitDiscovery,
}

impl From<CentralDiscoverySessionEnd> for CentralSessionFollowUp {
    fn from(central_session_end: CentralDiscoverySessionEnd) -> Self {
        match central_session_end {
            CentralDiscoverySessionEnd::BecameInactive
            | CentralDiscoverySessionEnd::BecameSatellite => Self::ReevaluateParticipation,
            CentralDiscoverySessionEnd::BackendStopped => Self::RetryBackend,
            CentralDiscoverySessionEnd::RuntimeDropped => Self::ExitDiscovery,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum InterfaceReconciliation {
    Unchanged,
    Changed(InterfaceChanges),
}

impl InterfaceReconciliation {
    fn between(
        previous_ip_addresses: &BTreeSet<IpAddr>,
        current_ip_addresses: &BTreeSet<IpAddr>,
    ) -> Self {
        let removed_ip_addresses: BTreeSet<IpAddr> = previous_ip_addresses
            .difference(current_ip_addresses)
            .copied()
            .collect();
        let added_ip_addresses: BTreeSet<IpAddr> = current_ip_addresses
            .difference(previous_ip_addresses)
            .copied()
            .collect();
        if removed_ip_addresses.is_empty() && added_ip_addresses.is_empty() {
            return Self::Unchanged;
        }
        Self::Changed(InterfaceChanges {
            removed_ip_addresses,
            added_ip_addresses,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct InterfaceChanges {
    removed_ip_addresses: BTreeSet<IpAddr>,
    added_ip_addresses: BTreeSet<IpAddr>,
}

impl InterfaceChanges {
    fn apply(&self, mdns_daemon: &ServiceDaemon) -> Result<(), MdnsDiscoveryError> {
        if !self.removed_ip_addresses.is_empty() {
            mdns_daemon
                .disable_interface(
                    self.removed_ip_addresses
                        .iter()
                        .copied()
                        .map(IfKind::Addr)
                        .collect::<Vec<_>>(),
                )
                .map_err(MdnsDiscoveryError::Mdns)?;
        }
        if !self.added_ip_addresses.is_empty() {
            mdns_daemon
                .enable_interface(
                    self.added_ip_addresses
                        .iter()
                        .copied()
                        .map(IfKind::Addr)
                        .collect::<Vec<_>>(),
                )
                .map_err(MdnsDiscoveryError::Mdns)?;
        }
        Ok(())
    }
}

enum DiscoveryRestartTrigger {
    RetryElapsed,
    ParticipationChanged,
    RuntimeDropped,
}

#[derive(Debug, PartialEq, Eq)]
enum ServiceEventOutcome {
    SnapshotChanged,
    SnapshotUnchanged,
    RejectedAtCapacity,
}

#[derive(Debug, PartialEq, Eq)]
enum ServiceAdvertisementRejection {
    WrongServiceType,
    OwnService,
    InvalidVersion(DiscoveryVersionError),
    InvalidServiceName(DiscoveryServiceNameError),
    CandidateTransport(CandidateInsertionError),
    NoEligibleEndpoints,
}

#[derive(Debug)]
enum MdnsDiscoveryError {
    Interfaces(std::io::Error),
    Mdns(mdns_sd::Error),
    RandomnessUnavailable(getrandom::Error),
    GeneratedServiceName(DiscoveryServiceNameError),
}

impl std::fmt::Display for MdnsDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interfaces(error) => write!(formatter, "enumerating LAN interfaces: {error}"),
            Self::Mdns(error) => write!(formatter, "DNS-SD: {error}"),
            Self::RandomnessUnavailable(error) => {
                write!(
                    formatter,
                    "generating an ephemeral publication name: {error}"
                )
            }
            Self::GeneratedServiceName(error) => {
                write!(formatter, "constructing an ephemeral service name: {error}")
            }
        }
    }
}

impl std::error::Error for MdnsDiscoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Interfaces(error) => Some(error),
            Self::Mdns(error) => Some(error),
            Self::RandomnessUnavailable(_) => None,
            Self::GeneratedServiceName(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip_address_set(ip_addresses: &[&str]) -> BTreeSet<IpAddr> {
        ip_addresses
            .iter()
            .map(|ip_address| ip_address.parse().unwrap())
            .collect()
    }

    fn resolved_service(
        discovery_transport: DiscoveryTransport,
        instance_name: &str,
        ip_addresses: &[IpAddr],
        service_port: u16,
        txt_properties: &[(&str, &str)],
    ) -> ResolvedService {
        ServiceInfo::new(
            discovery_transport.dns_sd_service_type(),
            instance_name,
            &format!("{instance_name}.local."),
            ip_addresses,
            service_port,
            txt_properties,
        )
        .unwrap()
        .as_resolved_service()
    }

    fn central_publications() -> CentralPublications {
        CentralPublications::from_random_bytes(
            [0x11; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
            [0x22; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
        )
        .unwrap()
    }

    #[test]
    fn central_publication_names_are_independent_and_rotate_with_each_session() {
        let first_session = CentralPublications::from_random_bytes(
            [0x11; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
            [0x22; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
        )
        .unwrap();
        let second_session = CentralPublications::from_random_bytes(
            [0x33; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
            [0x44; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
        )
        .unwrap();

        assert_ne!(
            first_session.tcp.instance_name.as_str(),
            first_session.udp.instance_name.as_str()
        );
        assert_ne!(
            first_session.tcp.instance_name.as_str(),
            second_session.tcp.instance_name.as_str()
        );
        assert_ne!(
            first_session.udp.instance_name.as_str(),
            second_session.udp.instance_name.as_str()
        );
        assert!(first_session
            .tcp
            .service_name
            .as_str()
            .ends_with(contract::TCP_DNS_SD_SERVICE_TYPE));
        assert!(first_session
            .udp
            .service_name
            .as_str()
            .ends_with(contract::UDP_DNS_SD_SERVICE_TYPE));
    }

    #[test]
    fn native_publications_use_both_transport_contracts_and_one_address_policy() {
        let central_publications = central_publications();
        let eligible_ip_addresses = ip_address_set(&["192.168.4.8", "fd00::8", "fe80::8"]);
        let tcp_addresses =
            advertised_ip_addresses(DiscoveryTransport::Tcp, &eligible_ip_addresses);
        let udp_addresses =
            advertised_ip_addresses(DiscoveryTransport::Udp, &eligible_ip_addresses);
        let tcp_service_info = service_info_for(&central_publications.tcp, &tcp_addresses).unwrap();
        let udp_service_info = service_info_for(&central_publications.udp, &udp_addresses).unwrap();

        assert_eq!(tcp_addresses, eligible_ip_addresses);
        assert_eq!(udp_addresses, ip_address_set(&["fe80::8"]));
        assert_eq!(
            (
                tcp_service_info.get_type(),
                tcp_service_info.get_port(),
                tcp_service_info.get_property_val_str(contract::TXT_VERSION_KEY),
            ),
            (
                contract::TCP_DNS_SD_SERVICE_TYPE,
                contract::TCP_RENDEZVOUS_PORT,
                Some(contract::TXT_VERSION_VALUE),
            )
        );
        assert_eq!(
            (
                udp_service_info.get_type(),
                udp_service_info.get_port(),
                udp_service_info.get_property_val_str(contract::TXT_VERSION_KEY),
            ),
            (
                contract::UDP_DNS_SD_SERVICE_TYPE,
                contract::UNICAST_DISCOVERY_PORT,
                Some(contract::TXT_VERSION_VALUE),
            )
        );
    }

    #[test]
    fn native_publication_addresses_are_bounded_and_keep_the_best_candidates() {
        let eligible_ip_addresses = ip_address_set(&[
            "192.168.4.18",
            "192.168.4.17",
            "192.168.4.16",
            "192.168.4.15",
            "192.168.4.14",
            "192.168.4.13",
            "192.168.4.12",
            "192.168.4.11",
            "192.168.4.10",
            "fd00::8",
            "fe80::8",
        ]);

        let advertised_ip_addresses =
            advertised_ip_addresses(DiscoveryTransport::Tcp, &eligible_ip_addresses);

        assert_eq!(
            advertised_ip_addresses.len(),
            usize::from(contract::SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY)
        );
        assert!(advertised_ip_addresses.iter().all(|ip_address| {
            matches!(ip_address, IpAddr::V4(ipv4_address) if ipv4_address.is_private())
        }));
        assert!(advertised_ip_addresses.contains(&"192.168.4.10".parse().unwrap()));
        assert!(!advertised_ip_addresses.contains(&"192.168.4.18".parse().unwrap()));
    }

    #[test]
    fn native_endpoint_decoding_preserves_transport_and_ipv6_scope() {
        let tcp_endpoint = validated_socket_endpoint(
            "192.168.4.8:42699".parse().unwrap(),
            DiscoveryTransport::Tcp,
        )
        .unwrap();
        let udp_endpoint = validated_socket_endpoint(
            "[fe80::8%4]:29717".parse().unwrap(),
            DiscoveryTransport::Udp,
        )
        .unwrap();

        assert_eq!(tcp_endpoint.transport(), DiscoveryTransport::Tcp);
        assert_eq!(udp_endpoint.transport(), DiscoveryTransport::Udp);
        assert_eq!(
            udp_endpoint.socket_addr(),
            "[fe80::8%4]:29717".parse().unwrap()
        );
        assert_eq!(
            validated_socket_endpoint("[fe80::8]:29717".parse().unwrap(), DiscoveryTransport::Udp,),
            None
        );
    }

    #[test]
    fn udp_departure_removes_its_record_from_the_combined_snapshot() {
        let central_publications = central_publications();
        let udp_service_name =
            DiscoveryServiceName::from_instance("udp-peer", DiscoveryTransport::Udp).unwrap();
        let mut udp_advertisement = ServiceAdvertisement::new(udp_service_name.clone());
        udp_advertisement
            .insert(DiscoveryEndpoint::udp("[fe80::8%4]:29717".parse().unwrap()).unwrap())
            .unwrap();
        let mut discovery_snapshot = DiscoverySnapshot::new(NonZeroU8::new(1).unwrap());
        assert_eq!(
            discovery_snapshot.insert(udp_advertisement),
            AdvertisementInsertion::Inserted
        );

        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceRemoved(
                    contract::UDP_DNS_SD_SERVICE_TYPE.to_owned(),
                    udp_service_name.as_str().to_owned(),
                ),
                &central_publications,
            ),
            ServiceEventOutcome::SnapshotChanged
        );
        assert!(discovery_snapshot.is_empty());
    }

    #[test]
    fn records_keep_all_valid_candidates_and_reject_our_own_record() {
        let central_publications = central_publications();
        let peer_service = resolved_service(
            DiscoveryTransport::Tcp,
            "prns-cafe0001",
            &["fe80::1".parse().unwrap(), "192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[(contract::TXT_VERSION_KEY, contract::TXT_VERSION_VALUE)],
        );
        let peer_service_advertisement =
            build_service_advertisement(&peer_service, &central_publications)
                .expect("peer is accepted");
        assert_eq!(peer_service_advertisement.endpoints().len(), 1);
        assert_eq!(
            peer_service_advertisement.endpoints()[0].socket_addr(),
            "192.168.4.8:42699".parse().unwrap()
        );

        let local_service = resolved_service(
            DiscoveryTransport::Tcp,
            central_publications.tcp.instance_name.as_str(),
            &["192.168.4.9".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            build_service_advertisement(&local_service, &central_publications),
            Err(ServiceAdvertisementRejection::OwnService)
        );
    }

    #[test]
    fn invalid_endpoints_and_explicit_incompatible_versions_are_rejected() {
        let central_publications = central_publications();
        let public_service = resolved_service(
            DiscoveryTransport::Tcp,
            "prns-deadbeef",
            &["8.8.8.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            build_service_advertisement(&public_service, &central_publications),
            Err(ServiceAdvertisementRejection::NoEligibleEndpoints)
        );

        let incompatible_service = resolved_service(
            DiscoveryTransport::Tcp,
            "prns-version2",
            &["192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[(contract::TXT_VERSION_KEY, "2")],
        );
        assert_eq!(
            build_service_advertisement(&incompatible_service, &central_publications),
            Err(ServiceAdvertisementRejection::InvalidVersion(
                DiscoveryVersionError::Unsupported(2)
            ))
        );
    }

    #[test]
    fn missing_version_is_implicit_v1() {
        let central_publications = central_publications();
        let legacy_service = resolved_service(
            DiscoveryTransport::Tcp,
            "prns-legacy",
            &["192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        build_service_advertisement(&legacy_service, &central_publications)
            .expect("implicit v1 record is accepted");
    }

    #[test]
    fn interface_reconciliation_names_every_address_change() {
        let no_ip_addresses = BTreeSet::new();
        let original_ip_addresses = ip_address_set(&["192.168.4.8", "fd00::8"]);
        let changed_ip_addresses = ip_address_set(&["192.168.4.9", "fd00::8"]);

        assert_eq!(
            [
                InterfaceReconciliation::between(&original_ip_addresses, &original_ip_addresses,),
                InterfaceReconciliation::between(&no_ip_addresses, &original_ip_addresses),
                InterfaceReconciliation::between(&original_ip_addresses, &changed_ip_addresses,),
                InterfaceReconciliation::between(&changed_ip_addresses, &no_ip_addresses),
            ],
            [
                InterfaceReconciliation::Unchanged,
                InterfaceReconciliation::Changed(InterfaceChanges {
                    removed_ip_addresses: BTreeSet::new(),
                    added_ip_addresses: original_ip_addresses.clone(),
                }),
                InterfaceReconciliation::Changed(InterfaceChanges {
                    removed_ip_addresses: ip_address_set(&["192.168.4.8"]),
                    added_ip_addresses: ip_address_set(&["192.168.4.9"]),
                }),
                InterfaceReconciliation::Changed(InterfaceChanges {
                    removed_ip_addresses: changed_ip_addresses,
                    added_ip_addresses: BTreeSet::new(),
                }),
            ]
        );
    }

    #[test]
    fn central_session_completion_has_an_explicit_follow_up() {
        assert_eq!(
            [
                CentralSessionFollowUp::from(CentralDiscoverySessionEnd::BecameInactive),
                CentralSessionFollowUp::from(CentralDiscoverySessionEnd::BecameSatellite),
                CentralSessionFollowUp::from(CentralDiscoverySessionEnd::BackendStopped),
                CentralSessionFollowUp::from(CentralDiscoverySessionEnd::RuntimeDropped),
            ],
            [
                CentralSessionFollowUp::ReevaluateParticipation,
                CentralSessionFollowUp::ReevaluateParticipation,
                CentralSessionFollowUp::RetryBackend,
                CentralSessionFollowUp::ExitDiscovery,
            ]
        );
    }

    #[test]
    fn snapshot_updates_known_services_at_capacity_and_removes_departures() {
        let central_publications = central_publications();
        let first_service = resolved_service(
            DiscoveryTransport::Tcp,
            "first",
            &["192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        let first_service_name = DiscoveryServiceName::from_fullname(
            first_service.get_fullname(),
            DiscoveryTransport::Tcp,
        )
        .unwrap();
        let mut discovery_snapshot = DiscoverySnapshot::new(NonZeroU8::new(1).unwrap());

        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(first_service)),
                &central_publications,
            ),
            ServiceEventOutcome::SnapshotChanged
        );

        let repeated_service = resolved_service(
            DiscoveryTransport::Tcp,
            "first",
            &["192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(repeated_service)),
                &central_publications,
            ),
            ServiceEventOutcome::SnapshotUnchanged
        );

        let overflow_service = resolved_service(
            DiscoveryTransport::Tcp,
            "overflow",
            &["192.168.4.9".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(overflow_service)),
                &central_publications,
            ),
            ServiceEventOutcome::RejectedAtCapacity
        );
        assert_eq!(discovery_snapshot.len(), 1);

        let replacement_service = resolved_service(
            DiscoveryTransport::Tcp,
            "first",
            &["192.168.4.10".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(replacement_service)),
                &central_publications,
            ),
            ServiceEventOutcome::SnapshotChanged
        );
        assert_eq!(
            discovery_snapshot
                .get(&first_service_name)
                .unwrap()
                .endpoints()[0]
                .ip(),
            "192.168.4.10".parse::<IpAddr>().unwrap()
        );

        let incompatible_service = resolved_service(
            DiscoveryTransport::Tcp,
            "first",
            &["192.168.4.10".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[(contract::TXT_VERSION_KEY, "2")],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(incompatible_service)),
                &central_publications,
            ),
            ServiceEventOutcome::SnapshotChanged
        );
        assert!(discovery_snapshot.is_empty());

        let restored_service = resolved_service(
            DiscoveryTransport::Tcp,
            "first",
            &["192.168.4.10".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(restored_service)),
                &central_publications,
            ),
            ServiceEventOutcome::SnapshotChanged
        );

        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceRemoved(
                    contract::TCP_DNS_SD_SERVICE_TYPE.to_owned(),
                    first_service_name.as_str().to_owned(),
                ),
                &central_publications,
            ),
            ServiceEventOutcome::SnapshotChanged
        );
        assert!(discovery_snapshot.is_empty());
    }
}
