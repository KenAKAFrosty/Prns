use embassy_net::udp::UdpSocket;
use embassy_net::IpAddress;
use embassy_time::{with_timeout, Duration};

use prns_core::engine::FanTarget;
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::InterfaceId;
use prns_runtime::manifold::grant::FrameTarget;

use super::peer::{SegmentRole, WifiPeerSlotLookup, WifiPeerTable};
use super::AutoWifiStatus;

pub(super) fn target_includes(target: FrameTarget, id: InterfaceId) -> bool {
    match target {
        FrameTarget::Direct(target) | FrameTarget::Fan(FanTarget::Only(target)) => target == id,
        FrameTarget::Fan(FanTarget::All) => true,
        FrameTarget::Fan(FanTarget::AllExcept(excluded)) => excluded != id,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FanoutCompletion {
    Complete,
    BudgetExhausted,
}

pub(super) struct FanoutPlan<const MEMBERS: usize> {
    selected: [bool; MEMBERS],
    next: usize,
    remaining: usize,
}

impl<const MEMBERS: usize> FanoutPlan<MEMBERS> {
    pub(super) fn new(target: FrameTarget, peers: &WifiPeerTable<MEMBERS>, start: usize) -> Self {
        let mut selected = [false; MEMBERS];
        let mut remaining = 0;
        for (slot, peer) in peers.iter() {
            selected[slot] = match target {
                FrameTarget::Direct(id) | FrameTarget::Fan(FanTarget::Only(id)) => peer.id() == id,
                FrameTarget::Fan(FanTarget::All) => true,
                FrameTarget::Fan(FanTarget::AllExcept(id)) => peer.id() != id,
            };
            remaining += usize::from(selected[slot]);
        }
        Self {
            selected,
            next: if MEMBERS == 0 { 0 } else { start % MEMBERS },
            remaining,
        }
    }

    fn next_slot(&mut self) -> Option<usize> {
        while self.remaining > 0 {
            let slot = self.next;
            self.next = (self.next + 1) % MEMBERS;
            if self.selected[slot] {
                self.selected[slot] = false;
                self.remaining -= 1;
                return Some(slot);
            }
        }
        None
    }

    fn per_attempt_budget(&self, total: Duration) -> Duration {
        Duration::from_micros(
            total
                .as_micros()
                .checked_div(self.remaining as u64)
                .unwrap_or(total.as_micros())
                .max(1),
        )
    }
}

pub(super) trait FanoutSender {
    async fn send_to_slot(&mut self, slot: usize) -> bool;
}

pub(super) async fn dispatch_fanout<const MEMBERS: usize>(
    plan: &mut FanoutPlan<MEMBERS>,
    sender: &mut impl FanoutSender,
    budget: Duration,
) -> FanoutCompletion {
    let per_attempt = plan.per_attempt_budget(budget);
    match with_timeout(budget, async {
        while let Some(slot) = plan.next_slot() {
            let _ = with_timeout(per_attempt, sender.send_to_slot(slot)).await;
        }
    })
    .await
    {
        Ok(()) => FanoutCompletion::Complete,
        Err(_) => FanoutCompletion::BudgetExhausted,
    }
}

pub(super) struct UdpFanoutSender<'a, 'd, const MEMBERS: usize> {
    pub(super) primary: &'a UdpSocket<'d>,
    pub(super) secondary: Option<&'a UdpSocket<'d>>,
    pub(super) peers: &'a WifiPeerTable<MEMBERS>,
    pub(super) status: AutoWifiStatus<MEMBERS>,
    pub(super) bytes: &'a [u8],
}

impl<const MEMBERS: usize> FanoutSender for UdpFanoutSender<'_, '_, MEMBERS> {
    async fn send_to_slot(&mut self, slot: usize) -> bool {
        let WifiPeerSlotLookup::Occupied(peer) = self.peers.lookup_slot(slot) else {
            return false;
        };
        let socket = match peer.segment() {
            SegmentRole::Primary => Some(self.primary),
            SegmentRole::Secondary => self.secondary,
        };
        let Some(socket) = socket else {
            return false;
        };
        if socket
            .send_to(
                self.bytes,
                (IpAddress::Ipv6(peer.address()), contract::DEFAULT_DATA_PORT),
            )
            .await
            .is_err()
        {
            return false;
        }
        self.status.member(slot).add_tx(self.bytes.len() as u64);
        true
    }
}

pub(super) async fn send_beacon(socket: Option<&UdpSocket<'_>>, token: Option<&[u8; 32]>) -> bool {
    let (Some(socket), Some(token)) = (socket, token) else {
        return false;
    };
    socket
        .send_to(
            token,
            (
                IpAddress::Ipv6(contract::DISCOVERY_GROUP),
                contract::DEFAULT_DISCOVERY_PORT,
            ),
        )
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::future::pending;
    use ::core::net::Ipv6Addr;
    use embassy_futures::select::{select, Either};
    use embassy_futures::{block_on, yield_now};
    use std::cell::Cell;

    fn address(suffix: u16) -> Ipv6Addr {
        Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, suffix)
    }

    fn peer(suffix: u16) -> super::super::peer::WifiPeer {
        super::super::peer::WifiPeer::new(address(suffix), SegmentRole::Primary)
    }

    fn id(suffix: u16) -> InterfaceId {
        peer(suffix).id()
    }

    fn peer_table<const MEMBERS: usize>(occupied: &[(usize, u16)]) -> WifiPeerTable<MEMBERS> {
        let mut peers = WifiPeerTable::new();
        for &(slot, suffix) in occupied {
            assert_eq!(
                peers.insert(slot, peer(suffix)),
                super::super::peer::WifiPeerInsertion::Inserted { slot }
            );
        }
        peers
    }

    fn slots<const MEMBERS: usize>(mut plan: FanoutPlan<MEMBERS>) -> std::vec::Vec<usize> {
        let mut slots = std::vec::Vec::new();
        while let Some(slot) = plan.next_slot() {
            slots.push(slot);
        }
        slots
    }

    struct MockSender {
        attempts: std::vec::Vec<usize>,
        blocked: Option<usize>,
    }

    impl FanoutSender for MockSender {
        async fn send_to_slot(&mut self, slot: usize) -> bool {
            self.attempts.push(slot);
            if self.blocked == Some(slot) {
                pending().await
            }
            true
        }
    }

    struct CancelGuard<'a>(&'a Cell<bool>);

    impl Drop for CancelGuard<'_> {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    struct BlockingSender<'a> {
        canceled: &'a Cell<bool>,
    }

    impl FanoutSender for BlockingSender<'_> {
        async fn send_to_slot(&mut self, _slot: usize) -> bool {
            let _guard = CancelGuard(self.canceled);
            pending().await
        }
    }

    #[test]
    fn targets_only_live_selected_members_in_rotating_order() {
        let peers = peer_table::<4>(&[(0, 1), (2, 3), (3, 4)]);

        assert_eq!(
            slots(FanoutPlan::new(FrameTarget::Fan(FanTarget::All), &peers, 2)),
            [2, 3, 0]
        );
        assert_eq!(
            slots(FanoutPlan::new(FrameTarget::Direct(id(4)), &peers, 0)),
            [3]
        );
        assert_eq!(
            slots(FanoutPlan::new(
                FrameTarget::Fan(FanTarget::AllExcept(id(3))),
                &peers,
                2,
            )),
            [3, 0]
        );
    }

    #[test]
    fn target_membership_covers_direct_and_fan_variants() {
        assert!(target_includes(FrameTarget::Direct(id(1)), id(1)));
        assert!(!target_includes(FrameTarget::Direct(id(2)), id(1)));
        assert!(target_includes(
            FrameTarget::Fan(FanTarget::Only(id(1))),
            id(1)
        ));
        assert!(target_includes(FrameTarget::Fan(FanTarget::All), id(1)));
        assert!(!target_includes(
            FrameTarget::Fan(FanTarget::AllExcept(id(1))),
            id(1)
        ));
    }

    #[test]
    fn one_aggregate_budget_is_divided_across_selected_members() {
        let occupied = ::std::vec::Vec::from_iter((0..24).map(|slot| (slot, slot as u16 + 1)));
        let peers = peer_table::<24>(&occupied);
        let broadcast = FanoutPlan::new(FrameTarget::Fan(FanTarget::All), &peers, 0);
        let direct = FanoutPlan::new(FrameTarget::Direct(id(1)), &peers, 0);
        let budget = Duration::from_millis(300);

        assert_eq!(
            broadcast.per_attempt_budget(budget),
            Duration::from_micros(12_500)
        );
        assert_eq!(direct.per_attempt_budget(budget), budget);
    }

    #[test]
    fn a_blocked_member_does_not_consume_later_members_budgets() {
        let peers = peer_table::<3>(&[(0, 1), (1, 2), (2, 3)]);
        let mut plan = FanoutPlan::new(FrameTarget::Fan(FanTarget::All), &peers, 0);
        let mut sender = MockSender {
            attempts: std::vec::Vec::new(),
            blocked: Some(0),
        };

        let completion = block_on(dispatch_fanout(
            &mut plan,
            &mut sender,
            Duration::from_millis(60),
        ));

        assert_eq!(completion, FanoutCompletion::Complete);
        assert_eq!(sender.attempts, [0, 1, 2]);
    }

    #[test]
    fn cancellation_drops_the_blocked_transport_future() {
        let peers = peer_table::<1>(&[(0, 1)]);
        let mut plan = FanoutPlan::new(FrameTarget::Fan(FanTarget::All), &peers, 0);
        let canceled = Cell::new(false);
        let mut sender = BlockingSender {
            canceled: &canceled,
        };

        block_on(async {
            let dispatch = dispatch_fanout(&mut plan, &mut sender, Duration::from_secs(1));
            let interrupt = async {
                yield_now().await;
            };
            assert!(matches!(
                select(dispatch, interrupt).await,
                Either::Second(())
            ));
        });

        assert!(canceled.get());
    }
}
