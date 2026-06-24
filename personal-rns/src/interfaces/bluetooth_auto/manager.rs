//! The shared Bluetooth connection-manager brain: the runtime-agnostic policy half of the
//! supervisor. Both the tokio (host) and embassy (embedded) supervisors feed it events and execute
//! the actions it emits, so the cross-platform policy — dial-on-sighting, the keeper duel that
//! resolves a double-connection, capacity gating, and dial/suppress backoff — lives in ONE place
//! that every platform inherits. Backends only wrangle the radio to honor the thin seam; the async
//! I/O (running the [`Handshake`](super::core::Handshake), pumping a member's frames) stays in the
//! drivers. This brain is pure logic over [`core`](super::core)'s primitives: no alloc, no await,
//! fixed-capacity state sized by `MAX_PEERS`.

use super::core::{
    arrangement, is_keeper, l2cap_plan, BleAddress, BleIdentity, Established, HandshakeRole,
    L2capPlan, Local,
};
use super::seam::Origin;

/// A peer suppressed after losing a keeper duel (or bouncing off a full radio) is left alone this
/// long before a fresh sighting re-dials it.
pub const SUPPRESS_TTL_MS: u64 = 8_000;
/// A dial in flight is given this long before a fresh sighting of the same address re-dials it.
pub const DIAL_RETRY_TTL_MS: u64 = 16_000;
pub const UNREACHABLE_TTL_MS: u64 = 60_000;

/// The handshake role for a link of this origin: a dialed link opens with `Hello` (Dialer), an
/// accepted one waits and replies `Welcome` (Listener). The driver calls this before running the
/// handshake, so the brain owns the rule even though the control-lane I/O is the driver's.
#[must_use]
pub fn role_for(origin: Origin) -> HandshakeRole {
    match origin {
        Origin::Dialed => HandshakeRole::Dialer,
        Origin::Accepted => HandshakeRole::Listener,
    }
}

/// An event fed to the manager. The driver translates radio/handshake outcomes into these; the
/// manager never touches the radio itself. `now_ms` is a monotonic millisecond clock the driver
/// supplies (tokio: elapsed since start; embassy: `Instant` since boot) — the brain stays
/// float-free and runtime-agnostic.
#[derive(Debug, Clone, Copy)]
pub enum ManagerInput {
    /// The radio saw a peer advertising our service at `address`.
    Sighting { address: BleAddress, now_ms: u64 },
    /// A link (dialed or accepted) finished its handshake and settled as `established`.
    Settled {
        address: BleAddress,
        origin: Origin,
        established: Established,
        now_ms: u64,
    },
    /// A link's handshake aborted or timed out before settling.
    HandshakeFailed { address: BleAddress, origin: Origin },
    DialFailed { address: BleAddress, now_ms: u64 },
    /// A settled member's link closed (the data pump saw its source/sink error, or the radio
    /// reported a disconnect).
    Closed {
        identity: BleIdentity,
        address: BleAddress,
    },
}

/// Whether the radio should advertise (accept inbound centrals) — a named two-state, not a bare
/// bool, so a call site reads its intent and the seam can't be handed an ambiguous flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertisingMode {
    On,
    Off,
}

/// Whether the radio should scan (look for peers to dial).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanningMode {
    On,
    Off,
}

/// An action the driver executes against the radio/fleet. The manager emits these through a sink
/// callback; the driver collects them and applies the async ones (`Dial`, `Admit`, …) after the
/// (synchronous) `handle` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerAction {
    /// Dial this address as a central.
    Dial(BleAddress),
    /// Stand the settled peer up as a fleet member in `slot` over the negotiated `lane`.
    Admit {
        identity: BleIdentity,
        slot: usize,
        address: BleAddress,
        lane: L2capPlan,
    },
    /// Tear down the member currently in `slot` (it lost a keeper duel to a fresh link).
    Evict { identity: BleIdentity, slot: usize },
    /// Drop the just-settled link without admitting it (duplicate loser, or no capacity).
    Reject { address: BleAddress, dialed: bool },
    /// Tell the backend a link to this address is gone, so it can release its connection state.
    NotifyClosed(BleAddress),
    /// Advertise (accept inbound) while a slot is free; stop when full.
    SetAdvertising(AdvertisingMode),
    /// Scan (look for peers to dial) while a slot is free; stop when full.
    SetScanning(ScanningMode),
}

#[derive(Clone, Copy)]
struct SettledSlot {
    identity: BleIdentity,
    keeper: bool,
    address: BleAddress,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackoffKind {
    Dialing,
    Suppressed,
    Unreachable,
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
            BackoffKind::Unreachable => UNREACHABLE_TTL_MS,
        }
    }

    fn elapsed(self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.since_ms) >= self.ttl_ms()
    }
}

/// The connection-manager brain. `MAX_PEERS` is the settled-member ceiling (the radio's concurrent
/// connection budget); `DIAL_TRACK` sizes the dial/suppress backoff table (addresses we are mid-dial
/// or cooling off — distinct from settled peers, so a few more than `MAX_PEERS`).
pub struct ConnectionManager<const MAX_PEERS: usize, const DIAL_TRACK: usize> {
    local: Local,
    settled: [Option<SettledSlot>; MAX_PEERS],
    backoff: [Option<Backoff>; DIAL_TRACK],
    advertising: bool,
    scanning: bool,
}

impl<const MAX_PEERS: usize, const DIAL_TRACK: usize> ConnectionManager<MAX_PEERS, DIAL_TRACK> {
    /// A fresh manager: no peers, radio idle (the driver calls [`start`](Self::start) to bring it
    /// up). `local` is this node's identity/endpoint/capabilities the keeper duel hashes.
    #[must_use]
    pub const fn new(local: Local) -> Self {
        Self {
            local,
            settled: [None; MAX_PEERS],
            backoff: [None; DIAL_TRACK],
            advertising: false,
            scanning: false,
        }
    }

    /// How many peers are settled right now.
    #[must_use]
    pub fn settled_count(&self) -> usize {
        self.settled.iter().filter(|slot| slot.is_some()).count()
    }

    /// Bring the radio up: with every slot free we both advertise and scan. The driver calls this
    /// once before its event loop.
    pub fn start<F: FnMut(ManagerAction)>(&mut self, emit: &mut F) {
        self.reconcile(emit);
    }

    /// Feed one event; the manager updates its state and emits any radio/fleet actions through
    /// `emit`. Synchronous and allocation-free — the driver applies the emitted actions itself.
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
        established: Established,
        now_ms: u64,
        emit: &mut F,
    ) {
        let dialed = matches!(origin, Origin::Dialed);
        if dialed {
            self.clear_backoff(address);
        }
        let identity = established.identity;
        let role = role_for(origin);
        let plan = arrangement(self.local.endpoint, established.endpoint);
        let keeper = is_keeper(
            plan,
            role,
            self.local.identity,
            self.local.endpoint,
            identity,
        );

        if let Some(existing) = self.find_settled_by_identity(identity) {
            let incumbent = self.settled[existing].expect("slot occupied");
            let challenger_wins = keeper && !incumbent.keeper;
            if !challenger_wins {
                self.upsert_backoff(address, BackoffKind::Suppressed, now_ms);
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
        let lane = l2cap_plan(
            plan,
            role,
            self.local.endpoint,
            &self.local.capabilities,
            &established.capabilities,
        );
        self.settled[slot] = Some(SettledSlot {
            identity,
            keeper,
            address,
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
        if matches!(origin, Origin::Dialed) {
            self.clear_backoff(address);
            emit(ManagerAction::NotifyClosed(address));
        }
    }

    fn on_dial_failed(&mut self, address: BleAddress, now_ms: u64) {
        self.upsert_backoff(address, BackoffKind::Unreachable, now_ms);
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
            Some(index) => self.backoff[index]
                .expect("backoff present")
                .elapsed(now_ms),
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

    fn local(identity: u8) -> Local {
        Local {
            identity: BleIdentity::new([identity; 16]),
            endpoint: endpoint(),
            capabilities: CAPS,
        }
    }

    fn established(identity: u8) -> Established {
        Established {
            identity: BleIdentity::new([identity; 16]),
            endpoint: endpoint(),
            capabilities: CAPS,
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

        // Within the dial-retry window: no re-dial.
        let within = collect(
            &mut manager,
            ManagerInput::Sighting {
                address: addr(9),
                now_ms: 1_000,
            },
        );
        assert!(within.is_empty());

        // After the window: dial again.
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
    fn a_failed_dial_suppresses_the_address_past_the_dial_retry_window() {
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
                now_ms: DIAL_RETRY_TTL_MS + 100,
            },
        );
        assert!(
            within.is_empty(),
            "an unreachable address is not re-dialed at the dial-retry window"
        );

        let after = collect(
            &mut manager,
            ManagerInput::Sighting {
                address: addr(9),
                now_ms: UNREACHABLE_TTL_MS + 200,
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
    fn duplicate_link_keeper_evicts_incumbent() {
        // ours = [1;16] < theirs = [2;16], GattOnly arrangement (two nrf endpoints) → we_should_be
        // _central == ours < theirs == true. So a Dialed link is the keeper, an Accepted one is not.
        let mut manager = ConnectionManager::<2, 8>::new(local(1));
        manager.start(&mut |_| {});

        // First the accepted link settles (not the keeper).
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

        // Then the same identity arrives over a dialed link (the keeper) → evict + re-admit.
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
        // ours = [9;16] > theirs = [2;16] → we_should_be_central == false. Now the Accepted link is
        // the keeper, so a later Dialed duplicate loses and is rejected (incumbent stays).
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
}
