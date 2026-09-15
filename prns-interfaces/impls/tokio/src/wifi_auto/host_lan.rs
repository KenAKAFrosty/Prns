use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};

use crate::network_device::AutoWifiDevicePolicy;

const MAX_HOST_LAN_INTERFACES: usize = 8;
const MAX_HOST_LAN_ADDRESSES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLanAddress {
    addr: IpAddr,
    prefix_len: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLanInterface {
    name: String,
    index: u32,
    addresses: std::vec::Vec<HostLanAddress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLanInterfaceError {
    EmptyName,
    InvalidIndex,
    NoAddresses,
    TooManyAddresses,
    InvalidPrefix,
    AddressNotLocal,
}

#[derive(Clone, Default)]
pub struct HostLanInventory {
    interfaces: Arc<Mutex<std::vec::Vec<HostLanInterface>>>,
}

impl HostLanAddress {
    pub fn new(addr: IpAddr, prefix_len: u8) -> Result<Self, HostLanInterfaceError> {
        let max_prefix = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix_len > max_prefix {
            return Err(HostLanInterfaceError::InvalidPrefix);
        }
        if addr.is_loopback() || !prns_core::interfaces::local_network::is_local_address(addr) {
            return Err(HostLanInterfaceError::AddressNotLocal);
        }
        Ok(Self { addr, prefix_len })
    }

    pub const fn addr(&self) -> IpAddr {
        self.addr
    }

    pub const fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    pub fn netmask(&self) -> IpAddr {
        match self.addr {
            IpAddr::V4(_) => IpAddr::V4(ipv4_netmask(self.prefix_len)),
            IpAddr::V6(_) => IpAddr::V6(ipv6_netmask(self.prefix_len)),
        }
    }
}

impl HostLanInterface {
    pub fn new(
        name: impl Into<String>,
        index: u32,
        addresses: impl Into<std::vec::Vec<HostLanAddress>>,
    ) -> Result<Self, HostLanInterfaceError> {
        let name = name.into();
        if name.is_empty() {
            return Err(HostLanInterfaceError::EmptyName);
        }
        if index == 0 {
            return Err(HostLanInterfaceError::InvalidIndex);
        }
        let addresses = addresses.into();
        if addresses.is_empty() {
            return Err(HostLanInterfaceError::NoAddresses);
        }
        if addresses.len() > MAX_HOST_LAN_ADDRESSES {
            return Err(HostLanInterfaceError::TooManyAddresses);
        }
        Ok(Self {
            name,
            index,
            addresses,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn index(&self) -> u32 {
        self.index
    }

    pub fn addresses(&self) -> &[HostLanAddress] {
        &self.addresses
    }

    pub fn link_local(&self) -> Option<Ipv6Addr> {
        self.addresses
            .iter()
            .find_map(|address| match address.addr {
                IpAddr::V6(ipv6) if ipv6.is_unicast_link_local() => Some(ipv6),
                _ => None,
            })
    }
}

impl HostLanInventory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(
        &self,
        interfaces: impl Into<std::vec::Vec<HostLanInterface>>,
    ) -> HostLanReplaceOutcome {
        let mut interfaces = interfaces.into();
        interfaces.truncate(MAX_HOST_LAN_INTERFACES);
        let Ok(mut stored) = self.interfaces.lock() else {
            return HostLanReplaceOutcome::Unavailable;
        };
        if *stored == interfaces {
            return HostLanReplaceOutcome::Unchanged;
        }
        *stored = interfaces;
        HostLanReplaceOutcome::Replaced
    }

    pub fn snapshot(&self) -> std::vec::Vec<HostLanInterface> {
        self.interfaces
            .lock()
            .map(|interfaces| interfaces.clone())
            .unwrap_or_default()
    }

    pub(crate) fn allowed_interfaces(
        &self,
        devices: &AutoWifiDevicePolicy,
    ) -> std::vec::Vec<HostLanInterface> {
        self.snapshot()
            .into_iter()
            .filter(|interface| devices.allows(&interface.name, false))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLanReplaceOutcome {
    Replaced,
    Unchanged,
    Unavailable,
}

fn ipv4_netmask(prefix_len: u8) -> Ipv4Addr {
    if prefix_len == 0 {
        return Ipv4Addr::UNSPECIFIED;
    }
    if prefix_len >= 32 {
        return Ipv4Addr::from(u32::MAX);
    }
    Ipv4Addr::from(!((1u32 << (32 - prefix_len)) - 1))
}

fn ipv6_netmask(prefix_len: u8) -> Ipv6Addr {
    let mut bits = [0u8; 16];
    let full_bytes = usize::from(prefix_len / 8).min(16);
    bits.iter_mut()
        .take(full_bytes)
        .for_each(|byte| *byte = 0xff);
    if full_bytes < 16 {
        let remainder = prefix_len % 8;
        if remainder != 0 {
            bits[full_bytes] = 0xff << (8 - remainder);
        }
    }
    Ipv6Addr::from(bits)
}

#[cfg(test)]
mod tests {
    use super::{
        ipv4_netmask, ipv6_netmask, HostLanAddress, HostLanInterface, HostLanInterfaceError,
        HostLanInventory, HostLanReplaceOutcome,
    };
    use crate::network_device::AutoWifiDevicePolicy;

    #[test]
    fn host_lan_interface_rejects_sysfs_shaped_zero_ifindex() {
        assert_eq!(
            HostLanInterface::new(
                "wlan0",
                0,
                vec![HostLanAddress::new("fe80::1".parse().unwrap(), 64).unwrap()]
            ),
            Err(HostLanInterfaceError::InvalidIndex)
        );
    }

    #[test]
    fn inventory_keeps_wlan0_link_local_for_default_device_policy() {
        let inventory = HostLanInventory::new();
        let interface = HostLanInterface::new(
            "wlan0",
            23,
            vec![
                HostLanAddress::new("fe80::282f:4eff:fe59:3321".parse().unwrap(), 64).unwrap(),
                HostLanAddress::new("192.168.1.18".parse().unwrap(), 24).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(
            inventory.replace(vec![interface.clone()]),
            HostLanReplaceOutcome::Replaced
        );
        let allowed = inventory.allowed_interfaces(&AutoWifiDevicePolicy::default());
        assert_eq!(allowed, vec![interface]);
        assert_eq!(
            allowed[0].link_local(),
            Some("fe80::282f:4eff:fe59:3321".parse().unwrap())
        );
        assert_eq!(
            allowed[0].addresses()[1].netmask(),
            "255.255.255.0".parse::<std::net::IpAddr>().unwrap()
        );
    }

    #[test]
    fn virtual_dummy0_stays_out_of_the_default_policy() {
        let inventory = HostLanInventory::new();
        inventory.replace(vec![HostLanInterface::new(
            "dummy0",
            2,
            vec![HostLanAddress::new("fe80::f858:24ff:fe9e:c3e".parse().unwrap(), 64).unwrap()],
        )
        .unwrap()]);
        assert!(inventory
            .allowed_interfaces(&AutoWifiDevicePolicy::default())
            .is_empty());
    }

    #[test]
    fn virtual_p2p_names_stay_out_of_the_default_policy() {
        let inventory = HostLanInventory::new();
        inventory.replace(vec![HostLanInterface::new(
            "p2p-wlan0-0",
            24,
            vec![HostLanAddress::new("fe80::2".parse().unwrap(), 64).unwrap()],
        )
        .unwrap()]);
        assert!(inventory
            .allowed_interfaces(&AutoWifiDevicePolicy::default())
            .is_empty());
    }

    #[test]
    fn netmasks_match_the_reported_prefix_length() {
        assert_eq!(
            ipv4_netmask(24),
            "255.255.255.0".parse::<std::net::Ipv4Addr>().unwrap()
        );
        assert_eq!(
            ipv6_netmask(64),
            "ffff:ffff:ffff:ffff::"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
        );
    }
}
