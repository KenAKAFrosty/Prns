use std::collections::VecDeque;
use std::net::Ipv6Addr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Notify;

use prns_core::interfaces::wifi_aware::core::{
    AwareEndpoint, NdpRole, RendezvousToken, AWARE_RENDEZVOUS_PORT,
};
use prns_core::interfaces::wifi_aware::seam::{
    Availability, DiscoveryMode, NdpEndReason, WifiAwareBackend, WifiAwareEvent,
};

pub const AVAILABILITY_AVAILABLE: i32 = 0;
pub const AVAILABILITY_DISABLED: i32 = 1;
pub const AVAILABILITY_NO_PERMISSION: i32 = 2;

const DISABLED_REASON: &str = "Wi-Fi Aware is turned off on this device";
const NO_PERMISSION_REASON: &str = "Wi-Fi Aware needs the nearby-devices permission";

fn role_of(is_initiator: bool) -> NdpRole {
    if is_initiator {
        NdpRole::Initiator
    } else {
        NdpRole::Responder
    }
}

enum Event {
    PeerDiscovered {
        peer: RendezvousToken,
    },
    NdpRequested {
        peer: RendezvousToken,
    },
    DataPathUp {
        peer: RendezvousToken,
        role: NdpRole,
        endpoint: AwareEndpoint,
    },
    DataPathDown {
        peer: RendezvousToken,
        role: NdpRole,
    },
    NdpFailed {
        peer: RendezvousToken,
        role: NdpRole,
    },
    Availability(Availability),
}

#[derive(Clone, Copy, Default)]
struct Desired {
    discovery: bool,
}

struct Shared {
    local: RendezvousToken,
    desired: Mutex<Desired>,
    requests: Mutex<VecDeque<(RendezvousToken, NdpRole)>>,
    abandons: Mutex<VecDeque<(RendezvousToken, NdpRole)>>,
    events: Mutex<VecDeque<Event>>,
    events_ready: Notify,
}

/// The Rust half of the Android Wi-Fi Aware bridge: the Kotlin `WifiAwareLink` pushes fabric events
/// in (discoveries, inbound requests, settled and lost data paths) and drains the desired discovery
/// state plus the per-peer request/abandon queues the policy produced. It owns the node's rendezvous
/// token so the value published on the air and the value the keeper duel ranks are the same one.
pub struct AndroidWifiAwareBridge {
    shared: Arc<Shared>,
}

impl Clone for AndroidWifiAwareBridge {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Default for AndroidWifiAwareBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl AndroidWifiAwareBridge {
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                local: RendezvousToken::new(fresh_token()),
                desired: Mutex::new(Desired::default()),
                requests: Mutex::new(VecDeque::new()),
                abandons: Mutex::new(VecDeque::new()),
                events: Mutex::new(VecDeque::new()),
                events_ready: Notify::new(),
            }),
        }
    }

    #[must_use]
    pub fn local_token(&self) -> u32 {
        self.shared.local.value()
    }

    pub fn peer_discovered(&self, peer: u32) {
        self.push(Event::PeerDiscovered {
            peer: RendezvousToken::new(peer),
        });
    }

    pub fn ndp_requested(&self, peer: u32) {
        self.push(Event::NdpRequested {
            peer: RendezvousToken::new(peer),
        });
    }

    pub fn data_path_up(&self, peer: u32, is_initiator: bool, addr: [u8; 16], scope: u32) {
        self.push(Event::DataPathUp {
            peer: RendezvousToken::new(peer),
            role: role_of(is_initiator),
            endpoint: AwareEndpoint {
                addr: Ipv6Addr::from(addr),
                scope,
                port: AWARE_RENDEZVOUS_PORT,
            },
        });
    }

    pub fn data_path_down(&self, peer: u32, is_initiator: bool) {
        self.push(Event::DataPathDown {
            peer: RendezvousToken::new(peer),
            role: role_of(is_initiator),
        });
    }

    pub fn ndp_failed(&self, peer: u32, is_initiator: bool) {
        self.push(Event::NdpFailed {
            peer: RendezvousToken::new(peer),
            role: role_of(is_initiator),
        });
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
    pub fn take_request(&self) -> Option<(RendezvousToken, NdpRole)> {
        self.shared
            .requests
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop_front())
    }

    #[must_use]
    pub fn take_abandon(&self) -> Option<(RendezvousToken, NdpRole)> {
        self.shared
            .abandons
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop_front())
    }

    fn set_discovery(&self, discovery: bool) {
        if let Ok(mut desired) = self.shared.desired.lock() {
            desired.discovery = discovery;
        }
    }

    fn enqueue_request(&self, peer: RendezvousToken, role: NdpRole) {
        if let Ok(mut queue) = self.shared.requests.lock() {
            queue.push_back((peer, role));
        }
    }

    fn enqueue_abandon(&self, peer: RendezvousToken, role: NdpRole) {
        if let Ok(mut queue) = self.shared.abandons.lock() {
            queue.push_back((peer, role));
        }
    }
}

/// A per-boot rendezvous token seeded from the wall clock. Two devices launch at distinct instants, so
/// their tokens differ with overwhelming probability — the distinctness the keeper duel relies on.
fn fresh_token() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let folded = (nanos as u32) ^ ((nanos >> 32) as u32) ^ ((nanos >> 64) as u32);
    folded.max(1)
}

pub struct AndroidWifiAwareBackend {
    bridge: AndroidWifiAwareBridge,
}

impl AndroidWifiAwareBackend {
    #[must_use]
    pub fn new(bridge: AndroidWifiAwareBridge) -> Self {
        Self { bridge }
    }
}

#[derive(Debug)]
pub enum AndroidWifiAwareError {}

impl WifiAwareBackend for AndroidWifiAwareBackend {
    type Error = AndroidWifiAwareError;

    fn local_token(&self) -> RendezvousToken {
        RendezvousToken::new(self.bridge.local_token())
    }

    async fn set_discovery(&mut self, mode: DiscoveryMode) -> Result<(), Self::Error> {
        self.bridge.set_discovery(matches!(mode, DiscoveryMode::On));
        Ok(())
    }

    async fn request_data_path(&mut self, peer: RendezvousToken, role: NdpRole) {
        self.bridge.enqueue_request(peer, role);
    }

    async fn abandon_data_path(&mut self, peer: RendezvousToken, role: NdpRole) {
        self.bridge.enqueue_abandon(peer, role);
    }

    async fn next_event(&mut self) -> WifiAwareEvent {
        loop {
            let event = self
                .bridge
                .shared
                .events
                .lock()
                .ok()
                .and_then(|mut events| events.pop_front());
            match event {
                Some(Event::PeerDiscovered { peer }) => {
                    return WifiAwareEvent::PeerDiscovered { peer };
                }
                Some(Event::NdpRequested { peer }) => {
                    return WifiAwareEvent::NdpRequested { peer };
                }
                Some(Event::DataPathUp {
                    peer,
                    role,
                    endpoint,
                }) => {
                    return WifiAwareEvent::DataPathUp {
                        peer,
                        role,
                        endpoint,
                    };
                }
                Some(Event::DataPathDown { peer, role }) => {
                    return WifiAwareEvent::DataPathDown {
                        peer,
                        role,
                        reason: NdpEndReason::LinkLost,
                    };
                }
                Some(Event::NdpFailed { peer, role }) => {
                    return WifiAwareEvent::NdpFailed { peer, role };
                }
                Some(Event::Availability(state)) => {
                    return WifiAwareEvent::AvailabilityChanged(state);
                }
                None => self.bridge.shared.events_ready.notified().await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desired_flag_and_request_queues_round_trip() {
        let bridge = AndroidWifiAwareBridge::new();
        assert!(!bridge.desired_discovery());
        bridge.set_discovery(true);
        assert!(bridge.desired_discovery());

        bridge.enqueue_request(RendezvousToken::new(7), NdpRole::Initiator);
        bridge.enqueue_request(RendezvousToken::new(9), NdpRole::Responder);
        assert_eq!(
            bridge.take_request(),
            Some((RendezvousToken::new(7), NdpRole::Initiator))
        );
        assert_eq!(
            bridge.take_request(),
            Some((RendezvousToken::new(9), NdpRole::Responder))
        );
        assert_eq!(bridge.take_request(), None);

        bridge.enqueue_abandon(RendezvousToken::new(7), NdpRole::Responder);
        assert_eq!(
            bridge.take_abandon(),
            Some((RendezvousToken::new(7), NdpRole::Responder))
        );
        assert_eq!(bridge.take_abandon(), None);
    }

    #[test]
    fn the_local_token_is_nonzero_and_stable() {
        let bridge = AndroidWifiAwareBridge::new();
        let token = bridge.local_token();
        assert_ne!(token, 0);
        assert_eq!(bridge.local_token(), token);
    }
}
