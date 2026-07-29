use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV6};

use tokio::net::TcpListener;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

use prns_core::interfaces::browser_rendezvous as contract;

use crate::network_device::AutoWifiDevicePolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EligibleAddress {
    pub(super) socket: SocketAddr,
    pub(super) netmask: IpAddr,
}

pub(super) struct AcceptedStream {
    pub(super) stream: tokio::net::TcpStream,
    pub(super) peer: SocketAddr,
    pub(super) local: SocketAddr,
}

pub(super) struct ListenerSet {
    tasks: HashMap<SocketAddr, JoinHandle<()>>,
}

impl ListenerSet {
    pub(super) fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    pub(super) fn insert(
        &mut self,
        listener: TcpListener,
        accepted: Sender<AcceptedStream>,
    ) -> std::io::Result<SocketAddr> {
        let local = listener.local_addr()?;
        let task = tokio::spawn(async move {
            while let Ok((stream, peer)) = listener.accept().await {
                if accepted
                    .send(AcceptedStream {
                        stream,
                        peer,
                        local,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        if let Some(previous) = self.tasks.insert(local, task) {
            previous.abort();
        }
        Ok(local)
    }

    pub(super) fn reconcile(&mut self, desired: &[EligibleAddress]) -> Vec<SocketAddr> {
        self.tasks.retain(|local, task| {
            let retain = !task.is_finished()
                && (local.ip().is_loopback()
                    || desired.iter().any(|address| address.socket == *local));
            if !retain {
                task.abort();
            }
            retain
        });
        self.tasks.keys().copied().collect()
    }

    pub(super) fn contains(&self, address: SocketAddr) -> bool {
        self.tasks.contains_key(&address)
    }

    pub(super) fn len(&self) -> usize {
        self.tasks.len()
    }

    pub(super) fn clear(&mut self) {
        for (_, task) in self.tasks.drain() {
            task.abort();
        }
    }
}

impl Drop for ListenerSet {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            task.abort();
        }
    }
}

pub(super) fn eligible_addresses(
    policy: &AutoWifiDevicePolicy,
    port: u16,
) -> std::io::Result<Vec<EligibleAddress>> {
    let interfaces = if_addrs::get_if_addrs()?;
    let mut addresses = Vec::new();
    for interface in interfaces {
        if !policy.allows(&interface.name, interface.is_loopback()) {
            continue;
        }
        let (ip, netmask, scope_id) = match interface.addr {
            if_addrs::IfAddr::V4(address) => {
                (IpAddr::V4(address.ip), IpAddr::V4(address.netmask), 0)
            }
            if_addrs::IfAddr::V6(address) => (
                IpAddr::V6(address.ip),
                IpAddr::V6(address.netmask),
                interface.index.unwrap_or(0),
            ),
        };
        if contract::local_address_scope(ip).is_none() || ip.is_loopback() {
            continue;
        }
        let socket = match ip {
            IpAddr::V4(address) => SocketAddr::new(IpAddr::V4(address), port),
            IpAddr::V6(address) => SocketAddr::V6(SocketAddrV6::new(address, port, 0, scope_id)),
        };
        if addresses
            .iter()
            .any(|candidate: &EligibleAddress| candidate.socket == socket)
        {
            continue;
        }
        addresses.push(EligibleAddress { socket, netmask });
    }
    addresses.sort_by_key(|address| address.socket);
    Ok(addresses)
}

pub(super) fn peer_is_eligible(
    local: SocketAddr,
    peer: SocketAddr,
    policy: &AutoWifiDevicePolicy,
) -> bool {
    if local.ip().is_loopback() {
        return peer.ip().is_loopback();
    }
    let Ok(addresses) = eligible_addresses(policy, local.port()) else {
        return false;
    };
    addresses.iter().any(|address| {
        address.socket == local && contract::is_same_subnet(local.ip(), address.netmask, peer.ip())
    })
}

pub(super) const fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_peers_are_authorized_only_on_the_loopback_listener() {
        let policy = AutoWifiDevicePolicy::default();
        assert!(peer_is_eligible(
            loopback(contract::PORT),
            "127.0.0.9:50000".parse().unwrap(),
            &policy,
        ));
        assert!(!peer_is_eligible(
            loopback(contract::PORT),
            "8.8.8.8:50000".parse().unwrap(),
            &policy,
        ));
    }
}
