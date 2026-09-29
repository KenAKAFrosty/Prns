use super::*;
use core::future::{pending, ready};

#[tokio::test]
async fn rotating_ready_sources_are_fair_from_either_initial_choice() {
    for first in [
        InterfaceEventSource::Message,
        InterfaceEventSource::Completion,
    ] {
        let mut selection = InterfaceArbitration::RoundRobin { first };
        let mut observed = Vec::new();
        for _ in 0..6 {
            observed.push(
                selection
                    .select(
                        ready(InterfaceEventSource::Message),
                        ready(InterfaceEventSource::Completion),
                    )
                    .await,
            );
        }
        let other = match first {
            InterfaceEventSource::Message => InterfaceEventSource::Completion,
            InterfaceEventSource::Completion => InterfaceEventSource::Message,
        };
        assert_eq!(observed, [first, other, first, other, first, other]);
    }
}

#[tokio::test]
async fn a_pending_preference_cannot_block_the_other_source_or_lose_a_wake() {
    for mut selection in [
        InterfaceArbitration::TokioFair,
        InterfaceArbitration::RoundRobin {
            first: InterfaceEventSource::Message,
        },
    ] {
        assert_eq!(selection.select(pending(), ready(7)).await, 7);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let receive = async {
            receiver
                .await
                .unwrap_or_else(|error| unreachable!("source: {error}"))
        };
        let send = async {
            tokio::task::yield_now().await;
            assert_eq!(sender.send(9), Ok(()));
        };
        let (observed, ()) = tokio::join!(selection.select(receive, pending()), send);
        assert_eq!(observed, 9);
    }
}

#[tokio::test]
async fn cancelled_pending_turn_keeps_the_cursor_and_borrowed_sources() {
    let mut selection = InterfaceArbitration::RoundRobin {
        first: InterfaceEventSource::Completion,
    };
    let (sender, mut receiver) = tokio::sync::oneshot::channel::<u8>();
    {
        let mut turn = core::pin::pin!(selection.select(pending(), async {
            (&mut receiver)
                .await
                .unwrap_or_else(|error| unreachable!("source: {error}"))
        }));
        poll_fn(|cx| {
            assert!(turn.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert!(matches!(
        selection,
        InterfaceArbitration::RoundRobin {
            first: InterfaceEventSource::Completion
        }
    ));
    assert_eq!(sender.send(9), Ok(()));
    assert_eq!(receiver.await, Ok(9));
}
