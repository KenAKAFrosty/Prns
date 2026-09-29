use super::*;
use core::{
    future::{pending, poll_fn},
    task::Poll,
};

#[tokio::test]
async fn default_selection_wakes_for_each_source_without_polling_a_completed_one_again() {
    assert_eq!(core::mem::size_of::<TokioFairBleEvents>(), 0);
    let mut selector = TokioFairBleEvents;
    for ready in 0..5 {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut receivers: [_; 5] = core::array::from_fn(|_| None);
        receivers[ready] = Some(receiver);
        let wait = |receiver: Option<tokio::sync::oneshot::Receiver<usize>>| async move {
            match receiver {
                Some(receiver) => receiver
                    .await
                    .unwrap_or_else(|error| unreachable!("send: {error}")),
                None => pending().await,
            }
        };
        let [b, h, c, d, g] = receivers;
        let select = selector.select(BleEventSources {
            backend: wait(b),
            handshake: wait(h),
            closed: wait(c),
            disabled: wait(d),
            groups: wait(g),
        });
        let send = async {
            tokio::task::yield_now().await;
            assert_eq!(sender.send(ready), Ok(()));
        };
        let (observed, ()) = tokio::join!(select, send);
        assert_eq!(observed, ready);
    }
}

#[tokio::test]
async fn cancelling_default_selection_leaves_borrowed_sources_usable() {
    let mut selector = TokioFairBleEvents;
    let (sender, mut receiver) = tokio::sync::oneshot::channel::<u8>();
    {
        let mut selection = core::pin::pin!(selector.select(BleEventSources {
            backend: async {
                (&mut receiver)
                    .await
                    .unwrap_or_else(|error| unreachable!("receive: {error}"))
            },
            handshake: pending(),
            closed: pending(),
            disabled: pending(),
            groups: pending(),
        }));
        poll_fn(|cx| {
            assert!(selection.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert_eq!(sender.send(7), Ok(()));
    assert_eq!(receiver.await, Ok(7));
}
