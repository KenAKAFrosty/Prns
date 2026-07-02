use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::path_requests::discovery::DiscoveryPathRequestColumns;
use crate::wire::DestinationHash;

#[derive(Debug)]
pub struct FixedDiscoveryPathRequestColumns<const MAX_DISCOVERY_PATH_REQUESTS: usize> {
    len: usize,
    destinations: [DestinationHash; MAX_DISCOVERY_PATH_REQUESTS],
    requesting_interfaces: [InterfaceId; MAX_DISCOVERY_PATH_REQUESTS],
    expires_ats: [InstantMillis; MAX_DISCOVERY_PATH_REQUESTS],
}

impl<const MAX_DISCOVERY_PATH_REQUESTS: usize> Default
    for FixedDiscoveryPathRequestColumns<MAX_DISCOVERY_PATH_REQUESTS>
{
    fn default() -> Self {
        Self {
            len: 0,
            destinations: [DestinationHash::new([0u8; 16]); MAX_DISCOVERY_PATH_REQUESTS],
            requesting_interfaces: [InterfaceId::new([0u8; 8]); MAX_DISCOVERY_PATH_REQUESTS],
            expires_ats: [InstantMillis(0); MAX_DISCOVERY_PATH_REQUESTS],
        }
    }
}

impl<const MAX_DISCOVERY_PATH_REQUESTS: usize> DiscoveryPathRequestColumns
    for FixedDiscoveryPathRequestColumns<MAX_DISCOVERY_PATH_REQUESTS>
{
    fn capacity(&self) -> usize {
        MAX_DISCOVERY_PATH_REQUESTS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations[..self.len]
    }
    fn requesting_interfaces(&self) -> &[InterfaceId] {
        &self.requesting_interfaces[..self.len]
    }
    fn expires_ats(&self) -> &[InstantMillis] {
        &self.expires_ats[..self.len]
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        requesting_interface: InterfaceId,
        expires_at: InstantMillis,
    ) {
        if self.len >= MAX_DISCOVERY_PATH_REQUESTS {
            return;
        }
        let i = self.len;
        self.destinations[i] = destination;
        self.requesting_interfaces[i] = requesting_interface;
        self.expires_ats[i] = expires_at;
        self.len += 1;
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            self.destinations[index] = self.destinations[last];
            self.requesting_interfaces[index] = self.requesting_interfaces[last];
            self.expires_ats[index] = self.expires_ats[last];
        }
        self.len = last;
    }
}
