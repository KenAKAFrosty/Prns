use std::net::SocketAddr;
use tokio::sync::mpsc::UnboundedReceiver;

use prns_core::crypto::sha256;
use prns_core::interfaces::wifi_auto as contract;

use crate::network_device::AutoWifiDevicePolicy;

pub(crate) fn start(
    instance_tag: &[u8],
    devices: AutoWifiDevicePolicy,
) -> UnboundedReceiver<SocketAddr> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let _ = (instance_tag, devices);
        start_bonjour()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        start_mdns_sd(instance_tag, devices)
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn start_bonjour() -> UnboundedReceiver<SocketAddr> {
    let (sightings, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        match prns_ffi::mdns::macos::MacosMdnsBackend::new(
            contract::MDNS_SERVICE_PORT,
            &[("v", b"1")],
        )
        .await
        {
            Ok(mut backend) => {
                crate::diagnostic_log::info!(
                    "wifi-auto: advertising + browsing {} on port {}",
                    contract::MDNS_SERVICE_TYPE,
                    contract::MDNS_SERVICE_PORT
                );
                while let Some(target) = backend.next_sighting().await {
                    if sightings.send(target).is_err() {
                        return;
                    }
                }
            }
            Err(error) => {
                crate::diagnostic_log::warn!("wifi-auto: mDNS browse unavailable ({error:?})");
            }
        }
    });
    rx
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
mod mdns_sd_backend {
    use std::collections::BTreeSet;
    use std::net::{IpAddr, SocketAddr, SocketAddrV6};
    use std::time::Duration;

    use mdns_sd::{IfKind, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
    use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

    use super::*;

    const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
    const TXT_VERSION_STR: (&str, &str) = ("v", "1");

    pub(super) fn start_mdns_sd(
        instance_tag: &[u8],
        devices: AutoWifiDevicePolicy,
    ) -> UnboundedReceiver<SocketAddr> {
        let (sightings, rx) = mpsc::unbounded_channel();
        let instance = instance_label(instance_tag);
        tokio::spawn(async move {
            if let Err(error) = run(instance, devices, sightings).await {
                crate::diagnostic_log::warn!("wifi-auto: mDNS browse unavailable ({error})");
            }
        });
        rx
    }

    async fn run(
        instance: String,
        devices: AutoWifiDevicePolicy,
        sightings: UnboundedSender<SocketAddr>,
    ) -> Result<(), MdnsError> {
        let daemon = ServiceDaemon::new().map_err(MdnsError::Daemon)?;
        daemon
            .disable_interface(IfKind::All)
            .map_err(MdnsError::Daemon)?;
        let events = daemon
            .browse(contract::MDNS_SERVICE_TYPE)
            .map_err(MdnsError::Daemon)?;
        let mut advertised: Option<AdvertisedService> = None;
        let mut addresses = BTreeSet::new();
        let mut reconcile = tokio::time::interval(RECONCILE_INTERVAL);
        reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = reconcile.tick() => {
                    let next = eligible_ips(&devices)?;
                    if next != addresses {
                        reconcile_interfaces(&daemon, &addresses, &next)?;
                        addresses = next;
                        advertised = None;
                    }
                    advertised = reconcile_advertisement(
                        &daemon,
                        &instance,
                        &addresses,
                        advertised.take(),
                    )?;
                }
                event = events.recv_async() => {
                    let Ok(event) = event else {
                        break;
                    };
                    if let Some(target) = sighting_from_event(event) {
                        if sightings.send(target).is_err() {
                            break;
                        }
                    }
                }
            }
        }

        if let Some(advertised) = advertised {
            let _ = daemon.unregister(&advertised.fullname);
        }
        let _ = daemon.stop_browse(contract::MDNS_SERVICE_TYPE);
        let _ = daemon.shutdown();
        Ok(())
    }

    fn reconcile_interfaces(
        daemon: &ServiceDaemon,
        previous: &BTreeSet<IpAddr>,
        next: &BTreeSet<IpAddr>,
    ) -> Result<(), MdnsError> {
        let removed = previous
            .difference(next)
            .copied()
            .map(IfKind::Addr)
            .collect::<Vec<_>>();
        if !removed.is_empty() {
            daemon
                .disable_interface(removed)
                .map_err(MdnsError::Daemon)?;
        }
        let added = next
            .difference(previous)
            .copied()
            .map(IfKind::Addr)
            .collect::<Vec<_>>();
        if !added.is_empty() {
            daemon.enable_interface(added).map_err(MdnsError::Daemon)?;
        }
        Ok(())
    }

    fn reconcile_advertisement(
        daemon: &ServiceDaemon,
        instance: &str,
        addresses: &BTreeSet<IpAddr>,
        previous: Option<AdvertisedService>,
    ) -> Result<Option<AdvertisedService>, MdnsError> {
        if addresses.is_empty() {
            if let Some(previous) = previous {
                let _ = daemon.unregister(&previous.fullname);
            }
            return Ok(None);
        }
        let desired = AdvertisedService {
            fullname: format!("{instance}.{}", contract::MDNS_SERVICE_TYPE),
            hostname: format!("{instance}.local."),
            addresses: addresses.clone(),
        };
        if previous.as_ref() == Some(&desired) {
            return Ok(previous);
        }
        if let Some(previous) = previous {
            let _ = daemon.unregister(&previous.fullname);
        }
        let address_values = addresses.iter().copied().collect::<Vec<_>>();
        let properties = [TXT_VERSION_STR];
        let mut service = ServiceInfo::new(
            contract::MDNS_SERVICE_TYPE,
            instance,
            &desired.hostname,
            address_values.as_slice(),
            contract::MDNS_SERVICE_PORT,
            &properties[..],
        )
        .map_err(MdnsError::Daemon)?;
        service.set_interfaces(addresses.iter().copied().map(IfKind::Addr).collect());
        daemon.register(service).map_err(MdnsError::Daemon)?;
        Ok(Some(desired))
    }

    fn sighting_from_event(event: ServiceEvent) -> Option<SocketAddr> {
        let ServiceEvent::ServiceResolved(service) = event else {
            return None;
        };
        if !service
            .ty_domain
            .eq_ignore_ascii_case(contract::MDNS_SERVICE_TYPE)
            || service.get_port() != contract::MDNS_SERVICE_PORT
        {
            return None;
        }
        pick_sighting(service.get_addresses().iter().filter_map(scoped_socket))
    }

    fn scoped_socket(address: &ScopedIp) -> Option<SocketAddr> {
        match address {
            ScopedIp::V4(address) => {
                let ip = IpAddr::V4(*address.addr());
                (!ip.is_loopback()).then_some(SocketAddr::new(ip, contract::MDNS_SERVICE_PORT))
            }
            ScopedIp::V6(address) => {
                let ip = *address.addr();
                if ip.is_loopback() {
                    return None;
                }
                Some(SocketAddr::V6(SocketAddrV6::new(
                    ip,
                    contract::MDNS_SERVICE_PORT,
                    0,
                    address.scope_id().index,
                )))
            }
            _ => None,
        }
    }

    fn eligible_ips(devices: &AutoWifiDevicePolicy) -> Result<BTreeSet<IpAddr>, MdnsError> {
        let interfaces = if_addrs::get_if_addrs().map_err(MdnsError::Interfaces)?;
        let mut addresses = BTreeSet::new();
        for interface in interfaces {
            if !devices.allows(&interface.name, interface.is_loopback()) {
                continue;
            }
            let ip = match interface.addr {
                if_addrs::IfAddr::V4(address) => IpAddr::V4(address.ip),
                if_addrs::IfAddr::V6(address) => IpAddr::V6(address.ip),
            };
            if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
                continue;
            }
            addresses.insert(ip);
        }
        Ok(addresses)
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct AdvertisedService {
        fullname: String,
        hostname: String,
        addresses: BTreeSet<IpAddr>,
    }

    #[derive(Debug)]
    enum MdnsError {
        Interfaces(std::io::Error),
        Daemon(mdns_sd::Error),
    }

    impl std::fmt::Display for MdnsError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Interfaces(error) => {
                    write!(formatter, "enumerating LAN interfaces: {error}")
                }
                Self::Daemon(error) => write!(formatter, "DNS-SD: {error}"),
            }
        }
    }

    impl std::error::Error for MdnsError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match self {
                Self::Interfaces(error) => Some(error),
                Self::Daemon(error) => Some(error),
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
use mdns_sd_backend::start_mdns_sd;

#[cfg_attr(any(target_os = "macos", target_os = "ios"), allow(dead_code))]
fn instance_label(tag: &[u8]) -> String {
    let digest = sha256(tag);
    format!(
        "prns-{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

#[cfg_attr(any(target_os = "macos", target_os = "ios"), allow(dead_code))]
fn pick_sighting(addresses: impl IntoIterator<Item = SocketAddr>) -> Option<SocketAddr> {
    let mut link_local_v6 = None;
    let mut other = None;
    for address in addresses {
        match address {
            SocketAddr::V6(v6) if v6.ip().is_unicast_link_local() => {
                link_local_v6.get_or_insert(address);
            }
            _ => {
                other.get_or_insert(address);
            }
        }
    }
    link_local_v6.or(other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn instance_labels_are_stable_and_hostname_safe() {
        let label = instance_label(b"LAN");
        assert!(label.starts_with("prns-"));
        assert_eq!(label.len(), 13);
        assert_eq!(label, instance_label(b"LAN"));
        assert_ne!(label, instance_label(b"Mesh"));
    }

    #[test]
    fn pick_sighting_prefers_ipv6_link_local_for_autointerface() {
        let link_local = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            contract::MDNS_SERVICE_PORT,
        );
        let lan = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40)),
            contract::MDNS_SERVICE_PORT,
        );
        assert_eq!(pick_sighting([link_local, lan]), Some(link_local));
        assert_eq!(pick_sighting([lan]), Some(lan));
        assert_eq!(pick_sighting([]), None);
    }
}
