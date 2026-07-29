use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::time::Duration;

use mdns_sd::{IfKind, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use prns_core::interfaces::browser_rendezvous as contract;
use prns_core::interfaces::browser_rendezvous::BrowserRendezvousId;

use crate::network_device::AutoWifiDevicePolicy;

use super::catalog::BrowserGatewayEndpoint;
use super::network;

const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
const TXT_ID: &str = "id";
const TXT_VERSION: &str = "v";
const TXT_PATH: &str = "path";

pub(super) struct LanDiscovery {
    core: watch::Sender<bool>,
    snapshots: mpsc::UnboundedReceiver<Vec<BrowserGatewayEndpoint>>,
    task: JoinHandle<()>,
}

impl LanDiscovery {
    pub(super) fn spawn(id: BrowserRendezvousId, devices: AutoWifiDevicePolicy) -> Self {
        let (core, core_rx) = watch::channel(false);
        let (snapshot_tx, snapshots) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(id, devices, core_rx, snapshot_tx));
        Self {
            core,
            snapshots,
            task,
        }
    }

    pub(super) fn set_core(&self, is_core: bool) {
        self.core.send_replace(is_core);
    }

    pub(super) async fn next_snapshot(&mut self) -> Option<Vec<BrowserGatewayEndpoint>> {
        self.snapshots.recv().await
    }
}

impl Drop for LanDiscovery {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(
    id: BrowserRendezvousId,
    devices: AutoWifiDevicePolicy,
    mut core: watch::Receiver<bool>,
    snapshots: mpsc::UnboundedSender<Vec<BrowserGatewayEndpoint>>,
) {
    loop {
        while !*core.borrow_and_update() {
            if core.changed().await.is_err() {
                return;
            }
        }
        let _ = serve_core(id, &devices, &mut core, &snapshots).await;
        if !*core.borrow() {
            let _ = snapshots.send(Vec::new());
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(RETRY_INTERVAL) => {}
            changed = core.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

async fn serve_core(
    id: BrowserRendezvousId,
    devices: &AutoWifiDevicePolicy,
    core: &mut watch::Receiver<bool>,
    snapshots: &mpsc::UnboundedSender<Vec<BrowserGatewayEndpoint>>,
) -> Result<(), LanDiscoveryError> {
    let daemon = ServiceDaemon::new().map_err(LanDiscoveryError::Mdns)?;
    daemon
        .disable_interface(IfKind::All)
        .map_err(LanDiscoveryError::Mdns)?;
    let events = daemon
        .browse(contract::DNS_SD_SERVICE_TYPE)
        .map_err(LanDiscoveryError::Mdns)?;
    let mut addresses = BTreeSet::new();
    let mut visible = BTreeMap::<String, VisibleGateway>::new();
    let mut published: Option<PublishedService> = None;
    let mut reconcile = tokio::time::interval(RECONCILE_INTERVAL);
    reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = reconcile.tick() => {
                let next = eligible_ips(devices)?;
                if next != addresses {
                    reconcile_interfaces(&daemon, &addresses, &next)?;
                    addresses = next;
                    visible.clear();
                    let _ = snapshots.send(Vec::new());
                }
                reconcile_advertisement(&daemon, id, &addresses, &visible, &mut published)?;
            }
            event = events.recv_async() => {
                let Ok(event) = event else {
                    break;
                };
                let changed = apply_event(event, id, &mut visible);
                if changed {
                    let _ = snapshots.send(snapshot(&visible));
                    reconcile_advertisement(&daemon, id, &addresses, &visible, &mut published)?;
                }
            }
            changed = core.changed() => {
                if changed.is_err() || !*core.borrow() {
                    break;
                }
            }
        }
    }

    if let Some(published) = published {
        let _ = daemon.unregister(&published.fullname);
    }
    let _ = daemon.stop_browse(contract::DNS_SD_SERVICE_TYPE);
    let _ = daemon.shutdown();
    Ok(())
}

fn eligible_ips(devices: &AutoWifiDevicePolicy) -> Result<BTreeSet<IpAddr>, LanDiscoveryError> {
    network::eligible_addresses(devices, contract::PORT)
        .map(|addresses| {
            addresses
                .into_iter()
                .map(|address| address.socket.ip())
                .collect()
        })
        .map_err(LanDiscoveryError::Interfaces)
}

fn reconcile_interfaces(
    daemon: &ServiceDaemon,
    previous: &BTreeSet<IpAddr>,
    next: &BTreeSet<IpAddr>,
) -> Result<(), LanDiscoveryError> {
    let removed = previous
        .difference(next)
        .copied()
        .map(IfKind::Addr)
        .collect::<Vec<_>>();
    if !removed.is_empty() {
        daemon
            .disable_interface(removed)
            .map_err(LanDiscoveryError::Mdns)?;
    }
    let added = next
        .difference(previous)
        .copied()
        .map(IfKind::Addr)
        .collect::<Vec<_>>();
    if !added.is_empty() {
        daemon
            .enable_interface(added)
            .map_err(LanDiscoveryError::Mdns)?;
    }
    Ok(())
}

fn reconcile_advertisement(
    daemon: &ServiceDaemon,
    id: BrowserRendezvousId,
    addresses: &BTreeSet<IpAddr>,
    visible: &BTreeMap<String, VisibleGateway>,
    published: &mut Option<PublishedService>,
) -> Result<(), LanDiscoveryError> {
    if addresses.is_empty() {
        if let Some(previous) = published.take() {
            let _ = daemon.unregister(&previous.fullname);
        }
        return Ok(());
    }
    let alias_owner = alias_owner(id, visible);
    let desired = PublishedService {
        fullname: format!("{}.{}", id, contract::DNS_SD_SERVICE_TYPE),
        hostname: if alias_owner == id {
            "prns.local.".to_owned()
        } else {
            format!("prns-{id}.local.")
        },
        addresses: addresses.clone(),
    };
    if published.as_ref() == Some(&desired) {
        return Ok(());
    }
    let properties = [
        (TXT_ID, id.to_string()),
        (TXT_VERSION, contract::PROTOCOL_VERSION.to_string()),
        (TXT_PATH, contract::PATH.to_owned()),
    ];
    let address_values = addresses.iter().copied().collect::<Vec<_>>();
    let mut service = ServiceInfo::new(
        contract::DNS_SD_SERVICE_TYPE,
        &id.to_string(),
        &desired.hostname,
        address_values.as_slice(),
        contract::PORT,
        &properties[..],
    )
    .map_err(LanDiscoveryError::Mdns)?;
    service.set_interfaces(addresses.iter().copied().map(IfKind::Addr).collect());
    daemon.register(service).map_err(LanDiscoveryError::Mdns)?;
    *published = Some(desired);
    Ok(())
}

fn alias_owner(
    own_id: BrowserRendezvousId,
    visible: &BTreeMap<String, VisibleGateway>,
) -> BrowserRendezvousId {
    visible
        .values()
        .map(|gateway| gateway.id)
        .chain(std::iter::once(own_id))
        .min()
        .unwrap_or(own_id)
}

fn apply_event(
    event: ServiceEvent,
    own_id: BrowserRendezvousId,
    visible: &mut BTreeMap<String, VisibleGateway>,
) -> bool {
    match event {
        ServiceEvent::ServiceResolved(service) => {
            let fullname = service.get_fullname().to_ascii_lowercase();
            match parse_service(&service) {
                Some(gateway) if gateway.id != own_id => {
                    visible.insert(fullname, gateway);
                    true
                }
                Some(_) => false,
                None => visible.remove(&fullname).is_some(),
            }
        }
        ServiceEvent::ServiceRemoved(service_type, fullname)
            if service_type.eq_ignore_ascii_case(contract::DNS_SD_SERVICE_TYPE) =>
        {
            visible.remove(&fullname.to_ascii_lowercase()).is_some()
        }
        _ => false,
    }
}

fn parse_service(service: &mdns_sd::ResolvedService) -> Option<VisibleGateway> {
    if !service
        .ty_domain
        .eq_ignore_ascii_case(contract::DNS_SD_SERVICE_TYPE)
        || service.get_port() != contract::PORT
        || service.get_properties().len() != 3
        || service.get_property_val_str(TXT_VERSION)? != contract::PROTOCOL_VERSION.to_string()
        || service.get_property_val_str(TXT_PATH)? != contract::PATH
    {
        return None;
    }
    let id = BrowserRendezvousId::from_lower_hex(service.get_property_val_str(TXT_ID)?).ok()?;
    let keys = service
        .get_properties()
        .iter()
        .map(|property| property.key())
        .collect::<BTreeSet<_>>();
    if keys != BTreeSet::from([TXT_ID, TXT_PATH, TXT_VERSION]) {
        return None;
    }
    let mut literals = service
        .get_addresses()
        .iter()
        .filter_map(scoped_socket)
        .collect::<Vec<_>>();
    literals.sort_by_key(|address| (address_priority(*address), *address));
    let endpoint = match literals.first() {
        Some(address) => BrowserGatewayEndpoint::new(id, *address).ok()?,
        None => {
            let has_local = service
                .get_addresses()
                .iter()
                .any(|address| contract::is_local_address(address.to_ip_addr()));
            if !has_local {
                return None;
            }
            BrowserGatewayEndpoint::from_local_hostname(id, service.get_hostname()).ok()?
        }
    };
    Some(VisibleGateway { id, endpoint })
}

fn scoped_socket(address: &ScopedIp) -> Option<SocketAddr> {
    match address {
        ScopedIp::V4(address) => {
            let ip = IpAddr::V4(*address.addr());
            contract::is_local_address(ip)
                .then_some(SocketAddr::new(ip, contract::PORT))
                .filter(|address| !address.ip().is_loopback())
        }
        ScopedIp::V6(address) => {
            let ip = IpAddr::V6(*address.addr());
            if !contract::is_local_address(ip)
                || ip.is_loopback()
                || address.addr().is_unicast_link_local()
            {
                return None;
            }
            Some(SocketAddr::V6(SocketAddrV6::new(
                *address.addr(),
                contract::PORT,
                0,
                address.scope_id().index,
            )))
        }
        _ => None,
    }
}

fn address_priority(address: SocketAddr) -> u8 {
    match address.ip() {
        IpAddr::V4(address) if address.is_private() => 0,
        IpAddr::V6(_) => 1,
        IpAddr::V4(_) => 2,
    }
}

fn snapshot(visible: &BTreeMap<String, VisibleGateway>) -> Vec<BrowserGatewayEndpoint> {
    visible
        .values()
        .fold(BTreeMap::new(), |mut gateways, gateway| {
            gateways
                .entry(gateway.id)
                .or_insert_with(|| gateway.endpoint.clone());
            gateways
        })
        .into_values()
        .collect()
}

#[derive(Clone)]
struct VisibleGateway {
    id: BrowserRendezvousId,
    endpoint: BrowserGatewayEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedService {
    fullname: String,
    hostname: String,
    addresses: BTreeSet<IpAddr>,
}

#[derive(Debug)]
enum LanDiscoveryError {
    Interfaces(std::io::Error),
    Mdns(mdns_sd::Error),
}

impl std::fmt::Display for LanDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interfaces(error) => write!(formatter, "enumerating LAN interfaces: {error}"),
            Self::Mdns(error) => write!(formatter, "DNS-SD: {error}"),
        }
    }
}

impl std::error::Error for LanDiscoveryError {
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

    fn service(
        id: BrowserRendezvousId,
        host: &str,
        addresses: &[IpAddr],
    ) -> mdns_sd::ResolvedService {
        let properties = [
            (TXT_ID, id.to_string()),
            (TXT_VERSION, contract::PROTOCOL_VERSION.to_string()),
            (TXT_PATH, contract::PATH.to_owned()),
        ];
        ServiceInfo::new(
            contract::DNS_SD_SERVICE_TYPE,
            &id.to_string(),
            host,
            addresses,
            contract::PORT,
            &properties[..],
        )
        .unwrap()
        .as_resolved_service()
    }

    #[test]
    fn resolved_records_require_the_exact_transport_contract() {
        let id = BrowserRendezvousId::new([0x44; contract::ID_LEN]);
        let valid = service(id, "prns-44.local.", &["192.168.4.8".parse().unwrap()]);
        assert_eq!(parse_service(&valid).unwrap().endpoint.id(), id);

        let wrong_port = ServiceInfo::new(
            contract::DNS_SD_SERVICE_TYPE,
            &id.to_string(),
            "prns-44.local.",
            &["192.168.4.8".parse::<IpAddr>().unwrap()][..],
            80,
            &[
                (TXT_ID, id.to_string()),
                (TXT_VERSION, "1".to_owned()),
                (TXT_PATH, "/prns".to_owned()),
            ][..],
        )
        .unwrap()
        .as_resolved_service();
        assert!(parse_service(&wrong_port).is_none());
    }

    #[test]
    fn public_records_never_become_catalog_endpoints() {
        let id = BrowserRendezvousId::new([0x45; contract::ID_LEN]);
        let public = service(id, "deceptive.local.", &["8.8.8.8".parse().unwrap()]);
        assert!(parse_service(&public).is_none());
    }

    #[test]
    fn ipv6_link_local_records_use_the_validated_local_hostname() {
        let id = BrowserRendezvousId::new([0x46; contract::ID_LEN]);
        let local = service(id, "prns-46.local.", &["fe80::1".parse().unwrap()]);
        let endpoint = parse_service(&local).unwrap().endpoint;
        assert_eq!(endpoint.address(), None);
        assert_eq!(endpoint.hostname(), Some("prns-46.local"));
    }

    #[test]
    fn the_lowest_visible_stable_id_owns_the_convenience_alias() {
        let low = BrowserRendezvousId::new([0x10; contract::ID_LEN]);
        let middle = BrowserRendezvousId::new([0x20; contract::ID_LEN]);
        let high = BrowserRendezvousId::new([0x30; contract::ID_LEN]);
        let mut visible = BTreeMap::new();
        visible.insert(
            "high".to_owned(),
            VisibleGateway {
                id: high,
                endpoint: BrowserGatewayEndpoint::new(high, "192.168.4.30:42721".parse().unwrap())
                    .unwrap(),
            },
        );
        visible.insert(
            "low".to_owned(),
            VisibleGateway {
                id: low,
                endpoint: BrowserGatewayEndpoint::new(low, "192.168.4.10:42721".parse().unwrap())
                    .unwrap(),
            },
        );

        assert_eq!(alias_owner(middle, &visible), low);
        visible.remove("low");
        assert_eq!(alias_owner(middle, &visible), middle);
    }
}
