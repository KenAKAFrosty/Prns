use crate::interfaces::{
    AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
};
use crate::routing::links::MAX_LINK_MTU;

/// Distinct from wifi_auto's TCP_RENDEZVOUS_PORT (42699), which AutoWifi binds on 0.0.0.0 — both
/// listeners must coexist on one host.
pub const WIFI_DIRECT_RENDEZVOUS_PORT: u16 = 42_717;

pub const WIFI_DIRECT_BEACON_PORT: u16 = 42_718;

pub const HARDWARE_MTU: usize = 1196;

pub const WIFI_DIRECT_HW_MTU: usize = if HARDWARE_MTU < MAX_LINK_MTU {
    HARDWARE_MTU
} else {
    MAX_LINK_MTU
};

pub const WIFI_DIRECT_BITRATE_GUESS_BPS: u32 = 100_000_000;

pub const FAMILY_TAG: &[u8] = b"wifi-direct";

pub const DEVICE_NAME_MARKER: &str = "Prns";

pub const SERVICE_TYPE: &str = "_prns._tcp";

pub const GROUP_SSID_PREFIX: &str = "DIRECT-Prns-";

pub const GROUP_PASSPHRASE: &str = "prns-mesh-shared-key";

pub const GO_MAX_CLIENTS: usize = 8;

pub const APPLE_UNAVAILABLE_REASON: &str =
    "no public Wi-Fi Direct API on Apple platforms; AWDL/Wi-Fi Aware is the analog";
pub const ESP32_UNAVAILABLE_REASON: &str =
    "no Wi-Fi Direct on ESP32; SoftAP+STA rides AutoWifi, connectionless rides ESP-NOW";

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

/// Which side brings up the group. A native (Android/iOS) peer can join a
/// wpa-hosted group, but a wpa client cannot reliably join a native GO, so a
/// mixed pair is always hosted by the Supplicant; a matched pair falls to an
/// address tiebreak.
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

pub fn descriptor(id: InterfaceId, bitrate_bps: u32) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        bitrate_bps: Some(bitrate_bps),
        hardware_mtu: Some(WIFI_DIRECT_HW_MTU),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
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
