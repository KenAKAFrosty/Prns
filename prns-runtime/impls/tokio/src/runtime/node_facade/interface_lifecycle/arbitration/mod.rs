use core::future::{poll_fn, Future};
use core::task::Poll;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceEventSource {
    Message,
    Completion,
}

/// Node-owned interface-driver readiness policy. Normal construction retains
/// Tokio fairness; explicit rotation makes competing inputs reproducible.
#[derive(Clone, Copy)]
pub enum InterfaceArbitration {
    TokioFair,
    RoundRobin { first: InterfaceEventSource },
}

impl InterfaceArbitration {
    pub(super) async fn select<T>(
        &mut self,
        message: impl Future<Output = T>,
        completion: impl Future<Output = T>,
    ) -> T {
        let Self::RoundRobin { first } = self else {
            return tokio::select! { event = message => event, event = completion => event };
        };
        let mut message = core::pin::pin!(message);
        let mut completion = core::pin::pin!(completion);
        poll_fn(|cx| {
            let (preferred, other) = match first {
                InterfaceEventSource::Message => {
                    (message.as_mut().poll(cx), InterfaceEventSource::Completion)
                }
                InterfaceEventSource::Completion => {
                    (completion.as_mut().poll(cx), InterfaceEventSource::Message)
                }
            };
            if let Poll::Ready(event) = preferred {
                *first = other;
                return Poll::Ready(event);
            }
            match other {
                InterfaceEventSource::Message => message.as_mut().poll(cx),
                InterfaceEventSource::Completion => completion.as_mut().poll(cx),
            }
        })
        .await
    }
}
