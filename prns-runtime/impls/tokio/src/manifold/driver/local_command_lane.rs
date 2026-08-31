use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::task::AtomicWaker;
use rtrb::{Consumer, PopError, Producer, PushError, RingBuffer};

use super::HostCommand;

pub(crate) struct LocalCommandProducer {
    commands: Producer<HostCommand>,
    producer_open: Arc<AtomicBool>,
    consumer_open: Arc<AtomicBool>,
    consumer_parked: Arc<AtomicBool>,
    consumer_waker: Arc<AtomicWaker>,
}

pub(crate) struct LocalCommandConsumer {
    commands: Consumer<HostCommand>,
    producer_open: Arc<AtomicBool>,
    consumer_open: Arc<AtomicBool>,
    consumer_parked: Arc<AtomicBool>,
    consumer_waker: Arc<AtomicWaker>,
}

pub(crate) fn local_command_lane(depth: usize) -> (LocalCommandProducer, LocalCommandConsumer) {
    let (producer, consumer) = RingBuffer::new(depth.max(1));
    let producer_open = Arc::new(AtomicBool::new(true));
    let consumer_open = Arc::new(AtomicBool::new(true));
    let consumer_parked = Arc::new(AtomicBool::new(false));
    let consumer_waker = Arc::new(AtomicWaker::new());
    (
        LocalCommandProducer {
            commands: producer,
            producer_open: producer_open.clone(),
            consumer_open: consumer_open.clone(),
            consumer_parked: consumer_parked.clone(),
            consumer_waker: consumer_waker.clone(),
        },
        LocalCommandConsumer {
            commands: consumer,
            producer_open,
            consumer_open,
            consumer_parked,
            consumer_waker,
        },
    )
}

impl LocalCommandProducer {
    pub(crate) fn send(&mut self, command: HostCommand) -> Result<(), HostCommand> {
        if !self.consumer_open.load(Ordering::Acquire) {
            return Err(command);
        }
        match self.commands.push(command) {
            Ok(()) => {
                if self.consumer_parked.load(Ordering::Acquire)
                    && self.consumer_parked.swap(false, Ordering::AcqRel)
                {
                    self.consumer_waker.wake();
                }
                Ok(())
            }
            Err(PushError::Full(command)) => Err(command),
        }
    }
}

impl Drop for LocalCommandProducer {
    fn drop(&mut self) {
        self.producer_open.store(false, Ordering::Release);
        if self.consumer_parked.load(Ordering::Acquire)
            && self.consumer_parked.swap(false, Ordering::AcqRel)
        {
            self.consumer_waker.wake();
        }
    }
}

impl LocalCommandConsumer {
    pub(crate) fn try_recv(&mut self) -> Option<HostCommand> {
        self.consumer_parked.store(false, Ordering::Release);
        self.commands.pop().ok()
    }

    pub(crate) fn poll_recv(&mut self, context: &mut Context<'_>) -> Poll<Option<HostCommand>> {
        match self.commands.pop() {
            Ok(command) => return Poll::Ready(Some(command)),
            Err(PopError::Empty) if !self.producer_open.load(Ordering::Acquire) => {
                return Poll::Ready(None);
            }
            Err(PopError::Empty) => {}
        }
        // The ring owns readiness. Publish the cold waiter, then recheck both data and closure so
        // a producer racing this arm either wakes us or becomes synchronously visible.
        self.consumer_waker.register(context.waker());
        self.consumer_parked.store(true, Ordering::Release);
        match self.commands.pop() {
            Ok(command) => {
                self.consumer_parked.store(false, Ordering::Release);
                Poll::Ready(Some(command))
            }
            Err(PopError::Empty) if !self.producer_open.load(Ordering::Acquire) => {
                self.consumer_parked.store(false, Ordering::Release);
                Poll::Ready(None)
            }
            Err(PopError::Empty) => Poll::Pending,
        }
    }
}

impl Drop for LocalCommandConsumer {
    fn drop(&mut self) {
        self.consumer_open.store(false, Ordering::Release);
        self.consumer_parked.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use crate::engine::{CloseLink, CommandId, IssuedCommand, PrnsCommand};
    use crate::routing::links::LinkId;
    use crate::wire::TRUNCATED_HASH_BYTE_LEN;

    use super::*;

    #[test]
    fn raw_lane_endpoints_remain_movable_before_executor_ownership_is_chosen() {
        fn assert_send<T: Send>() {}
        assert_send::<LocalCommandProducer>();
        assert_send::<LocalCommandConsumer>();
    }

    fn command(id: u64) -> HostCommand {
        HostCommand::Engine(IssuedCommand {
            id: CommandId(id),
            command: PrnsCommand::CloseLink(CloseLink {
                link_id: LinkId::new([id as u8; TRUNCATED_HASH_BYTE_LEN]),
            }),
        })
    }

    fn id(command: HostCommand) -> CommandId {
        let HostCommand::Engine(issued) = command else {
            panic!("test command")
        };
        issued.id
    }

    #[test]
    fn local_commands_move_fifo_without_a_shared_channel() {
        let (mut producer, mut consumer) = local_command_lane(4);
        assert!(producer.send(command(1)).is_ok());
        assert!(producer.send(command(2)).is_ok());
        assert_eq!(id(consumer.try_recv().unwrap()), CommandId(1));
        assert_eq!(id(consumer.try_recv().unwrap()), CommandId(2));
        assert!(consumer.try_recv().is_none());
    }

    #[test]
    fn a_full_local_lane_applies_backpressure_without_losing_the_command() {
        let (mut producer, _consumer) = local_command_lane(1);
        assert!(producer.send(command(1)).is_ok());
        let command = producer.send(command(2)).unwrap_err();
        assert_eq!(id(command), CommandId(2));
    }

    #[tokio::test]
    async fn only_a_parked_consumer_is_woken() {
        let (mut producer, mut consumer) = local_command_lane(4);
        let send = async move {
            tokio::task::yield_now().await;
            assert!(producer.send(command(3)).is_ok());
        };
        let receive = poll_fn(|context| consumer.poll_recv(context));
        let ((), received) = tokio::join!(send, receive);
        assert_eq!(id(received.unwrap()), CommandId(3));
    }
}
