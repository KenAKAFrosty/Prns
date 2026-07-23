use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use prns_core::interfaces::wifi_direct::{
    host_role, DataPlanePlan, GoIntent, GroupRole, HostRole, Initiative, PeerEvidence, Platform,
    SegmentAddress,
};
use prns_core::interfaces::wifi_direct::{
    Availability, DiscoveryMode, WifiDirectBackend, WifiDirectEvent, WifiDirectGroup,
};
use prns_core::interfaces::MacAddress;

pub const AVAILABILITY_AVAILABLE: i32 = 0;
pub const AVAILABILITY_DISABLED: i32 = 1;
pub const AVAILABILITY_NO_PERMISSION: i32 = 2;
pub const AVAILABILITY_EXPERIMENTAL_DISABLED: i32 = 3;

const DISABLED_REASON: &str = "Wi-Fi P2P is turned off on this device";
const NO_PERMISSION_REASON: &str = "Wi-Fi P2P needs the nearby-devices permission";
const EXPERIMENTAL_DISABLED_REASON: &str = "experimental Wi-Fi P2P is disabled in this build";
const UNKNOWN_AVAILABILITY_REASON: &str = "Wi-Fi P2P reported an unknown availability state";

pub struct AndroidWifiDirectGroup {
    role: GroupRole,
    owner: Ipv4Addr,
}

impl WifiDirectGroup for AndroidWifiDirectGroup {
    fn role(&self) -> GroupRole {
        self.role
    }

    fn data_plane(&self) -> DataPlanePlan {
        match self.role {
            GroupRole::Owner => DataPlanePlan::HostRendezvous {
                local: SegmentAddress::V4(self.owner),
            },
            GroupRole::Client => DataPlanePlan::DialOwner {
                owner: SegmentAddress::V4(self.owner),
            },
        }
    }
}

enum Event {
    Sighting {
        peer: MacAddress,
        initiative: Initiative,
    },
    PeerGone {
        peer: MacAddress,
    },
    Invitation {
        peer: MacAddress,
    },
    GroupFormed {
        role: GroupRole,
        owner: Ipv4Addr,
    },
    FormationFailed {
        peer: MacAddress,
    },
    GroupLost,
    Availability(Availability),
}

#[derive(Clone, Copy, Default)]
struct Desired {
    discovery: bool,
}

struct Shared {
    desired: Mutex<Desired>,
    formation_requested: Mutex<Option<AndroidWifiDirectFormation>>,
    forming_with: Mutex<Option<MacAddress>>,
    remove_requested: Mutex<bool>,
    local_name_hash: Mutex<Option<i32>>,
    events: Mutex<VecDeque<Event>>,
    events_ready: Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AndroidWifiDirectFormation {
    pub peer: MacAddress,
    pub intent: GoIntent,
}

pub struct AndroidWifiDirectBridge {
    shared: Arc<Shared>,
}

impl Clone for AndroidWifiDirectBridge {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Default for AndroidWifiDirectBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl AndroidWifiDirectBridge {
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                desired: Mutex::new(Desired::default()),
                formation_requested: Mutex::new(None),
                forming_with: Mutex::new(None),
                remove_requested: Mutex::new(false),
                local_name_hash: Mutex::new(None),
                events: Mutex::new(VecDeque::new()),
                events_ready: Notify::new(),
            }),
        }
    }

    pub fn sighting(&self, peer: [u8; 6], from_supplicant: bool, peer_name_hash: i32) {
        let initiative = self.initiative_for(from_supplicant, peer_name_hash);
        self.push(Event::Sighting {
            peer: MacAddress::new(peer),
            initiative,
        });
    }

    pub fn set_local_name_hash(&self, hash: i32) {
        if let Ok(mut slot) = self.shared.local_name_hash.lock() {
            *slot = Some(hash);
        }
    }

    fn initiative_for(&self, from_supplicant: bool, peer_name_hash: i32) -> Initiative {
        let peer_platform = if from_supplicant {
            Platform::Supplicant
        } else {
            Platform::Native
        };
        match host_role(Platform::Native, peer_platform) {
            HostRole::PeerHosts => Initiative::Theirs,
            HostRole::WeHost => Initiative::Ours,
            HostRole::Tiebreak => match self.local_name_hash() {
                Some(local) if local < peer_name_hash => Initiative::Ours,
                _ => Initiative::Theirs,
            },
        }
    }

    fn local_name_hash(&self) -> Option<i32> {
        self.shared
            .local_name_hash
            .lock()
            .ok()
            .and_then(|slot| *slot)
    }

    pub fn peer_gone(&self, peer: [u8; 6]) {
        self.push(Event::PeerGone {
            peer: MacAddress::new(peer),
        });
    }

    pub fn invitation(&self, peer: [u8; 6]) {
        self.push(Event::Invitation {
            peer: MacAddress::new(peer),
        });
    }

    pub fn group_formed(&self, is_owner: bool, owner: Ipv4Addr) {
        if let Ok(mut slot) = self.shared.forming_with.lock() {
            *slot = None;
        }
        let role = if is_owner {
            GroupRole::Owner
        } else {
            GroupRole::Client
        };
        self.push(Event::GroupFormed { role, owner });
    }

    pub fn formation_failed(&self) {
        let peer = self
            .shared
            .forming_with
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(peer) = peer {
            self.push(Event::FormationFailed { peer });
        }
    }

    pub fn group_lost(&self) {
        self.push(Event::GroupLost);
    }

    pub fn availability(&self, code: i32) {
        let availability = match code {
            AVAILABILITY_AVAILABLE => Availability::Available,
            AVAILABILITY_DISABLED => Availability::Unavailable(DISABLED_REASON),
            AVAILABILITY_NO_PERMISSION => Availability::Unavailable(NO_PERMISSION_REASON),
            AVAILABILITY_EXPERIMENTAL_DISABLED => {
                Availability::Unavailable(EXPERIMENTAL_DISABLED_REASON)
            }
            _ => Availability::Unavailable(UNKNOWN_AVAILABILITY_REASON),
        };
        self.push(Event::Availability(availability));
    }

    fn push(&self, event: Event) {
        if let Ok(mut events) = self.shared.events.lock() {
            events.push_back(event);
        }
        self.shared.events_ready.notify_one();
    }

    #[must_use]
    pub fn desired_discovery(&self) -> bool {
        self.shared
            .desired
            .lock()
            .map(|desired| desired.discovery)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn take_formation_request(&self) -> Option<AndroidWifiDirectFormation> {
        self.shared
            .formation_requested
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    #[must_use]
    pub fn take_remove_group(&self) -> bool {
        self.shared
            .remove_requested
            .lock()
            .map(|mut slot| std::mem::replace(&mut *slot, false))
            .unwrap_or(false)
    }

    fn set_discovery(&self, discovery: bool) {
        if let Ok(mut desired) = self.shared.desired.lock() {
            desired.discovery = discovery;
        }
    }

    fn request_formation(&self, peer: MacAddress, intent: GoIntent) {
        if let Ok(mut slot) = self.shared.formation_requested.lock() {
            *slot = Some(AndroidWifiDirectFormation { peer, intent });
        }
        if let Ok(mut slot) = self.shared.forming_with.lock() {
            *slot = Some(peer);
        }
    }

    fn request_remove_group(&self) {
        if let Ok(mut slot) = self.shared.remove_requested.lock() {
            *slot = true;
        }
    }
}

pub struct AndroidWifiDirectBackend {
    bridge: AndroidWifiDirectBridge,
}

impl AndroidWifiDirectBackend {
    #[must_use]
    pub fn new(bridge: AndroidWifiDirectBridge) -> Self {
        Self { bridge }
    }
}

#[derive(Debug)]
pub enum AndroidWifiDirectError {}

impl WifiDirectBackend for AndroidWifiDirectBackend {
    type Error = AndroidWifiDirectError;
    type Group = AndroidWifiDirectGroup;

    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), Self::Error> {
        self.bridge.set_discovery(matches!(mode, DiscoveryMode::On));
        Ok(())
    }

    async fn form_group(&mut self, peer: MacAddress, intent: GoIntent) {
        self.bridge.request_formation(peer, intent);
    }

    async fn accept_invitation(&mut self, peer: MacAddress, intent: GoIntent) {
        self.bridge.request_formation(peer, intent);
    }

    async fn remove_group(&mut self) {
        self.bridge.request_remove_group();
    }

    async fn next_event(&mut self) -> WifiDirectEvent<AndroidWifiDirectGroup> {
        loop {
            let event = self
                .bridge
                .shared
                .events
                .lock()
                .ok()
                .and_then(|mut events| events.pop_front());
            match event {
                Some(Event::Sighting { peer, initiative }) => {
                    return WifiDirectEvent::Sighting {
                        peer,
                        evidence: PeerEvidence::ServiceRecord,
                        initiative,
                    };
                }
                Some(Event::PeerGone { peer }) => return WifiDirectEvent::PeerGone { peer },
                Some(Event::Invitation { peer }) => {
                    return WifiDirectEvent::Invitation { peer };
                }
                Some(Event::GroupFormed { role, owner }) => {
                    return WifiDirectEvent::GroupFormed {
                        group: AndroidWifiDirectGroup { role, owner },
                    };
                }
                Some(Event::FormationFailed { peer }) => {
                    return WifiDirectEvent::FormationFailed { peer };
                }
                Some(Event::GroupLost) => {
                    return WifiDirectEvent::GroupLost {
                        reason: prns_core::interfaces::wifi_direct::GroupEndReason::LinkLost,
                    };
                }
                Some(Event::Availability(state)) => {
                    return WifiDirectEvent::AvailabilityChanged(state);
                }
                None => self.bridge.shared.events_ready.notified().await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovery_and_group_requests_round_trip() {
        let bridge = AndroidWifiDirectBridge::new();
        let mut backend = AndroidWifiDirectBackend::new(bridge.clone());

        assert!(backend.set_discovery(DiscoveryMode::On).await.is_ok());
        assert!(bridge.desired_discovery());
        assert!(backend.set_discovery(DiscoveryMode::Off).await.is_ok());
        assert!(!bridge.desired_discovery());

        backend
            .form_group(MacAddress::new([0xA1; 6]), GoIntent::BALANCED)
            .await;
        assert_eq!(
            bridge.take_formation_request(),
            Some(AndroidWifiDirectFormation {
                peer: MacAddress::new([0xA1; 6]),
                intent: GoIntent::BALANCED,
            })
        );
        assert_eq!(bridge.take_formation_request(), None);

        bridge.formation_failed();
        assert!(matches!(
            backend.next_event().await,
            WifiDirectEvent::FormationFailed { peer }
                if peer == MacAddress::new([0xA1; 6])
        ));

        backend.remove_group().await;
        assert!(bridge.take_remove_group());
        assert!(!bridge.take_remove_group());
    }

    #[tokio::test]
    async fn permission_denial_is_a_typed_availability_event() {
        let bridge = AndroidWifiDirectBridge::new();
        let mut backend = AndroidWifiDirectBackend::new(bridge.clone());
        bridge.availability(AVAILABILITY_NO_PERMISSION);

        let event = backend.next_event().await;
        if let WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(reason)) = &event {
            assert_eq!(*reason, NO_PERMISSION_REASON);
        }
        assert!(matches!(
            event,
            WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn the_default_android_build_reports_the_experimental_boundary() {
        let bridge = AndroidWifiDirectBridge::new();
        let mut backend = AndroidWifiDirectBackend::new(bridge.clone());
        bridge.availability(AVAILABILITY_EXPERIMENTAL_DISABLED);

        assert!(matches!(
            backend.next_event().await,
            WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(
                EXPERIMENTAL_DISABLED_REASON
            ))
        ));
    }

    #[tokio::test]
    async fn an_explicit_supplicant_peer_is_formed_toward_by_the_native_host() {
        let bridge = AndroidWifiDirectBridge::new();
        let mut backend = AndroidWifiDirectBackend::new(bridge.clone());
        bridge.set_local_name_hash(i32::MIN);
        bridge.sighting([0xA1; 6], true, i32::MAX);

        assert!(matches!(
            backend.next_event().await,
            WifiDirectEvent::Sighting {
                initiative: Initiative::Ours,
                ..
            }
        ));
    }
}
