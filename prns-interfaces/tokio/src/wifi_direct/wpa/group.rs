use std::net::Ipv6Addr;
use std::time::Duration;

use prns_core::interfaces::wifi_direct::core::{DataPlanePlan, GroupRole, SegmentAddress};
use prns_core::interfaces::wifi_direct::seam::WifiDirectGroup;

const LINK_LOCAL_WAIT_ROUNDS: u32 = 50;
const LINK_LOCAL_WAIT_STEP: Duration = Duration::from_millis(100);

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

pub fn plan_for(role: GroupRole, link_local: Ipv6Addr, scope: u32) -> DataPlanePlan {
    match role {
        GroupRole::Owner => DataPlanePlan::HostRendezvous {
            local: SegmentAddress::V6LinkLocal {
                addr: link_local,
                scope,
            },
        },
        GroupRole::Client => DataPlanePlan::ResolveOwnerByBeacon {
            local: link_local,
            scope,
        },
    }
}

pub async fn wait_link_local(ifname: &str) -> Option<(Ipv6Addr, u32)> {
    for _ in 0..LINK_LOCAL_WAIT_ROUNDS {
        if let Some(found) = link_local_of(ifname) {
            return Some(found);
        }
        tokio::time::sleep(LINK_LOCAL_WAIT_STEP).await;
    }
    log::warn!("wifi-direct no link-local appeared on {ifname}; visible addresses:");
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            log::warn!(
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
    fn an_owner_hosts_and_a_client_resolves_by_beacon() {
        let ll: Ipv6Addr = "fe80::1234".parse().expect("parses");
        assert_eq!(
            plan_for(GroupRole::Owner, ll, 7),
            DataPlanePlan::HostRendezvous {
                local: SegmentAddress::V6LinkLocal { addr: ll, scope: 7 }
            }
        );
        assert_eq!(
            plan_for(GroupRole::Client, ll, 7),
            DataPlanePlan::ResolveOwnerByBeacon {
                local: ll,
                scope: 7
            }
        );
    }
}
