use ::core::net::Ipv6Addr;

use embassy_net::udp::UdpSocket;
use embassy_net::IpAddress;
use embassy_time::{with_timeout, Duration};

use prns_core::engine::FanTarget;
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::InterfaceId;
use prns_runtime::manifold::grant::FrameTarget;

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
    pub(super) fn new(
        target: FrameTarget,
        peers: &[Option<Ipv6Addr>; MEMBERS],
        ids: &[InterfaceId; MEMBERS],
        start: usize,
    ) -> Self {
        let mut selected = [false; MEMBERS];
        let mut remaining = 0;
        for slot in 0..MEMBERS {
            if peers[slot].is_none() {
                continue;
            }
            selected[slot] = match target {
                FrameTarget::Direct(id) | FrameTarget::Fan(FanTarget::Only(id)) => ids[slot] == id,
                FrameTarget::Fan(FanTarget::All) => true,
                FrameTarget::Fan(FanTarget::AllExcept(id)) => ids[slot] != id,
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
    pub(super) peers: &'a [Option<Ipv6Addr>; MEMBERS],
    pub(super) peer_on_secondary: &'a [bool; MEMBERS],
    pub(super) status: AutoWifiStatus<MEMBERS>,
    pub(super) bytes: &'a [u8],
}

impl<const MEMBERS: usize> FanoutSender for UdpFanoutSender<'_, '_, MEMBERS> {
    async fn send_to_slot(&mut self, slot: usize) -> bool {
        let Some(peer) = self.peers[slot] else {
            return false;
        };
        let socket = if self.peer_on_secondary[slot] {
            self.secondary
        } else {
            Some(self.primary)
        };
        let Some(socket) = socket else {
            return false;
        };
        if socket
            .send_to(
                self.bytes,
                (IpAddress::Ipv6(peer), contract::DEFAULT_DATA_PORT),
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

pub(super) async fn send_reverse_peering(
    socket: Option<&UdpSocket<'_>>,
    token: Option<&[u8; 32]>,
    peer: Ipv6Addr,
) -> bool {
    let (Some(socket), Some(token)) = (socket, token) else {
        return false;
    };
    match socket
        .send_to(
            token,
            (IpAddress::Ipv6(peer), contract::UNICAST_DISCOVERY_PORT),
        )
        .await
    {
        Ok(()) => true,
        Err(error) => {
            crate::diagnostic_log::warn!(
                "wifi-auto: reverse peering to {peer} failed: {error:?}"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::future::pending;
    use embassy_futures::select::{select, Either};
    use embassy_futures::{block_on, yield_now};
    use prns_core::interfaces::InterfaceKind;
    use std::cell::Cell;

    fn peer(suffix: u16) -> Ipv6Addr {
        Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, suffix)
    }

    fn id(suffix: u8) -> InterfaceId {
        InterfaceId::new([InterfaceKind::WifiPeer as u8, 0, 0, 0, 0, 0, 0, suffix])
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
        let peers = [Some(peer(1)), None, Some(peer(3)), Some(peer(4))];
        let ids = [id(1), id(2), id(3), id(4)];

        assert_eq!(
            slots(FanoutPlan::new(
                FrameTarget::Fan(FanTarget::All),
                &peers,
                &ids,
                2,
            )),
            [2, 3, 0]
        );
        assert_eq!(
            slots(FanoutPlan::new(FrameTarget::Direct(id(4)), &peers, &ids, 0,)),
            [3]
        );
        assert_eq!(
            slots(FanoutPlan::new(
                FrameTarget::Fan(FanTarget::AllExcept(id(3))),
                &peers,
                &ids,
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
        let peers = [Some(peer(1)); 24];
        let ids = ::core::array::from_fn(|slot| id(slot as u8 + 1));
        let broadcast = FanoutPlan::new(FrameTarget::Fan(FanTarget::All), &peers, &ids, 0);
        let direct = FanoutPlan::new(FrameTarget::Direct(id(1)), &peers, &ids, 0);
        let budget = Duration::from_millis(300);

        assert_eq!(
            broadcast.per_attempt_budget(budget),
            Duration::from_micros(12_500)
        );
        assert_eq!(direct.per_attempt_budget(budget), budget);
    }

    #[test]
    fn a_blocked_member_does_not_consume_later_members_budgets() {
        let peers = [Some(peer(1)), Some(peer(2)), Some(peer(3))];
        let ids = [id(1), id(2), id(3)];
        let mut plan = FanoutPlan::new(FrameTarget::Fan(FanTarget::All), &peers, &ids, 0);
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
        let peers = [Some(peer(1))];
        let ids = [id(1)];
        let mut plan = FanoutPlan::new(FrameTarget::Fan(FanTarget::All), &peers, &ids, 0);
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
