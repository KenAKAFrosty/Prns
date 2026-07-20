use super::core::{
    is_keeper, l2cap_arrangement, l2cap_plan, needs_redial, BleAddress, BleIdentity,
    EstablishedPeer, EstablishedTransport, HandshakeRole, L2capPlan, LocalPeer,
};
use super::seam::{AdvertisingMode, Origin, ScanningMode};

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
pub enum ManagerInput {
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
pub enum ManagerAction {
    Dial(BleAddress),
    Admit {
        identity: BleIdentity,
        slot: usize,
        address: BleAddress,
        lane: L2capPlan,
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

pub struct ConnectionManager<const MAX_PEERS: usize, const DIAL_TRACK: usize> {
    local: LocalPeer,
    settled: [Option<SettledSlot>; MAX_PEERS],
    backoff: [Option<Backoff>; DIAL_TRACK],
    advertising: bool,
    scanning: bool,
    dial_pause_until_ms: u64,
    handshaking: usize,
}

impl<const MAX_PEERS: usize, const DIAL_TRACK: usize> ConnectionManager<MAX_PEERS, DIAL_TRACK> {
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

    pub fn start<F: FnMut(ManagerAction)>(&mut self, emit: &mut F) {
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

    pub fn handle<F: FnMut(ManagerAction)>(&mut self, input: ManagerInput, emit: &mut F) {
        match input {
            ManagerInput::Sighting { address, now_ms } => self.on_sighting(address, now_ms, emit),
            ManagerInput::Settled {
                address,
                origin,
                established,
                now_ms,
            } => self.on_settled(address, origin, established, now_ms, emit),
            ManagerInput::HandshakeFailed { address, origin } => {
                self.on_handshake_failed(address, origin, emit);
            }
            ManagerInput::DialFailed { address, now_ms } => self.on_dial_failed(address, now_ms),
            ManagerInput::Closed { identity, address } => self.on_closed(identity, address, emit),
        }
    }

    fn on_sighting<F: FnMut(ManagerAction)>(
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
            emit(ManagerAction::Dial(address));
        }
    }

    fn on_settled<F: FnMut(ManagerAction)>(
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
            EstablishedTransport::ColumbaGatt => (super::core::L2capArrangement::GattOnly, false),
        };
        if can_upgrade
            && needs_redial(plan, role, self.local.endpoint)
            && self.find_settled_by_identity(identity).is_none()
            && self.settled_count() < MAX_PEERS
        {
            emit(ManagerAction::Reject {
                address,
                dialed: false,
            });
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
                emit(ManagerAction::Reject { address, dialed });
                return;
            }
            self.settled[existing] = None;
            emit(ManagerAction::Evict {
                identity: incumbent.identity,
                slot: existing,
            });
        } else if self.settled_count() >= MAX_PEERS {
            self.upsert_backoff(address, BackoffKind::Suppressed, now_ms);
            emit(ManagerAction::Reject { address, dialed });
            return;
        }

        let Some(slot) = self.first_free_settled() else {
            self.upsert_backoff(address, BackoffKind::Suppressed, now_ms);
            emit(ManagerAction::Reject { address, dialed });
            return;
        };
        // Only the keeper connection opens the L2CAP fast lane. In a dual-dial both boards transiently
        // admit their own outbound first; opening L2CAP there would race two creates on different
        // physical connections (neither side accepts) until the duel evicts down to the keeper, by
        // which point the setup window has usually lapsed. Gating on `keeper` means both ends only ever
        // attempt L2CAP on the same surviving connection; a non-keeper link rides the GATT floor.
        let lane = match (keeper, established.transport) {
            (true, EstablishedTransport::Native { capabilities, .. }) => l2cap_plan(
                plan,
                role,
                self.local.endpoint,
                &self.local.capabilities,
                &capabilities,
            ),
            (_, EstablishedTransport::ColumbaGatt)
            | (false, EstablishedTransport::Native { .. }) => L2capPlan::None,
        };
        self.settled[slot] = Some(SettledSlot {
            identity,
            keeper,
            address,
            settled_at_ms: now_ms,
        });
        emit(ManagerAction::Admit {
            identity,
            slot,
            address,
            lane,
        });
        self.reconcile(emit);
    }

    fn on_handshake_failed<F: FnMut(ManagerAction)>(
        &mut self,
        address: BleAddress,
        origin: Origin,
        emit: &mut F,
    ) {
        self.handshaking = self.handshaking.saturating_sub(1);
        if matches!(origin, Origin::Dialed) {
            self.clear_backoff(address);
        }
        emit(ManagerAction::NotifyClosed(address));
    }

    fn on_dial_failed(&mut self, address: BleAddress, now_ms: u64) {
        self.upsert_backoff(address, BackoffKind::FailedDial, now_ms);
    }

    fn on_closed<F: FnMut(ManagerAction)>(
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
        emit(ManagerAction::NotifyClosed(address));
        self.reconcile(emit);
    }

    fn reconcile<F: FnMut(ManagerAction)>(&mut self, emit: &mut F) {
        let want = self.settled_count() < MAX_PEERS;
        if want != self.advertising {
            self.advertising = want;
            emit(ManagerAction::SetAdvertising(if want {
                AdvertisingMode::On
            } else {
                AdvertisingMode::Off
            }));
        }
        if want != self.scanning {
            self.scanning = want;
            emit(ManagerAction::SetScanning(if want {
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
        // Table full: prune anything already expired, else evict the oldest. Dropping a backoff
        // entry only risks re-dialing a cooling-off peer a little early — never a correctness bug.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::bluetooth_auto::core::{Endpoint, LinkCapabilities, Nrf52Host};

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
        manager: &mut ConnectionManager<M, D>,
        input: ManagerInput,
    ) -> std::vec::Vec<ManagerAction> {
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
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
        let mut actions = std::vec::Vec::new();
        manager.start(&mut |action| actions.push(action));
        assert_eq!(
            actions,
            std::vec![
                ManagerAction::SetAdvertising(AdvertisingMode::On),
                ManagerAction::SetScanning(ScanningMode::On)
            ]
        );
    }

    #[test]
    fn sighting_dials_then_backs_off_until_ttl() {
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
        manager.start(&mut |_| {});

        let first = collect(
            &mut manager,
            ManagerInput::Sighting {
                address: addr(9),
                now_ms: 0,
            },
        );
        assert_eq!(first, std::vec![ManagerAction::Dial(addr(9))]);

        let within = collect(
            &mut manager,
            ManagerInput::Sighting {
                address: addr(9),
                now_ms: 1_000,
            },
        );
        assert!(within.is_empty());

        let after = collect(
            &mut manager,
            ManagerInput::Sighting {
                address: addr(9),
                now_ms: DIAL_RETRY_TTL_MS,
            },
        );
        assert_eq!(after, std::vec![ManagerAction::Dial(addr(9))]);
    }

    #[test]
    fn a_failed_dial_uses_a_short_recovery_backoff() {
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
        manager.start(&mut |_| {});

        let dialed = collect(
            &mut manager,
            ManagerInput::Sighting {
                address: addr(9),
                now_ms: 0,
            },
        );
        assert_eq!(dialed, std::vec![ManagerAction::Dial(addr(9))]);

        manager.handle(
            ManagerInput::DialFailed {
                address: addr(9),
                now_ms: 100,
            },
            &mut |_| {},
        );

        let within = collect(
            &mut manager,
            ManagerInput::Sighting {
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
            ManagerInput::Sighting {
                address: addr(9),
                now_ms: DIAL_FAILED_RETRY_TTL_MS + 200,
            },
        );
        assert_eq!(after, std::vec![ManagerAction::Dial(addr(9))]);
    }

    #[test]
    fn admit_fills_the_only_slot_and_stops_radio() {
        let mut manager = ConnectionManager::<1, 8>::new(local(1));
        manager.start(&mut |_| {});

        let actions = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(2),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        assert_eq!(
            actions,
            std::vec![
                ManagerAction::Admit {
                    identity: BleIdentity::new([2; 16]),
                    slot: 0,
                    address: addr(2),
                    lane: L2capPlan::None,
                },
                ManagerAction::SetAdvertising(AdvertisingMode::Off),
                ManagerAction::SetScanning(ScanningMode::Off),
            ]
        );
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn settle_past_capacity_is_rejected() {
        let mut manager = ConnectionManager::<1, 8>::new(local(1));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(2),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        let actions = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(3),
                origin: Origin::Dialed,
                established: established(3),
                now_ms: 0,
            },
        );
        assert_eq!(
            actions,
            std::vec![ManagerAction::Reject {
                address: addr(3),
                dialed: true
            }]
        );
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn closing_a_member_reopens_the_radio() {
        let mut manager = ConnectionManager::<1, 8>::new(local(1));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(2),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        let actions = collect(
            &mut manager,
            ManagerInput::Closed {
                identity: BleIdentity::new([2; 16]),
                address: addr(2),
            },
        );
        assert_eq!(
            actions,
            std::vec![
                ManagerAction::NotifyClosed(addr(2)),
                ManagerAction::SetAdvertising(AdvertisingMode::On),
                ManagerAction::SetScanning(ScanningMode::On),
            ]
        );
        assert_eq!(manager.settled_count(), 0);
    }

    #[test]
    fn a_designated_l2cap_opener_rejects_an_inbound_native_link_for_redial() {
        use crate::interfaces::bluetooth_auto::core::{AndroidHost, AppleHost, Psm};

        let capabilities = LinkCapabilities {
            l2cap: Psm::new(0x0080),
            link_mtu: 500,
        };

        let mut manager = ConnectionManager::<2, 8>::new(LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: Endpoint::Android(AndroidHost::Android),
            capabilities,
        });
        manager.start(&mut |_| {});

        let actions = collect(
            &mut manager,
            ManagerInput::Settled {
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
            std::vec![ManagerAction::Reject {
                address: addr(2),
                dialed: false,
            }]
        );
        assert_eq!(manager.settled_count(), 0);

        let redial = collect(
            &mut manager,
            ManagerInput::Sighting {
                address: addr(2),
                now_ms: 6,
            },
        );
        assert_eq!(redial, std::vec![ManagerAction::Dial(addr(2))]);
    }

    #[test]
    fn duplicate_link_keeper_evicts_incumbent() {
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
        manager.start(&mut |_| {});

        let admit = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        assert!(matches!(admit[0], ManagerAction::Admit { slot: 0, .. }));

        let resolve = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 0,
            },
        );
        assert_eq!(
            resolve[0],
            ManagerAction::Evict {
                identity: BleIdentity::new([2; 16]),
                slot: 0,
            }
        );
        assert!(matches!(
            resolve[1],
            ManagerAction::Admit {
                address,
                ..
            } if address == addr(11)
        ));
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn duplicate_link_loser_is_rejected() {
        let mut manager = ConnectionManager::<2, 8>::new(local(9));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        let resolve = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 0,
            },
        );
        assert_eq!(
            resolve,
            std::vec![ManagerAction::Reject {
                address: addr(11),
                dialed: true
            }]
        );
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn only_the_keeper_connection_opens_the_l2cap_fast_lane() {
        use crate::interfaces::bluetooth_auto::core::{Esp32Host, Psm};
        let l2cap_caps = LinkCapabilities {
            l2cap: Psm::new(0x0080),
            link_mtu: 247,
        };
        let me = LocalPeer {
            identity: BleIdentity::new([1; 16]),
            endpoint: Endpoint::Esp32(Esp32Host::Esp32),
            capabilities: l2cap_caps,
        };
        let mut manager = ConnectionManager::<2, 8>::new(me);
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
            ManagerInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: peer,
                now_ms: 0,
            },
        );
        assert!(
            matches!(
                admit[0],
                ManagerAction::Admit {
                    lane: L2capPlan::None,
                    ..
                }
            ),
            "the non-keeper (accepted) link must not open L2CAP"
        );

        let resolve = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: peer,
                now_ms: 0,
            },
        );
        assert!(matches!(resolve[0], ManagerAction::Evict { .. }));
        assert!(
            matches!(
                resolve[1],
                ManagerAction::Admit {
                    lane: L2capPlan::Open { .. },
                    ..
                }
            ),
            "the keeper (dialed, we are central) link opens the L2CAP fast lane"
        );
    }

    #[test]
    fn a_late_duplicate_keeps_the_stable_link_instead_of_evicting() {
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
        manager.start(&mut |_| {});
        let admit = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        assert!(matches!(admit[0], ManagerAction::Admit { slot: 0, .. }));

        let resolve = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: KEEPER_DUEL_WINDOW_MS + 1,
            },
        );
        assert_eq!(
            resolve,
            std::vec![ManagerAction::Reject {
                address: addr(11),
                dialed: true
            }]
        );
        assert_eq!(manager.settled_count(), 1);
    }

    #[test]
    fn dialing_pauses_after_chasing_a_duplicate() {
        let mut manager = ConnectionManager::<2, 8>::new(local(9));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        let reject = collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 1_000,
            },
        );
        assert_eq!(
            reject,
            std::vec![ManagerAction::Reject {
                address: addr(11),
                dialed: true
            }]
        );

        let paused = collect(
            &mut manager,
            ManagerInput::Sighting {
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
            ManagerInput::Sighting {
                address: addr(13),
                now_ms: 1_000 + DIAL_PAUSE_MS,
            },
        );
        assert_eq!(resumed, std::vec![ManagerAction::Dial(addr(13))]);
    }

    #[test]
    fn a_member_close_clears_the_dial_pause() {
        let mut manager = ConnectionManager::<2, 8>::new(local(9));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 1_000,
            },
        );
        collect(
            &mut manager,
            ManagerInput::Closed {
                identity: BleIdentity::new([2; 16]),
                address: addr(10),
            },
        );
        let dialed = collect(
            &mut manager,
            ManagerInput::Sighting {
                address: addr(12),
                now_ms: 1_100,
            },
        );
        assert_eq!(dialed, std::vec![ManagerAction::Dial(addr(12))]);
    }

    #[test]
    fn an_unrelated_close_keeps_the_dial_pause_while_peers_remain() {
        let mut manager = ConnectionManager::<4, 8>::new(local(9));
        manager.start(&mut |_| {});
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(10),
                origin: Origin::Accepted,
                established: established(2),
                now_ms: 0,
            },
        );
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(20),
                origin: Origin::Accepted,
                established: established(3),
                now_ms: 0,
            },
        );
        collect(
            &mut manager,
            ManagerInput::Settled {
                address: addr(11),
                origin: Origin::Dialed,
                established: established(2),
                now_ms: 1_000,
            },
        );
        collect(
            &mut manager,
            ManagerInput::Closed {
                identity: BleIdentity::new([3; 16]),
                address: addr(20),
            },
        );
        let sighting = collect(
            &mut manager,
            ManagerInput::Sighting {
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
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
        manager.start(&mut |_| {});
        let actions = collect(
            &mut manager,
            ManagerInput::HandshakeFailed {
                address: addr(7),
                origin: Origin::Accepted,
            },
        );
        assert_eq!(actions, std::vec![ManagerAction::NotifyClosed(addr(7))]);
    }

    #[test]
    fn the_handshake_gate_bounds_an_inbound_flood() {
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
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
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
        for _ in 0..(2 + HANDSHAKE_SLACK) {
            assert!(manager.begin_handshake(Origin::Accepted));
        }
        assert!(!manager.begin_handshake(Origin::Accepted));
        manager.handle(
            ManagerInput::HandshakeFailed {
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
