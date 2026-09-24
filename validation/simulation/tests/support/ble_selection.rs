use core::{
    future::{poll_fn, Future},
    pin::Pin,
    task::Poll,
};
use prns_interfaces_tokio::bluetooth_auto::{BleEventSelector, BleEventSources};

/// Fixture-owned readiness arbitration, independent of protocol entropy.
pub(super) struct RoundRobinBleEvents {
    next: usize,
}

#[derive(Clone, Copy)]
pub(super) enum FirstEvent {
    Backend,
    Handshake,
    Closed,
    Disabled,
    Groups,
}

impl FirstEvent {
    pub const ALL: [Self; 5] = [
        Self::Backend,
        Self::Handshake,
        Self::Closed,
        Self::Disabled,
        Self::Groups,
    ];
    fn index(self) -> usize {
        match self {
            Self::Backend => 0,
            Self::Handshake => 1,
            Self::Closed => 2,
            Self::Disabled => 3,
            Self::Groups => 4,
        }
    }
}

impl RoundRobinBleEvents {
    pub fn new(first: FirstEvent) -> Self {
        Self {
            next: first.index(),
        }
    }
}

impl BleEventSelector for RoundRobinBleEvents {
    async fn select<T>(
        &mut self,
        sources: BleEventSources<
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
            impl Future<Output = T>,
        >,
    ) -> T {
        let mut backend = core::pin::pin!(sources.backend);
        let mut handshake = core::pin::pin!(sources.handshake);
        let mut closed = core::pin::pin!(sources.closed);
        let mut disabled = core::pin::pin!(sources.disabled);
        let mut groups = core::pin::pin!(sources.groups);
        let mut branches: [Pin<&mut dyn Future<Output = T>>; 5] = [
            backend.as_mut(),
            handshake.as_mut(),
            closed.as_mut(),
            disabled.as_mut(),
            groups.as_mut(),
        ];
        poll_fn(|cx| {
            for offset in 0..branches.len() {
                let index = (self.next + offset) % branches.len();
                if let Poll::Ready(event) = branches[index].as_mut().poll(cx) {
                    self.next = (index + 1) % branches.len();
                    return Poll::Ready(event);
                }
            }
            Poll::Pending
        })
        .await
    }
}

#[tokio::test]
async fn ready_branches_rotate_without_starvation() {
    let mut selector = RoundRobinBleEvents::new(FirstEvent::Backend);
    let mut observed = Vec::new();
    for _ in 0..10 {
        observed.push(
            selector
                .select(BleEventSources {
                    backend: core::future::ready(0),
                    handshake: core::future::ready(1),
                    closed: core::future::ready(2),
                    disabled: core::future::ready(3),
                    groups: core::future::ready(4),
                })
                .await,
        );
    }
    assert_eq!(observed, [0, 1, 2, 3, 4, 0, 1, 2, 3, 4]);
}

#[tokio::test]
async fn pending_selection_registers_wakes_and_cancellation_does_not_advance_it() {
    let mut selector = RoundRobinBleEvents::new(FirstEvent::Backend);
    {
        let mut pending = core::pin::pin!(selector.select(BleEventSources {
            backend: core::future::pending::<()>(),
            handshake: core::future::pending(),
            closed: core::future::pending(),
            disabled: core::future::pending(),
            groups: core::future::pending(),
        }));
        poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert_eq!(selector.next, 0);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let send = async {
        tokio::task::yield_now().await;
        assert_eq!(sender.send(42), Ok(()));
    };
    let select = selector.select(BleEventSources {
        backend: core::future::pending(),
        handshake: core::future::pending(),
        closed: core::future::pending(),
        disabled: core::future::pending(),
        groups: async {
            receiver
                .await
                .unwrap_or_else(|error| unreachable!("wake: {error}"))
        },
    });
    let ((), observed) = tokio::join!(send, select);
    assert_eq!((observed, selector.next), (42, 0));
}
