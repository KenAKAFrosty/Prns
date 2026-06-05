/// The runtime tried to hand an interface more than its outbound queue can hold; the
/// packet was not enqueued. The caller decides drop vs retry — submitting never
/// blocks. Returned by [`InboundSink::submit`](crate::interfaces::InboundSink::submit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueFull;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    QueueFull,
}
