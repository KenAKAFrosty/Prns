use super::backend::{AdvertisingMode, Origin, ScanningMode};
use super::framing::BLE_HW_MTU;
use super::handshake::{
    is_keeper, l2cap_arrangement, l2cap_plan, needs_redial, EstablishedPeer, EstablishedTransport,
    HandshakeRole, L2capPlan, LocalPeer,
};
use super::identity::{BleAddress, BleIdentity};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EgressCapability,
    IngressCapability, InterfaceCapabilities, InterfaceDefaults, InterfaceDescriptor, InterfaceId,
    InterfaceMode, MtuPolicy, TransportCapability,
};

pub const BLE_BITRATE_GUESS_BPS: BitrateBps = BitrateBps::guess(700_000);

pub const SUPPRESS_TTL_MS: u64 = 8_000;
pub const DIAL_RETRY_TTL_MS: u64 = 16_000;
pub const DIAL_FAILED_RETRY_TTL_MS: u64 = 5_000;
pub const DIAL_PAUSE_MS: u64 = 15_000;
pub const KEEPER_DUEL_WINDOW_MS: u64 = 5_000;
pub const HANDSHAKE_SLACK: usize = 4;

#[must_use]
pub fn role_for(origin: Origin) -> HandshakeRole {
    match origin {
        Origin::Dialed => HandshakeRole::Dialer,
        Origin::Accepted => HandshakeRole::Listener,
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PolicyInput {
    Sighting {
        address: BleAddress,
        now_ms: u64,
    },
    Settled {
        address: BleAddress,
        origin: Origin,
        established: EstablishedPeer,
        now_ms: u64,
    },
    HandshakeFailed {
        address: BleAddress,
        origin: Origin,
    },
    DialFailed {
        address: BleAddress,
        now_ms: u64,
    },
    Closed {
        identity: BleIdentity,
        address: BleAddress,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    Dial(BleAddress),
    Admit {
        identity: BleIdentity,
        slot: usize,
        address: BleAddress,
        lane: L2capPlan,
        peer_rssi: Option<i8>,
    },
    Evict {
        identity: BleIdentity,
        slot: usize,
    },
    Reject {
        address: BleAddress,
        dialed: bool,
    },
    NotifyClosed(BleAddress),
    SetAdvertising(AdvertisingMode),
    SetScanning(ScanningMode),
}

#[derive(Clone, Copy)]
struct SettledSlot {
    identity: BleIdentity,
    keeper: bool,
    address: BleAddress,
    settled_at_ms: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackoffKind {
    Dialing,
    Suppressed,
    FailedDial,
}

#[derive(Clone, Copy)]
struct Backoff {
    address: BleAddress,
    kind: BackoffKind,
    since_ms: u64,
}

impl Backoff {
    fn ttl_ms(self) -> u64 {
        match self.kind {
            BackoffKind::Dialing => DIAL_RETRY_TTL_MS,
            BackoffKind::Suppressed => SUPPRESS_TTL_MS,
            BackoffKind::FailedDial => DIAL_FAILED_RETRY_TTL_MS,
        }
    }

    fn elapsed(self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.since_ms) >= self.ttl_ms()
    }
}

pub struct ConnectionPolicy<const MAX_PEERS: usize, const DIAL_TRACK: usize> {
    local: LocalPeer,
    settled: [Option<SettledSlot>; MAX_PEERS],
    backoff: [Option<Backoff>; DIAL_TRACK],
    advertising: bool,
    scanning: bool,
    dial_pause_until_ms: u64,
    handshaking: usize,
}

impl<const MAX_PEERS: usize, const DIAL_TRACK: usize> ConnectionPolicy<MAX_PEERS, DIAL_TRACK> {
    #[must_use]
    pub const fn new(local: LocalPeer) -> Self {
        Self {
            local,
            settled: [None; MAX_PEERS],
            backoff: [None; DIAL_TRACK],
            advertising: false,
            scanning: false,
            dial_pause_until_ms: 0,
            handshaking: 0,
        }
    }

    #[must_use]
    pub fn settled_count(&self) -> usize {
        self.settled.iter().filter(|slot| slot.is_some()).count()
    }

    pub fn start<F: FnMut(PolicyAction)>(&mut self, emit: &mut F) {
        self.reconcile(emit);
    }

    #[must_use]
    pub fn begin_handshake(&mut self, origin: Origin) -> bool {
        if matches!(origin, Origin::Accepted)
            && self.handshaking + self.settled_count() >= MAX_PEERS + HANDSHAKE_SLACK
        {
            return false;
        }
        self.handshaking += 1;
        true
    }

    pub fn handle<F: FnMut(PolicyAction)>(&mut self, input: PolicyInput, emit: &mut F) {
        match input {
            PolicyInput::Sighting { address, now_ms } => self.on_sighting(address, now_ms, emit),
            PolicyInput::Settled {
                address,
                origin,
                established,
                now_ms,
            } => self.on_settled(address, origin, established, now_ms, emit),
            PolicyInput::HandshakeFailed { address, origin } => {
                self.on_handshake_failed(address, origin, emit);
            }
            PolicyInput::DialFailed { address, now_ms } => self.on_dial_failed(address, now_ms),
            PolicyInput::Closed { identity, address } => self.on_closed(identity, address, emit),
        }
    }

    fn on_sighting<F: FnMut(PolicyAction)>(
        &mut self,
        address: BleAddress,
        now_ms: u64,
        emit: &mut F,
    ) {
        let dialable = self.settled_count() < MAX_PEERS
            && now_ms >= self.dial_pause_until_ms
            && self.find_settled_by_address(address).is_none()
            && self.backoff_ready(address, now_ms);
        if dialable {
            self.upsert_backoff(address, BackoffKind::Dialing, now_ms);
            emit(PolicyAction::Dial(address));
        }
    }

    fn on_settled<F: FnMut(PolicyAction)>(
        &mut self,
        address: BleAddress,
        origin: Origin,
        established: EstablishedPeer,
        now_ms: u64,
        emit: &mut F,
    ) {
        self.handshaking = self.handshaking.saturating_sub(1);
        let dialed = matches!(origin, Origin::Dialed);
        if dialed {
            self.clear_backoff(address);
        }
        let identity = established.identity;
        let role = role_for(origin);
        let (plan, can_upgrade) = match established.transport {
            EstablishedTransport::Native {
                endpoint,
                capabilities,
            } => (
                l2cap_arrangement(self.local.endpoint, endpoint),
                self.local.capabilities.l2cap.is_some() && capabilities.l2cap.is_some(),
            ),
            EstablishedTransport::ColumbaGatt => {
                (super::handshake::L2capArrangement::GattOnly, false)
            }
        };
        if can_upgrade
            && needs_redial(plan, role, self.local.endpoint)
            && self.find_settled_by_identity(identity).is_none()
            && self.settled_count() < MAX_PEERS
        {
            let we_open = matches!(
                plan,
                super::handshake::L2capArrangement::Opens(opener) if opener == self.local.endpoint
            );
            if dialed {
                // Non-opener who dialed: pause outbound so the designated opener can take central.
                self.upsert_backoff(address, BackoffKind::Suppressed, now_ms);
                self.dial_pause_until_ms = now_ms.saturating_add(DIAL_PAUSE_MS);
            }
            emit(PolicyAction::Reject { address, dialed });
            if we_open && !dialed {
                // Opener was peripheral: redial as central on the address we already have.
                self.upsert_backoff(address, BackoffKind::Dialing, now_ms);
                emit(PolicyAction::Dial(address));
            }
            return;
        }
        let keeper = is_keeper(
            plan,
            role,
            self.local.identity,
            self.local.endpoint,
            identity,
        );

        if let Some(existing) = self.find_settled_by_identity(identity) {
            let Some(incumbent) = self.settled[existing] else {
                return;
            };
            let incumbent_recent =
                now_ms.saturating_sub(incumbent.settled_at_ms) < KEEPER_DUEL_WINDOW_MS;
            let challenger_wins = keeper && !incumbent.keeper && incumbent_recent;
            if !challenger_wins {
                self.upsert_backoff(address, BackoffKind::Suppressed, now_ms);
                if dialed {
                    self.dial_pause_until_ms = now_ms.saturating_add(DIAL_PAUSE_MS);
                }
                emit(PolicyAction::Reject { address, dialed });
                return;
            }
            self.settled[existing] = None;
            emit(PolicyAction::Evict {
                identity: incumbent.identity,
                slot: existing,
            });
        } else if self.settled_count() >= MAX_PEERS {
            self.upsert_backoff(address, BackoffKind::Suppressed, now_ms);
            emit(PolicyAction::Reject { address, dialed });
            return;
        }

        let Some(slot) = self.first_free_settled() else {
            self.upsert_backoff(address, BackoffKind::Suppressed, now_ms);
            emit(PolicyAction::Reject { address, dialed });
            return;
        };
        let lane = match established.transport {
            EstablishedTransport::Native { capabilities, .. }
                if keeper || matches!(plan, super::handshake::L2capArrangement::EitherOpens) =>
            {
                l2cap_plan(
                    plan,
                    role,
                    self.local.endpoint,
                    &self.local.capabilities,
                    &capabilities,
                )
            }
            EstablishedTransport::Native { .. } | EstablishedTransport::ColumbaGatt => {
                L2capPlan::None
            }
        };
        self.settled[slot] = Some(SettledSlot {
            identity,
            keeper,
            address,
            settled_at_ms: now_ms,
        });
        emit(PolicyAction::Admit {
            identity,
            slot,
            address,
            lane,
            peer_rssi: established.peer_rssi,
        });
        self.reconcile(emit);
    }

    fn on_handshake_failed<F: FnMut(PolicyAction)>(
        &mut self,
        address: BleAddress,
        origin: Origin,
        emit: &mut F,
    ) {
        self.handshaking = self.handshaking.saturating_sub(1);
        if matches!(origin, Origin::Dialed) {
            self.clear_backoff(address);
        }
        emit(PolicyAction::NotifyClosed(address));
    }

    fn on_dial_failed(&mut self, address: BleAddress, now_ms: u64) {
        self.upsert_backoff(address, BackoffKind::FailedDial, now_ms);
    }

    fn on_closed<F: FnMut(PolicyAction)>(
        &mut self,
        identity: BleIdentity,
        address: BleAddress,
        emit: &mut F,
    ) {
        if let Some(slot) = self.find_settled_by_identity(identity) {
            if self.settled[slot].is_some_and(|peer| peer.address == address) {
                self.settled[slot] = None;
            }
        }
        if self.settled_count() == 0 {
            self.dial_pause_until_ms = 0;
        }
        emit(PolicyAction::NotifyClosed(address));
        self.reconcile(emit);
    }

    fn reconcile<F: FnMut(PolicyAction)>(&mut self, emit: &mut F) {
        let want = self.settled_count() < MAX_PEERS;
        if want != self.advertising {
            self.advertising = want;
            emit(PolicyAction::SetAdvertising(if want {
                AdvertisingMode::On
            } else {
                AdvertisingMode::Off
            }));
        }
        if want != self.scanning {
            self.scanning = want;
            emit(PolicyAction::SetScanning(if want {
                ScanningMode::On
            } else {
                ScanningMode::Off
            }));
        }
    }

    fn find_settled_by_identity(&self, identity: BleIdentity) -> Option<usize> {
        self.settled
            .iter()
            .position(|slot| slot.is_some_and(|peer| peer.identity == identity))
    }

    fn find_settled_by_address(&self, address: BleAddress) -> Option<usize> {
        self.settled
            .iter()
            .position(|slot| slot.is_some_and(|peer| peer.address == address))
    }

    fn first_free_settled(&self) -> Option<usize> {
        self.settled.iter().position(Option::is_none)
    }

    fn backoff_ready(&self, address: BleAddress, now_ms: u64) -> bool {
        match self.find_backoff(address) {
            Some(index) => self.backoff[index].is_none_or(|backoff| backoff.elapsed(now_ms)),
            None => true,
        }
    }

    fn find_backoff(&self, address: BleAddress) -> Option<usize> {
        self.backoff
            .iter()
            .position(|entry| entry.is_some_and(|b| b.address == address))
    }

    fn clear_backoff(&mut self, address: BleAddress) {
        if let Some(index) = self.find_backoff(address) {
            self.backoff[index] = None;
        }
    }

    fn upsert_backoff(&mut self, address: BleAddress, kind: BackoffKind, now_ms: u64) {
        let entry = Backoff {
            address,
            kind,
            since_ms: now_ms,
        };
        if let Some(index) = self.find_backoff(address) {
            self.backoff[index] = Some(entry);
            return;
        }
        if let Some(index) = self.backoff.iter().position(Option::is_none) {
            self.backoff[index] = Some(entry);
            return;
        }
        self.prune_backoff(now_ms);
        let slot = self.backoff.iter().position(Option::is_none).or_else(|| {
            self.backoff
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.map_or(u64::MAX, |b| b.since_ms))
                .map(|(index, _)| index)
        });
        if let Some(index) = slot {
            self.backoff[index] = Some(entry);
        }
    }

    fn prune_backoff(&mut self, now_ms: u64) {
        for entry in &mut self.backoff {
            if entry.is_some_and(|b| b.elapsed(now_ms)) {
                *entry = None;
            }
        }
    }
}

pub fn descriptor(id: InterfaceId, bitrate: BitrateBps) -> InterfaceDescriptor {
    defaults_for_bitrate(bitrate)
        .configured(ConfiguredInterfacePolicy::default())
        .descriptor(id)
}

pub fn defaults_for_bitrate(bitrate: BitrateBps) -> InterfaceDefaults {
    InterfaceDefaults {
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
        },
        mode: InterfaceMode::Full,
        gravity: crate::interfaces::InterfaceGravity::ZERO,
        bitrate,
        mtu: MtuPolicy::fixed(BLE_HW_MTU),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::bluetooth_auto::{Endpoint, LinkCapabilities, Nrf52Host};

    const CAPS: LinkCapabilities = LinkCapabilities {
        l2cap: None,
        link_mtu: 500,
    };

    fn endpoint() -> Endpoint {
        Endpoint::Nrf52(Nrf52Host::Nrf52)
    }

    fn local(identity: u8) -> LocalPeer {
        LocalPeer {
            identity: BleIdentity::new([identity; 16]),
            endpoint: endpoint(),
            capabilities: CAPS,
        }
    }

    fn established(identity: u8) -> EstablishedPeer {
        EstablishedPeer {
            identity: BleIdentity::new([identity; 16]),
            transport: EstablishedTransport::Native {
                endpoint: endpoint(),
                capabilities: CAPS,
            },
            peer_rssi: None,
        }
    }

    fn addr(byte: u8) -> BleAddress {
        BleAddress::new([byte; 6])
    }

    fn collect<const M: usize, const D: usize>(
        manager: &mut ConnectionPolicy<M, D>,
        input: PolicyInput,
    ) -> std::vec::Vec<PolicyAction> {
        let mut actions = std::vec::Vec::new();
        manager.handle(input, &mut |action| actions.push(action));
        actions
    }

    #[test]
    fn role_is_derived_from_origin() {
        assert_eq!(role_for(Origin::Dialed), HandshakeRole::Dialer);
        assert_eq!(role_for(Origin::Accepted), HandshakeRole::Listener);
    }

    #[test]
    fn start_brings_radio_up_with_capacity() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(1));
        let mut actions = std::vec::Vec::new();
        manager.start(&mut |action| actions.push(action));
        assert_eq!(
            actions,
            std::vec![
                PolicyAction::SetAdvertising(AdvertisingMode::On),
                PolicyAction::SetScanning(ScanningMode::On)
            ]
        );
    }

    #[test]
    fn sighting_dials_then_backs_off_until_ttl() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(1));
        manager.start(&mut |_| {});

        let first = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(9),
                now_ms: 0,
            },
        );
        assert_eq!(first, std::vec![PolicyAction::Dial(addr(9))]);

        let within = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(9),
                now_ms: 1_000,
            },
        );
        assert!(within.is_empty());

        let after = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(9),
                now_ms: DIAL_RETRY_TTL_MS,
            },
        );
        assert_eq!(after, std::vec![PolicyAction::Dial(addr(9))]);
    }

    #[test]
    fn a_failed_dial_uses_a_short_recovery_backoff() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(1));
        manager.start(&mut |_| {});

        let dialed = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(9),
                now_ms: 0,
            },
        );
        assert_eq!(dialed, std::vec![PolicyAction::Dial(addr(9))]);

        manager.handle(
            PolicyInput::DialFailed {
                address: addr(9),
                now_ms: 100,
            },
            &mut |_| {},
        );

        let within = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(9),
                now_ms: DIAL_FAILED_RETRY_TTL_MS - 1,
            },
        );
        assert!(
            within.is_empty(),
            "a failed dial still gets a brief radio-recovery backoff"
        );

        let after = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(9),
                now_ms: DIAL_FAILED_RETRY_TTL_MS + 200,
            },
        );
        assert_eq!(after, std::vec![PolicyAction::Dial(addr(9))]);
    }

    #[test]
    fn admit_fills_the_only_slot_and_stops_radio() {
        let mut manager = ConnectionPolicy::<1, 8>::new(local(1));
        manager.start(&mut |_| {});

        let actions = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(2),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        assert_eq!(
            actions,
            std::vec![
                PolicyAction::Admit {
                    identity: BleIdentity::new([2; 16]),
                    slot: 0,
                    address: addr(2),
                    lane: L2capPlan::None,
                    peer_rssi: None,
                },
                PolicyAction::SetAdvertising(AdvertisingMode::Off),
                PolicyAction::SetScanning(ScanningMode::Off),
            ]
        );
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn settle_past_capacity_is_rejected() {
        let mut manager = ConnectionPolicy::<1, 8>::new(local(1));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(2),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        let actions = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(3),
                origin: Origin::Dialed,
                established: established(3),
                now_ms: 0,
            },
        );
        assert_eq!(
            actions,
            std::vec![PolicyAction::Reject {
                address: addr(3),
                dialed: true
            }]
        );
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn closing_a_member_reopens_the_radio() {
        let mut manager = ConnectionPolicy::<1, 8>::new(local(1));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(2),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        let actions = collect(
            &mut manager,
            PolicyInput::Closed {
                identity: BleIdentity::new([2; 16]),
                address: addr(2),
            },
        );
        assert_eq!(
            actions,
            std::vec![
                PolicyAction::NotifyClosed(addr(2)),
                PolicyAction::SetAdvertising(AdvertisingMode::On),
                PolicyAction::SetScanning(ScanningMode::On),
            ]
        );
        assert_eq!(manager.settled_count(), 0);
    }

    #[test]
    fn a_designated_l2cap_opener_rejects_an_inbound_native_link_for_redial() {
        use crate::interfaces::bluetooth_auto::{AndroidHost, AppleHost, Psm};

        let capabilities = LinkCapabilities {
            l2cap: Psm::new(0x0080),
            link_mtu: 500,
        };

        let mut manager = ConnectionPolicy::<2, 8>::new(LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: Endpoint::Android(AndroidHost::Android),
            capabilities,
        });
        manager.start(&mut |_| {});

        let actions = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(2),
                origin: Origin::Accepted,
                established: EstablishedPeer {
                    identity: BleIdentity::new([2; 16]),
                    transport: EstablishedTransport::Native {
                        endpoint: Endpoint::CoreBluetooth(AppleHost::MacOs),
                        capabilities,
                    },
                    peer_rssi: None,
                },
                now_ms: 5,
            },
        );

        assert_eq!(
            actions,
            std::vec![
                PolicyAction::Reject {
                    address: addr(2),
                    dialed: false,
                },
                PolicyAction::Dial(addr(2)),
            ]
        );
        assert_eq!(manager.settled_count(), 0);
    }

    #[test]
    fn a_non_opener_who_dialed_rejects_and_pauses_outbound() {
        use crate::interfaces::bluetooth_auto::{AndroidHost, AppleHost, Psm};

        let capabilities = LinkCapabilities {
            l2cap: Psm::new(0x00c0),
            link_mtu: 500,
        };

        let mut manager = ConnectionPolicy::<2, 8>::new(LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: Endpoint::CoreBluetooth(AppleHost::MacOs),
            capabilities,
        });
        manager.start(&mut |_| {});

        let actions = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(2),
                origin: Origin::Dialed,
                established: EstablishedPeer {
                    identity: BleIdentity::new([2; 16]),
                    transport: EstablishedTransport::Native {
                        endpoint: Endpoint::Android(AndroidHost::Android),
                        capabilities,
                    },
                    peer_rssi: None,
                },
                now_ms: 5,
            },
        );

        assert_eq!(
            actions,
            std::vec![PolicyAction::Reject {
                address: addr(2),
                dialed: true,
            }]
        );
        assert_eq!(manager.settled_count(), 0);

        let while_paused = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(2),
                now_ms: 6,
            },
        );
        assert!(
            while_paused.is_empty(),
            "non-opener must pause dialing so the opener can take central"
        );

        let after_pause = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(2),
                now_ms: 5 + DIAL_PAUSE_MS,
            },
        );
        assert_eq!(after_pause, std::vec![PolicyAction::Dial(addr(2))]);
    }

    #[test]
    fn duplicate_link_keeper_evicts_incumbent() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(1));
        manager.start(&mut |_| {});

        let admit = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        assert!(matches!(admit[0], PolicyAction::Admit { slot: 0, .. }));

        let resolve = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 0,
            },
        );
        assert_eq!(
            resolve[0],
            PolicyAction::Evict {
                identity: BleIdentity::new([2; 16]),
                slot: 0,
            }
        );
        assert!(matches!(
            resolve[1],
            PolicyAction::Admit {
                address,
                ..
            } if address == addr(11)
        ));
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn duplicate_link_loser_is_rejected() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(9));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        let resolve = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 0,
            },
        );
        assert_eq!(
            resolve,
            std::vec![PolicyAction::Reject {
                address: addr(11),
                dialed: true
            }]
        );
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn either_opens_uses_both_physical_roles_and_keeper_wins_duplicate() {
        use crate::interfaces::bluetooth_auto::{Esp32Host, Psm};
        let l2cap_caps = LinkCapabilities {
            l2cap: Psm::new(0x0080),
            link_mtu: 247,
        };
        let me = LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: Endpoint::Esp32(Esp32Host::Esp32),
            capabilities: l2cap_caps,
        };
        let mut manager = ConnectionPolicy::<2, 8>::new(me);
        manager.start(&mut |_| {});

        let peer = EstablishedPeer {
            identity: BleIdentity::new([2; 16]),
            transport: EstablishedTransport::Native {
                endpoint: Endpoint::Esp32(Esp32Host::Esp32),
                capabilities: l2cap_caps,
            },
            peer_rssi: None,
        };

        let admit = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: peer,
                now_ms: 0,
            },
        );
        assert!(
            matches!(
                admit[0],
                PolicyAction::Admit {
                    lane: L2capPlan::Accept,
                    ..
                }
            ),
            "the accepted EitherOpens link accepts the central's L2CAP fast lane"
        );

        let resolve = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: peer,
                now_ms: 0,
            },
        );
        assert!(matches!(resolve[0], PolicyAction::Evict { .. }));
        assert!(
            matches!(
                resolve[1],
                PolicyAction::Admit {
                    lane: L2capPlan::Open { .. },
                    ..
                }
            ),
            "the keeper (dialed, we are central) link opens the L2CAP fast lane"
        );
    }

    #[test]
    fn a_late_duplicate_keeps_the_stable_link_instead_of_evicting() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(1));
        manager.start(&mut |_| {});
        let admit = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        assert!(matches!(admit[0], PolicyAction::Admit { slot: 0, .. }));

        let resolve = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: KEEPER_DUEL_WINDOW_MS + 1,
            },
        );
        assert_eq!(
            resolve,
            std::vec![PolicyAction::Reject {
                address: addr(11),
                dialed: true
            }]
        );
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn dialing_pauses_after_chasing_a_duplicate() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(9));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        let reject = collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 1_000,
            },
        );
        assert_eq!(
            reject,
            std::vec![PolicyAction::Reject {
                address: addr(11),
                dialed: true
            }]
        );

        let paused = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(12),
                now_ms: 1_100,
            },
        );
        assert!(
            paused.is_empty(),
            "a fresh address is not chased while paused"
        );

        let resumed = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(13),
                now_ms: 1_000 + DIAL_PAUSE_MS,
            },
        );
        assert_eq!(resumed, std::vec![PolicyAction::Dial(addr(13))]);
    }

    #[test]
    fn a_member_close_clears_the_dial_pause() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(9));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 1_000,
            },
        );
        collect(
            &mut manager,
            PolicyInput::Closed {
                identity: BleIdentity::new([2; 16]),
                address: addr(10),
            },
        );
        let dialed = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(12),
                now_ms: 1_100,
            },
        );
        assert_eq!(dialed, std::vec![PolicyAction::Dial(addr(12))]);
    }

    #[test]
    fn an_unrelated_close_keeps_the_dial_pause_while_peers_remain() {
        let mut manager = ConnectionPolicy::<4, 8>::new(local(9));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(20),
                origin: Origin::Accepted,
                established: established(3),
                now_ms: 0,
            },
        );
        collect(
            &mut manager,
            PolicyInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 1_000,
            },
        );
        collect(
            &mut manager,
            PolicyInput::Closed {
                identity: BleIdentity::new([3; 16]),
                address: addr(20),
            },
        );
        let sighting = collect(
            &mut manager,
            PolicyInput::Sighting {
                address: addr(30),
                now_ms: 2_000,
            },
        );
        assert!(
            sighting.is_empty(),
            "an unrelated peer's close must not re-open the dial pause while peers remain"
        );
    }

    #[test]
    fn an_accepted_handshake_failure_notifies_the_backend_to_clean_up() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(1));
        manager.start(&mut |_| {});
        let actions = collect(
            &mut manager,
            PolicyInput::HandshakeFailed {
                address: addr(7),
                origin: Origin::Accepted,
            },
        );
        assert_eq!(actions, std::vec![PolicyAction::NotifyClosed(addr(7))]);
    }

    #[test]
    fn the_handshake_gate_bounds_an_inbound_flood() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(1));
        for _ in 0..(2 + HANDSHAKE_SLACK) {
            assert!(manager.begin_handshake(Origin::Accepted));
        }
        assert!(
            !manager.begin_handshake(Origin::Accepted),
            "an inbound flood past the budget is refused, not spawned"
        );
        assert!(
            manager.begin_handshake(Origin::Dialed),
            "our own dials are never gated here"
        );
    }

    #[test]
    fn a_completed_handshake_frees_a_gate_slot() {
        let mut manager = ConnectionPolicy::<2, 8>::new(local(1));
        for _ in 0..(2 + HANDSHAKE_SLACK) {
            assert!(manager.begin_handshake(Origin::Accepted));
        }
        assert!(!manager.begin_handshake(Origin::Accepted));
        manager.handle(
            PolicyInput::HandshakeFailed {
                address: addr(7),
                origin: Origin::Accepted,
            },
            &mut |_| {},
        );
        assert!(
            manager.begin_handshake(Origin::Accepted),
            "a completed handshake frees an in-flight slot"
        );
    }
}
