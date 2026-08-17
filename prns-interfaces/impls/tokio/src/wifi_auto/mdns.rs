use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::num::NonZeroU8;
use std::time::Duration;

use mdns_sd::{IfKind, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};

use prns_core::interfaces::local_network::is_local_address;
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::wifi_auto::{
    AdvertisementInsertion, CandidateInsertion, DiscoveryEndpoint, DiscoveryServiceName,
    DiscoveryServiceNameError, DiscoverySnapshot, DiscoveryVersion, DiscoveryVersionError,
    ServiceAdvertisement,
};

use crate::network_device::AutoWifiDevicePolicy;

use super::{
    DiscoveryLifecycleError, DiscoveryParticipation, ServiceDiscovery, ServiceDiscoveryPublisher,
};

const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
pub(crate) const DISCOVERY_CAPACITY: NonZeroU8 = NonZeroU8::new(128).unwrap();

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
    let instance_identity = InstanceIdentity::fresh();
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

        let central_session_follow_up = match run_central_session(
            &instance_identity,
            &auto_wifi_device_policy,
            &mut service_discovery_publisher,
        )
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
    instance_identity: &InstanceIdentity,
    auto_wifi_device_policy: &AutoWifiDevicePolicy,
    service_discovery_publisher: &mut ServiceDiscoveryPublisher,
) -> Result<CentralDiscoverySessionEnd, MdnsDiscoveryError> {
    let mdns_daemon = ServiceDaemon::new().map_err(MdnsDiscoveryError::Mdns)?;
    mdns_daemon
        .disable_interface(IfKind::All)
        .map_err(MdnsDiscoveryError::Mdns)?;
    let service_event_receiver = mdns_daemon
        .browse(contract::DNS_SD_SERVICE_TYPE)
        .map_err(MdnsDiscoveryError::Mdns)?;
    let mut eligible_ip_addresses = BTreeSet::new();
    let mut published_service: Option<PublishedService> = None;
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
                        interface_changes.apply(&mdns_daemon)?;
                        eligible_ip_addresses = current_eligible_ip_addresses;
                        discovery_snapshot =
                            DiscoverySnapshot::new(service_discovery_publisher.capacity());
                        let _ = service_discovery_publisher
                            .replace_snapshot(discovery_snapshot.clone());
                    }
                }
                reconcile_service_advertisement(
                    &mdns_daemon,
                    instance_identity,
                    &eligible_ip_addresses,
                    &mut published_service,
                )?;
            }
            received_service_event = service_event_receiver.recv_async() => {
                let service_event = match received_service_event {
                    Ok(service_event) => service_event,
                    Err(_backend_stopped) => break CentralDiscoverySessionEnd::BackendStopped,
                };
                match apply_service_event(
                    &mut discovery_snapshot,
                    &service_event,
                    &instance_identity.service_fullname,
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

    if let Some(published_service) = published_service {
        let _ = mdns_daemon.unregister(&published_service.service_fullname);
    }
    let _ = mdns_daemon.stop_browse(contract::DNS_SD_SERVICE_TYPE);
    let _ = mdns_daemon.shutdown();
    Ok(central_session_end)
}

fn apply_service_event(
    discovery_snapshot: &mut DiscoverySnapshot,
    service_event: &ServiceEvent,
    local_service_fullname: &str,
) -> ServiceEventOutcome {
    match service_event {
        ServiceEvent::ServiceResolved(resolved_service) => {
            apply_resolved_service(discovery_snapshot, resolved_service, local_service_fullname)
        }
        ServiceEvent::ServiceRemoved(removed_service_type, removed_service_fullname)
            if removed_service_type.eq_ignore_ascii_case(contract::DNS_SD_SERVICE_TYPE) =>
        {
            match DiscoveryServiceName::new(removed_service_fullname) {
                Ok(discovery_service_name)
                    if discovery_snapshot.remove(&discovery_service_name) =>
                {
                    ServiceEventOutcome::SnapshotChanged
                }
                Ok(_unknown_service_name) => ServiceEventOutcome::SnapshotUnchanged,
                Err(
                    DiscoveryServiceNameError::Empty | DiscoveryServiceNameError::TooLong { .. },
                ) => ServiceEventOutcome::SnapshotUnchanged,
            }
        }
        ServiceEvent::SearchStarted(_)
        | ServiceEvent::SearchStopped(_)
        | ServiceEvent::ServiceFound(_, _)
        | ServiceEvent::ServiceRemoved(_, _)
        | _ => ServiceEventOutcome::SnapshotUnchanged,
    }
}

fn apply_resolved_service(
    discovery_snapshot: &mut DiscoverySnapshot,
    resolved_service: &ResolvedService,
    local_service_fullname: &str,
) -> ServiceEventOutcome {
    let service_advertisement =
        match build_service_advertisement(resolved_service, local_service_fullname) {
            Ok(service_advertisement) => service_advertisement,
            Err(
                ServiceAdvertisementRejection::WrongServiceType
                | ServiceAdvertisementRejection::OwnService
                | ServiceAdvertisementRejection::InvalidServiceName(_),
            ) => return ServiceEventOutcome::SnapshotUnchanged,
            Err(
                ServiceAdvertisementRejection::InvalidVersion(_)
                | ServiceAdvertisementRejection::NoEligibleEndpoints,
            ) => {
                let Ok(discovery_service_name) =
                    DiscoveryServiceName::new(resolved_service.get_fullname())
                else {
                    return ServiceEventOutcome::SnapshotUnchanged;
                };
                return match discovery_snapshot.remove(&discovery_service_name) {
                    true => ServiceEventOutcome::SnapshotChanged,
                    false => ServiceEventOutcome::SnapshotUnchanged,
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
    Ok(eligible_ip_addresses)
}

fn reconcile_service_advertisement(
    mdns_daemon: &ServiceDaemon,
    instance_identity: &InstanceIdentity,
    eligible_ip_addresses: &BTreeSet<IpAddr>,
    published_service: &mut Option<PublishedService>,
) -> Result<(), MdnsDiscoveryError> {
    if eligible_ip_addresses.is_empty() {
        if let Some(previously_published_service) = published_service.take() {
            let _ = mdns_daemon.unregister(&previously_published_service.service_fullname);
        }
        return Ok(());
    }
    let desired_service = PublishedService {
        service_fullname: instance_identity.service_fullname.clone(),
        advertised_ip_addresses: eligible_ip_addresses.clone(),
    };
    if published_service.as_ref() == Some(&desired_service) {
        return Ok(());
    }
    if let Some(previously_published_service) = published_service.take() {
        let _ = mdns_daemon.unregister(&previously_published_service.service_fullname);
    }
    let txt_properties = [(contract::TXT_VERSION_KEY, contract::TXT_VERSION_VALUE)];
    let advertised_ip_addresses = eligible_ip_addresses.iter().copied().collect::<Vec<_>>();
    let mut service_info = ServiceInfo::new(
        contract::DNS_SD_SERVICE_TYPE,
        &instance_identity.instance_name,
        &instance_identity.hostname,
        advertised_ip_addresses.as_slice(),
        contract::TCP_RENDEZVOUS_PORT,
        &txt_properties[..],
    )
    .map_err(MdnsDiscoveryError::Mdns)?;
    service_info.set_interfaces(
        eligible_ip_addresses
            .iter()
            .copied()
            .map(IfKind::Addr)
            .collect(),
    );
    mdns_daemon
        .register(service_info)
        .map_err(MdnsDiscoveryError::Mdns)?;
    *published_service = Some(desired_service);
    Ok(())
}

fn build_service_advertisement(
    resolved_service: &ResolvedService,
    local_service_fullname: &str,
) -> Result<ServiceAdvertisement, ServiceAdvertisementRejection> {
    if !resolved_service
        .ty_domain
        .eq_ignore_ascii_case(contract::DNS_SD_SERVICE_TYPE)
    {
        return Err(ServiceAdvertisementRejection::WrongServiceType);
    }
    if resolved_service
        .get_fullname()
        .eq_ignore_ascii_case(local_service_fullname)
    {
        return Err(ServiceAdvertisementRejection::OwnService);
    }
    let version_metadata = match resolved_service.get_property_val(contract::TXT_VERSION_KEY) {
        None => None,
        Some(Some(value)) => Some(value),
        Some(None) => Some(&[][..]),
    };
    DiscoveryVersion::parse(version_metadata)
        .map_err(ServiceAdvertisementRejection::InvalidVersion)?;
    let discovery_service_name = DiscoveryServiceName::new(resolved_service.get_fullname())
        .map_err(ServiceAdvertisementRejection::InvalidServiceName)?;
    let mut discovery_endpoints = BTreeSet::new();
    for scoped_ip_address in resolved_service.get_addresses() {
        if let Some(discovery_endpoint) =
            validated_discovery_endpoint(scoped_ip_address, resolved_service.get_port())
        {
            discovery_endpoints.insert(discovery_endpoint);
        }
    }
    let mut service_advertisement = ServiceAdvertisement::new(discovery_service_name);
    for discovery_endpoint in discovery_endpoints {
        if service_advertisement.insert(discovery_endpoint) == CandidateInsertion::AtCapacity {
            break;
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
    DiscoveryEndpoint::new(socket_address).ok()
}

struct InstanceIdentity {
    instance_name: String,
    hostname: String,
    service_fullname: String,
}

impl InstanceIdentity {
    fn fresh() -> Self {
        let mut instance_suffix = [0u8; 4];
        if getrandom::getrandom(&mut instance_suffix).is_err() {
            instance_suffix = std::process::id().to_be_bytes();
        }
        let instance_name = format!(
            "prns-{:02x}{:02x}{:02x}{:02x}",
            instance_suffix[0], instance_suffix[1], instance_suffix[2], instance_suffix[3]
        );
        Self {
            hostname: format!("{instance_name}.{}", contract::DNS_SD_LOCAL_DOMAIN),
            service_fullname: format!("{instance_name}.{}", contract::DNS_SD_SERVICE_TYPE),
            instance_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedService {
    service_fullname: String,
    advertised_ip_addresses: BTreeSet<IpAddr>,
}

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
    NoEligibleEndpoints,
}

#[derive(Debug)]
enum MdnsDiscoveryError {
    Interfaces(std::io::Error),
    Mdns(mdns_sd::Error),
}

impl std::fmt::Display for MdnsDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interfaces(error) => write!(formatter, "enumerating LAN interfaces: {error}"),
            Self::Mdns(error) => write!(formatter, "DNS-SD: {error}"),
        }
    }
}

impl std::error::Error for MdnsDiscoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Interfaces(error) => Some(error),
            Self::Mdns(error) => Some(error),
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
        instance_name: &str,
        ip_addresses: &[IpAddr],
        service_port: u16,
        txt_properties: &[(&str, &str)],
    ) -> ResolvedService {
        ServiceInfo::new(
            contract::DNS_SD_SERVICE_TYPE,
            instance_name,
            &format!("{instance_name}.local."),
            ip_addresses,
            service_port,
            txt_properties,
        )
        .unwrap()
        .as_resolved_service()
    }

    #[test]
    fn records_keep_all_valid_candidates_and_reject_our_own_record() {
        let instance_identity = InstanceIdentity::fresh();
        let peer_service = resolved_service(
            "prns-cafe0001",
            &["fe80::1".parse().unwrap(), "192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[(contract::TXT_VERSION_KEY, contract::TXT_VERSION_VALUE)],
        );
        let peer_service_advertisement =
            build_service_advertisement(&peer_service, &instance_identity.service_fullname)
                .expect("peer is accepted");
        assert_eq!(peer_service_advertisement.endpoints().len(), 1);
        assert_eq!(
            peer_service_advertisement.endpoints()[0].socket_addr(),
            "192.168.4.8:42699".parse().unwrap()
        );

        let local_service = resolved_service(
            &instance_identity.instance_name,
            &["192.168.4.9".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            build_service_advertisement(&local_service, &instance_identity.service_fullname),
            Err(ServiceAdvertisementRejection::OwnService)
        );
    }

    #[test]
    fn invalid_endpoints_and_explicit_incompatible_versions_are_rejected() {
        let instance_identity = InstanceIdentity::fresh();
        let public_service = resolved_service(
            "prns-deadbeef",
            &["8.8.8.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            build_service_advertisement(&public_service, &instance_identity.service_fullname),
            Err(ServiceAdvertisementRejection::NoEligibleEndpoints)
        );

        let incompatible_service = resolved_service(
            "prns-version2",
            &["192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[(contract::TXT_VERSION_KEY, "2")],
        );
        assert_eq!(
            build_service_advertisement(&incompatible_service, &instance_identity.service_fullname,),
            Err(ServiceAdvertisementRejection::InvalidVersion(
                DiscoveryVersionError::Unsupported(2)
            ))
        );
    }

    #[test]
    fn missing_version_is_implicit_v1() {
        let instance_identity = InstanceIdentity::fresh();
        let legacy_service = resolved_service(
            "prns-legacy",
            &["192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        build_service_advertisement(&legacy_service, &instance_identity.service_fullname)
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
        let local_service_fullname = "ours._reticulum._tcp.local.";
        let first_service = resolved_service(
            "first",
            &["192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        let first_service_name = DiscoveryServiceName::new(first_service.get_fullname()).unwrap();
        let mut discovery_snapshot = DiscoverySnapshot::new(NonZeroU8::new(1).unwrap());

        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(first_service)),
                local_service_fullname,
            ),
            ServiceEventOutcome::SnapshotChanged
        );

        let repeated_service = resolved_service(
            "first",
            &["192.168.4.8".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(repeated_service)),
                local_service_fullname,
            ),
            ServiceEventOutcome::SnapshotUnchanged
        );

        let overflow_service = resolved_service(
            "overflow",
            &["192.168.4.9".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(overflow_service)),
                local_service_fullname,
            ),
            ServiceEventOutcome::RejectedAtCapacity
        );
        assert_eq!(discovery_snapshot.len(), 1);

        let replacement_service = resolved_service(
            "first",
            &["192.168.4.10".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(replacement_service)),
                local_service_fullname,
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
            "first",
            &["192.168.4.10".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[(contract::TXT_VERSION_KEY, "2")],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(incompatible_service)),
                local_service_fullname,
            ),
            ServiceEventOutcome::SnapshotChanged
        );
        assert!(discovery_snapshot.is_empty());

        let restored_service = resolved_service(
            "first",
            &["192.168.4.10".parse().unwrap()],
            contract::TCP_RENDEZVOUS_PORT,
            &[],
        );
        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceResolved(Box::new(restored_service)),
                local_service_fullname,
            ),
            ServiceEventOutcome::SnapshotChanged
        );

        assert_eq!(
            apply_service_event(
                &mut discovery_snapshot,
                &ServiceEvent::ServiceRemoved(
                    contract::DNS_SD_SERVICE_TYPE.to_owned(),
                    first_service_name.as_str().to_owned(),
                ),
                local_service_fullname,
            ),
            ServiceEventOutcome::SnapshotChanged
        );
        assert!(discovery_snapshot.is_empty());
    }
}
