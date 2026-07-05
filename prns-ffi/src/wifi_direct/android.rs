use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use prns_core::interfaces::wifi_direct::core::{
    host_role, DataPlanePlan, GoIntent, GroupRole, HostRole, Initiative, PeerEvidence, Platform,
    SegmentAddress,
};
use prns_core::interfaces::wifi_direct::seam::{
    Availability, DiscoveryMode, WifiDirectBackend, WifiDirectEvent, WifiDirectGroup,
};
use prns_core::interfaces::MacAddress;

pub const AVAILABILITY_AVAILABLE: i32 = 0;
pub const AVAILABILITY_DISABLED: i32 = 1;
pub const AVAILABILITY_NO_PERMISSION: i32 = 2;

const DISABLED_REASON: &str = "Wi-Fi P2P is turned off on this device";
const NO_PERMISSION_REASON: &str = "Wi-Fi P2P needs the nearby-devices permission";

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
    GroupLost,
    Availability(Availability),
}

#[derive(Clone, Copy, Default)]
struct Desired {
    discovery: bool,
}

struct Shared {
    desired: Mutex<Desired>,
    host_requested: Mutex<bool>,
    remove_requested: Mutex<bool>,
    local_name_hash: Mutex<Option<i32>>,
    events: Mutex<VecDeque<Event>>,
    events_ready: Notify,
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
                host_requested: Mutex::new(false),
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
        let role = if is_owner {
            GroupRole::Owner
        } else {
            GroupRole::Client
        };
        self.push(Event::GroupFormed { role, owner });
    }

    pub fn group_lost(&self) {
        self.push(Event::GroupLost);
    }

    pub fn availability(&self, code: i32) {
        let availability = match code {
            AVAILABILITY_AVAILABLE => Availability::Available,
            AVAILABILITY_NO_PERMISSION => Availability::Unavailable(NO_PERMISSION_REASON),
            _ => Availability::Unavailable(DISABLED_REASON),
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
    pub fn take_host_request(&self) -> bool {
        self.shared
            .host_requested
            .lock()
            .map(|mut slot| std::mem::replace(&mut *slot, false))
            .unwrap_or(false)
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

    fn request_host(&self) {
        if let Ok(mut slot) = self.shared.host_requested.lock() {
            *slot = true;
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

    async fn form_group(&mut self, _peer: MacAddress, _intent: GoIntent) {
        self.bridge.request_host();
    }

    async fn accept_invitation(&mut self, _peer: MacAddress, _intent: GoIntent) {
        self.bridge.request_host();
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
                Some(Event::GroupLost) => {
                    return WifiDirectEvent::GroupLost {
                        reason: prns_core::interfaces::wifi_direct::seam::GroupEndReason::LinkLost,
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
