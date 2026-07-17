use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

use prns_config::{
    AddressFamilyPreference, TcpDialPlan, TcpListenHost, TcpListenPlan, UdpEndpointHost,
    UdpEndpointPlan,
};

pub(crate) fn tcp_target(plan: &TcpDialPlan) -> String {
    if plan.host.contains(':') && !plan.host.starts_with('[') {
        format!("[{}]:{}", plan.host, plan.port)
    } else {
        format!("{}:{}", plan.host, plan.port)
    }
}

pub(crate) async fn resolve_tcp_listener(plan: &TcpListenPlan) -> io::Result<SocketAddr> {
    match &plan.host {
        TcpListenHost::Any => Ok(match plan.address_family {
            AddressFamilyPreference::Ipv6 => {
                SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), plan.port)
            }
            AddressFamilyPreference::System | AddressFamilyPreference::Ipv4 => {
                SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), plan.port)
            }
        }),
        TcpListenHost::Address(host) => {
            resolve_host(host, plan.port, plan.address_family, "TCP listen address").await
        }
        TcpListenHost::Device(device) => resolve_device(device, plan.port, plan.address_family),
    }
}

pub(crate) async fn resolve_udp_endpoint(plan: &UdpEndpointPlan) -> io::Result<SocketAddr> {
    match &plan.host {
        UdpEndpointHost::Address(host) => {
            let addresses = tokio::net::lookup_host((host.as_str(), plan.port)).await?;
            addresses
                .into_iter()
                .find(SocketAddr::is_ipv4)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::AddrNotAvailable,
                        format!("UDP endpoint {host:?} has no IPv4 address"),
                    )
                })
        }
        UdpEndpointHost::DeviceBroadcast(device) => {
            let interfaces = if_addrs::get_if_addrs()?;
            interfaces
                .into_iter()
                .find_map(|interface| {
                    if interface.name != *device {
                        return None;
                    }
                    match interface.addr {
                        if_addrs::IfAddr::V4(address) => address
                            .broadcast
                            .map(|broadcast| SocketAddr::new(IpAddr::V4(broadcast), plan.port)),
                        if_addrs::IfAddr::V6(_) => None,
                    }
                })
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::AddrNotAvailable,
                        format!("kernel interface {device:?} has no IPv4 broadcast address"),
                    )
                })
        }
    }
}

pub(crate) const fn udp_ephemeral_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
}

async fn resolve_host(
    host: &str,
    port: u16,
    preference: AddressFamilyPreference,
    label: &str,
) -> io::Result<SocketAddr> {
    let addresses = tokio::net::lookup_host((host, port))
        .await?
        .collect::<std::vec::Vec<_>>();
    select_address(&addresses, preference).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("{label} {host:?} resolved to no usable address"),
        )
    })
}

fn resolve_device(
    device: &str,
    port: u16,
    preference: AddressFamilyPreference,
) -> io::Result<SocketAddr> {
    let interfaces = if_addrs::get_if_addrs()?;
    let addresses = interfaces
        .into_iter()
        .filter(|interface| interface.name == device)
        .map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(address) => SocketAddr::new(IpAddr::V4(address.ip), port),
            if_addrs::IfAddr::V6(address) => SocketAddr::V6(SocketAddrV6::new(
                address.ip,
                port,
                0,
                interface.index.unwrap_or(0),
            )),
        })
        .collect::<std::vec::Vec<_>>();
    select_address(&addresses, preference).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("kernel interface {device:?} has no usable IP address"),
        )
    })
}

fn select_address(
    addresses: &[SocketAddr],
    preference: AddressFamilyPreference,
) -> Option<SocketAddr> {
    let preferred = match preference {
        AddressFamilyPreference::System => return addresses.first().copied(),
        AddressFamilyPreference::Ipv4 => addresses.iter().find(|address| address.is_ipv4()),
        AddressFamilyPreference::Ipv6 => addresses.iter().find(|address| address.is_ipv6()),
    };
    preferred.copied().or_else(|| addresses.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_targets_bracket_literal_ipv6_hosts() {
        let mut plan = TcpDialPlan {
            host: "2001:db8::1".to_string(),
            port: 4242,
            connect_timeout: prns_config::ConnectTimeoutSeconds::new(5),
            reconnect_limit: prns_config::ReconnectLimit::Unlimited,
            address_family: AddressFamilyPreference::System,
            tunnel: prns_config::TcpTunnelMode::Direct,
        };
        assert_eq!(tcp_target(&plan), "[2001:db8::1]:4242");
        plan.host = "example.com".to_string();
        assert_eq!(tcp_target(&plan), "example.com:4242");
    }

    #[test]
    fn listener_address_selection_prefers_the_configured_family() {
        let v4 = "127.0.0.1:4242".parse().expect("valid IPv4 address");
        let v6 = "[::1]:4242".parse().expect("valid IPv6 address");
        assert_eq!(
            select_address(&[v4, v6], AddressFamilyPreference::Ipv6),
            Some(v6)
        );
        assert_eq!(
            select_address(&[v6], AddressFamilyPreference::Ipv4),
            Some(v6)
        );
    }
}
