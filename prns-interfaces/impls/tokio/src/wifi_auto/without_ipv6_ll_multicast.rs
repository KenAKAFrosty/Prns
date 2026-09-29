//! Host admission must accept IPv4 Auto peers when IPv6 link-local multicast
//! is not on the LAN. TCP A records already work; UDP still requires IPv6.

use std::net::{IpAddr, SocketAddr};

use prns_core::interfaces::wifi_auto::{DiscoveryEndpoint, DiscoveryTransport};

use super::{
    endpoint_is_eligible, link_local_nics, local_prefixes, HostLanAddress, HostLanInterface,
    HostLanInventory, LocalPrefix, NetworkDiscoveryOwner,
};
use crate::network_device::AutoWifiDevicePolicy;

#[test]
fn ipv4_tcp_peer_on_the_station_subnet_is_eligible_without_any_ipv6() {
    let local = LocalPrefix {
        addr: "192.168.1.36".parse().unwrap(),
        netmask: "255.255.255.0".parse().unwrap(),
        index: 14,
    };
    let peer = DiscoveryEndpoint::tcp(SocketAddr::new(
        IpAddr::V4("192.168.1.40".parse().unwrap()),
        prns_core::interfaces::wifi_auto::TCP_RENDEZVOUS_PORT,
    ))
    .expect("IPv4 TCP rendezvous is representable");
    assert_eq!(peer.transport(), DiscoveryTransport::Tcp);
    assert!(endpoint_is_eligible(
        peer,
        NetworkDiscoveryOwner::Host,
        &[local]
    ));
}

/// Embassy and Android publish a unicast-discovery A. The host still rejects
/// IPv4 UDP endpoints (`UdpRequiresIpv6`). Keep this until that constructor
/// accepts same-subnet A records.
#[test]
#[ignore = "DiscoveryEndpoint::udp still requires IPv6; IPv4 UDP is the blocked-multicast contract"]
fn ipv4_udp_peer_on_the_station_subnet_is_eligible_without_any_ipv6() {
    let local = LocalPrefix {
        addr: "192.168.1.36".parse().unwrap(),
        netmask: "255.255.255.0".parse().unwrap(),
        index: 14,
    };
    let peer = DiscoveryEndpoint::udp(SocketAddr::new(
        IpAddr::V4("192.168.1.62".parse().unwrap()),
        prns_core::interfaces::wifi_auto::UNICAST_DISCOVERY_PORT,
    ))
    .expect("IPv4 UDP discovery endpoint is representable");
    assert!(endpoint_is_eligible(
        peer,
        NetworkDiscoveryOwner::Host,
        &[local]
    ));
}

#[test]
fn host_lan_ifindex_fills_link_local_scope_when_sysfs_cannot() {
    let inventory = HostLanInventory::new();
    inventory.replace(vec![HostLanInterface::new(
        "wlan0",
        23,
        vec![
            HostLanAddress::new("fe80::282f:4eff:fe59:3321".parse().unwrap(), 64).unwrap(),
            HostLanAddress::new("192.168.1.18".parse().unwrap(), 24).unwrap(),
        ],
    )
    .unwrap()]);
    let policy = AutoWifiDevicePolicy::default();
    let nics = link_local_nics(&policy, &inventory);
    assert!(
        nics.iter().any(|nic| {
            nic.index == 23
                && nic.link_local
                    == "fe80::282f:4eff:fe59:3321"
                        .parse::<std::net::Ipv6Addr>()
                        .unwrap()
        }),
        "host-reported wlan0 ifindex must become a link-local NIC: {nics:?}"
    );
    let prefixes = local_prefixes(&policy, &inventory);
    let hv4 = DiscoveryEndpoint::udp("[fe80::aea7:4ff:fee1:4b3c%23]:29717".parse().unwrap())
        .expect("scoped HV4 AAAA is a UDP endpoint");
    assert!(endpoint_is_eligible(
        hv4,
        NetworkDiscoveryOwner::Host,
        &prefixes
    ));
}
