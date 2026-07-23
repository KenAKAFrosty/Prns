/// Distinct from WiFi Auto's rendezvous port so both listeners can coexist.
pub const WIFI_DIRECT_RENDEZVOUS_PORT: u16 = 42_717;

pub const WIFI_DIRECT_BEACON_PORT: u16 = 42_718;

pub const FAMILY_TAG: &[u8] = b"wifi-direct";

pub const DEVICE_NAME_MARKER: &str = "Prns";

pub const SERVICE_TYPE: &str = "_prns._tcp";

pub const NATIVE_SERVICE_INSTANCE: &str = "Prns-native";

pub const SUPPLICANT_SERVICE_INSTANCE: &str = "Prns-supplicant";

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

#[must_use]
pub fn service_instance_platform(instance: &str) -> Option<Platform> {
    match instance {
        SUPPLICANT_SERVICE_INSTANCE => Some(Platform::Supplicant),
        NATIVE_SERVICE_INSTANCE => Some(Platform::Native),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRole {
    WeHost,
    PeerHosts,
    Tiebreak,
}

#[must_use]
pub fn host_role(mine: Platform, peer: Platform) -> HostRole {
    match (mine, peer) {
        (Platform::Supplicant, Platform::Native) => HostRole::PeerHosts,
        (Platform::Native, Platform::Supplicant) => HostRole::WeHost,
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
    fn a_native_peer_hosts_for_a_supplicant_and_the_supplicant_defers() {
        assert_eq!(
            host_role(Platform::Supplicant, Platform::Native),
            HostRole::PeerHosts
        );
        assert_eq!(
            host_role(Platform::Native, Platform::Supplicant),
            HostRole::WeHost
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

    #[test]
    fn service_instances_carry_a_closed_platform_role() {
        assert_eq!(
            service_instance_platform(SUPPLICANT_SERVICE_INSTANCE),
            Some(Platform::Supplicant)
        );
        assert_eq!(
            service_instance_platform(NATIVE_SERVICE_INSTANCE),
            Some(Platform::Native)
        );
        assert_eq!(service_instance_platform(DEVICE_NAME_MARKER), None);
        assert_eq!(service_instance_platform("Prns-other"), None);
    }
}
