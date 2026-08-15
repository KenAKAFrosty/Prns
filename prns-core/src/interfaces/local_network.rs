use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAddressScope {
    Loopback,
    Private,
    LinkLocal,
}

fn ipv4_scope(address: Ipv4Addr) -> Option<LocalAddressScope> {
    if address.is_loopback() {
        return Some(LocalAddressScope::Loopback);
    }
    if address.is_private() {
        return Some(LocalAddressScope::Private);
    }
    if address.is_link_local() {
        return Some(LocalAddressScope::LinkLocal);
    }
    None
}

fn ipv6_scope(address: Ipv6Addr) -> Option<LocalAddressScope> {
    if address.is_loopback() {
        return Some(LocalAddressScope::Loopback);
    }
    if address.is_unique_local() {
        return Some(LocalAddressScope::Private);
    }
    if address.is_unicast_link_local() {
        return Some(LocalAddressScope::LinkLocal);
    }
    None
}

#[must_use]
pub fn local_address_scope(address: IpAddr) -> Option<LocalAddressScope> {
    match address {
        IpAddr::V4(address) => ipv4_scope(address),
        IpAddr::V6(address) => ipv6_scope(address),
    }
}

#[must_use]
pub fn is_local_address(address: IpAddr) -> bool {
    local_address_scope(address).is_some()
}

#[must_use]
pub fn is_same_subnet(local: IpAddr, netmask: IpAddr, peer: IpAddr) -> bool {
    if local.is_loopback() && peer.is_loopback() {
        return true;
    }
    if local_address_scope(local).is_none() || local_address_scope(peer).is_none() {
        return false;
    }
    match (local, netmask, peer) {
        (IpAddr::V4(local), IpAddr::V4(mask), IpAddr::V4(peer)) => {
            u32::from(local) & u32::from(mask) == u32::from(peer) & u32::from(mask)
        }
        (IpAddr::V6(local), IpAddr::V6(mask), IpAddr::V6(peer)) => {
            u128::from(local) & u128::from(mask) == u128::from(peer) & u128::from(mask)
        }
        (IpAddr::V4(_), IpAddr::V4(_), IpAddr::V6(_))
        | (IpAddr::V4(_), IpAddr::V6(_), IpAddr::V4(_))
        | (IpAddr::V4(_), IpAddr::V6(_), IpAddr::V6(_))
        | (IpAddr::V6(_), IpAddr::V4(_), IpAddr::V4(_))
        | (IpAddr::V6(_), IpAddr::V4(_), IpAddr::V6(_))
        | (IpAddr::V6(_), IpAddr::V6(_), IpAddr::V4(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_local_unicast_ranges_are_eligible() {
        let accepted = [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.255.254",
            "169.254.1.2",
            "::1",
            "fc00::1",
            "fdff::1",
            "fe80::1",
        ];
        let rejected = [
            "0.0.0.0",
            "8.8.8.8",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "2001:4860:4860::8888",
            "ff02::1",
        ];
        for address in accepted {
            let address = address.parse().expect("test address parses");
            assert!(is_local_address(address), "{address} must be local");
        }
        for address in rejected {
            let address = address.parse().expect("test address parses");
            assert!(!is_local_address(address), "{address} must be rejected");
        }
    }

    #[test]
    fn subnet_validation_rejects_other_private_and_public_networks() {
        assert!(is_same_subnet(
            "192.168.4.1".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            "192.168.4.99".parse().unwrap(),
        ));
        assert!(!is_same_subnet(
            "192.168.4.1".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            "192.168.5.99".parse().unwrap(),
        ));
        assert!(!is_same_subnet(
            "192.168.4.1".parse().unwrap(),
            "255.255.255.0".parse().unwrap(),
            "8.8.8.8".parse().unwrap(),
        ));
    }
}
