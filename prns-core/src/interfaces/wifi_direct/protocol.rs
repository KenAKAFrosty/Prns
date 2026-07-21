/// Distinct from WiFi Auto's rendezvous port so both listeners can coexist.
pub const WIFI_DIRECT_RENDEZVOUS_PORT: u16 = 42_717;

pub const WIFI_DIRECT_BEACON_PORT: u16 = 42_718;

pub const FAMILY_TAG: &[u8] = b"wifi-direct";

pub const DEVICE_NAME_MARKER: &str = "Prns";

pub const SERVICE_TYPE: &str = "_prns._tcp";

pub const GROUP_SSID_PREFIX: &str = "DIRECT-Prns-";

pub const GROUP_PASSPHRASE: &str = "prns-mesh-shared-key";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupRole {
    Owner,
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoIntent(u8);

impl GoIntent {
    pub const PREFER_CLIENT: Self = Self(2);
    pub const BALANCED: Self = Self(7);
    pub const PREFER_OWNER: Self = Self(13);

    #[must_use]
    pub const fn wire(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerEvidence {
    ServiceRecord,
    NameMarker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Initiative {
    Ours,
    Theirs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Supplicant,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRole {
    WeHost,
    PeerHosts,
    Tiebreak,
}

/// A native peer can join a supplicant-hosted group, but a supplicant client cannot reliably join a native group owner, so the supplicant hosts mixed pairs.
#[must_use]
pub fn host_role(mine: Platform, peer: Platform) -> HostRole {
    match (mine, peer) {
        (Platform::Supplicant, Platform::Native) => HostRole::WeHost,
        (Platform::Native, Platform::Supplicant) => HostRole::PeerHosts,
        (Platform::Supplicant, Platform::Supplicant) | (Platform::Native, Platform::Native) => {
            HostRole::Tiebreak
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentAddress {
    V4(core::net::Ipv4Addr),
    V6LinkLocal {
        addr: core::net::Ipv6Addr,
        scope: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPlanePlan {
    HostRendezvous {
        local: SegmentAddress,
    },
    DialOwner {
        owner: SegmentAddress,
    },
    ResolveOwnerByBeacon {
        local: core::net::Ipv6Addr,
        scope: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_supplicant_hosts_for_a_native_peer_and_a_native_defers_to_it() {
        assert_eq!(
            host_role(Platform::Supplicant, Platform::Native),
            HostRole::WeHost
        );
        assert_eq!(
            host_role(Platform::Native, Platform::Supplicant),
            HostRole::PeerHosts
        );
    }

    #[test]
    fn a_matched_pair_falls_through_to_the_address_tiebreak() {
        assert_eq!(
            host_role(Platform::Supplicant, Platform::Supplicant),
            HostRole::Tiebreak
        );
        assert_eq!(
            host_role(Platform::Native, Platform::Native),
            HostRole::Tiebreak
        );
    }
}
