use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use prns_core::interfaces::wifi_direct::WifiDirectGroup;
use prns_core::interfaces::wifi_direct::{DataPlanePlan, GroupRole, SegmentAddress};

const LINK_LOCAL_WAIT_ROUNDS: u32 = 50;
const LINK_LOCAL_WAIT_STEP: Duration = Duration::from_millis(100);

/// The address the group owner claims on the P2P group netdev, matching the convention a native Android group owner uses (192.168.49.1).
/// A joining phone runs DHCP expecting the owner here, so every owner (wpa or native) serves clients at this address rather than an IPv6 link-local the phone cannot reach.
const GO_ADDRESS: Ipv4Addr = Ipv4Addr::new(192, 168, 49, 1);
const GO_ADDRESS_WAIT_ROUNDS: u32 = 60;
const GO_ADDRESS_WAIT_STEP: Duration = Duration::from_millis(100);

pub struct WpaGroup {
    role: GroupRole,
    plan: DataPlanePlan,
}

impl WpaGroup {
    #[must_use]
    pub fn new(role: GroupRole, plan: DataPlanePlan) -> Self {
        Self { role, plan }
    }
}

impl WifiDirectGroup for WpaGroup {
    fn role(&self) -> GroupRole {
        self.role
    }

    fn data_plane(&self) -> DataPlanePlan {
        self.plan
    }
}

pub fn role_from_group(role: &str) -> Option<GroupRole> {
    match role {
        "GO" => Some(GroupRole::Owner),
        "client" => Some(GroupRole::Client),
        _ => None,
    }
}

pub fn owner_plan() -> DataPlanePlan {
    DataPlanePlan::HostRendezvous {
        local: SegmentAddress::V4(GO_ADDRESS),
    }
}

pub fn client_plan(link_local: Ipv6Addr, scope: u32) -> DataPlanePlan {
    DataPlanePlan::ResolveOwnerByBeacon {
        local: link_local,
        scope,
    }
}

pub async fn wait_for_go_address(ifname: &str) -> bool {
    for _ in 0..GO_ADDRESS_WAIT_ROUNDS {
        if interface_has_address(ifname, GO_ADDRESS) {
            return true;
        }
        tokio::time::sleep(GO_ADDRESS_WAIT_STEP).await;
    }
    crate::diagnostic_log::warn!(
        "wifi-direct owner address {GO_ADDRESS} never appeared on {ifname}; is the group-owner DHCP helper running?"
    );
    false
}

fn interface_has_address(ifname: &str, target: Ipv4Addr) -> bool {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return false;
    };
    ifaces.into_iter().any(|iface| {
        iface.name == ifname && matches!(&iface.addr, if_addrs::IfAddr::V4(v4) if v4.ip == target)
    })
}

pub async fn wait_link_local(ifname: &str) -> Option<(Ipv6Addr, u32)> {
    for _ in 0..LINK_LOCAL_WAIT_ROUNDS {
        if let Some(found) = link_local_of(ifname) {
            return Some(found);
        }
        tokio::time::sleep(LINK_LOCAL_WAIT_STEP).await;
    }
    crate::diagnostic_log::warn!(
        "wifi-direct no link-local appeared on {ifname}; visible addresses:"
    );
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            crate::diagnostic_log::warn!(
                "wifi-direct   {} index={:?} addr={:?}",
                iface.name,
                iface.index,
                iface.addr.ip()
            );
        }
    }
    None
}

fn link_local_of(ifname: &str) -> Option<(Ipv6Addr, u32)> {
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.name != ifname {
                continue;
            }
            let Some(index) = iface.index else {
                continue;
            };
            if let if_addrs::IfAddr::V6(v6) = &iface.addr {
                if v6.ip.segments()[0] & 0xffc0 == 0xfe80 {
                    return Some((v6.ip, index));
                }
            }
        }
    }
    let index = ifindex(ifname)?;
    probe_link_local(index).map(|addr| (addr, index))
}

fn ifindex(ifname: &str) -> Option<u32> {
    std::fs::read_to_string(format!("/sys/class/net/{ifname}/ifindex"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn probe_link_local(index: u32) -> Option<Ipv6Addr> {
    let probe =
        std::net::UdpSocket::bind(std::net::SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
            .ok()?;
    let target =
        std::net::SocketAddrV6::new(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1), 9, 0, index);
    probe.connect(std::net::SocketAddr::V6(target)).ok()?;
    let std::net::SocketAddr::V6(local) = probe.local_addr().ok()? else {
        return None;
    };
    let addr = *local.ip();
    if addr.segments()[0] & 0xffc0 == 0xfe80 {
        Some(addr)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpa_role_strings_map_to_group_roles() {
        assert_eq!(role_from_group("GO"), Some(GroupRole::Owner));
        assert_eq!(role_from_group("client"), Some(GroupRole::Client));
        assert_eq!(role_from_group("mystery"), None);
    }

    #[test]
    fn an_owner_hosts_on_the_group_address_and_a_client_resolves_by_beacon() {
        let ll: Ipv6Addr = "fe80::1234".parse().expect("parses");
        assert_eq!(
            owner_plan(),
            DataPlanePlan::HostRendezvous {
                local: SegmentAddress::V4(GO_ADDRESS)
            }
        );
        assert_eq!(
            client_plan(ll, 7),
            DataPlanePlan::ResolveOwnerByBeacon {
                local: ll,
                scope: 7
            }
        );
    }
}
