//! Optional bounded, payload-free radio observations. No wire or queue policy effects.
use portable_atomic::{AtomicU32, Ordering};
use prns_core::routing::links::{handshake::link_request_from, LinkId};
use prns_core::wire::{DestinationType, IfacFlag, PacketType, WireContext, WirePacketHeader};
use prns_runtime::manifold::outbound::OutboundQueueDropReason;

/// Ordinary observations per observer lifetime, plus one reserved LimitReached callback.
pub const LORA_LINK_TRACE_LIMIT: u32 = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoRaTracePacket {
    Request,
    Proof,
    LinkData,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoRaTraceDrop {
    Contention,
    Duty,
    Disabled,
    Changed,
    Unconfigured,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoRaLinkPhase {
    QueueActive,
    QueuePending,
    QueueActivated,
    QueueRejected,
    QueueCleared,
    QueueDropped(OutboundQueueDropReason),
    ActiveDropped(LoRaTraceDrop),
    TxStarted,
    TxFrameStarted,
    TxFrameComplete,
    TxComplete,
    TxRadioError,
    TxFramingError,
    RxFrame,
    RxAwaiting,
    RxReplaced,
    RxRejected,
    RxPacket,
    RxDelivered,
    RxCrcError,
    RxHeaderError,
    RxRadioError,
    RxReset,
    LimitReached,
}

/// Public ephemeral Link ID only; sinks must hash it before writing external logs.
/// `at_ms` uses the absolute Embassy monotonic clock; unknown is None.
/// `queue_bytes` excludes the active packet, includes local FIFO record headers,
/// and saturates at u16::MAX. Frame indices are zero-based; count zero means unknown.
/// TxFrameComplete means the radio driver returned Ok, never peer reception.
/// RxDelivered means the software ingress handoff completed, never app delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoRaLinkEvent {
    pub sequence: u32,
    pub at_ms: Option<u64>,
    pub link_id: Option<LinkId>,
    pub packet: LoRaTracePacket,
    pub phase: LoRaLinkPhase,
    pub age_ms: Option<u64>,
    pub queue_bytes: u16,
    pub active: bool,
    pub frame_index: u8,
    pub frame_count: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoRaLinkSnapshot {
    /// Detailed observations attempted, before producer and sink budgets.
    pub attempted: u32,
    /// Callbacks admitted by the sink, including the terminal marker.
    pub emitted: u32,
    /// Ordinary observations omitted after the producer limit.
    pub producer_dropped: u32,
    /// Callbacks rejected by the nonblocking sink, including a rejected terminal marker.
    pub sink_dropped: u32,
    pub queue_accepted: u32,
    pub tx_started: u32,
    pub tx_complete: u32,
    pub tx_failed: u32,
    pub rx_frames: u32,
    pub rx_packets: u32,
    pub rx_crc_errors: u32,
    pub rx_header_errors: u32,
    pub rx_radio_errors: u32,
}

/// Caller-owned counters and nonblocking sink, with no history or packet storage.
/// Counters saturate and continue after the event budget. Snapshot fields are
/// individually atomic, not a transaction; a concurrent event can straddle a snapshot.
pub struct LoRaLinkTrace {
    observer: fn(LoRaLinkEvent) -> bool,
    counters: [AtomicU32; 13],
}
impl LoRaLinkTrace {
    #[must_use]
    pub const fn new(observer: fn(LoRaLinkEvent) -> bool) -> Self {
        Self {
            observer,
            counters: [const { AtomicU32::new(0) }; 13],
        }
    }
    #[must_use]
    pub fn snapshot(&self) -> LoRaLinkSnapshot {
        let n = |i: usize| self.counters[i].load(Ordering::Relaxed);
        LoRaLinkSnapshot {
            attempted: n(0),
            emitted: n(1),
            producer_dropped: n(2),
            sink_dropped: n(3),
            queue_accepted: n(4),
            tx_started: n(5),
            tx_complete: n(6),
            tx_failed: n(7),
            rx_frames: n(8),
            rx_packets: n(9),
            rx_crc_errors: n(10),
            rx_header_errors: n(11),
            rx_radio_errors: n(12),
        }
    }
    fn increment(&self, i: usize) -> u32 {
        self.counters[i]
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_add(1))
            })
            .unwrap_or_else(|v| v)
            .saturating_add(1)
    }
    pub(super) fn observe(&self, mut event: LoRaLinkEvent) {
        use LoRaLinkPhase::*;
        let counter = match event.phase {
            QueueActive | QueuePending => Some(4),
            TxStarted => Some(5),
            TxComplete => Some(6),
            TxRadioError | TxFramingError => Some(7),
            RxFrame => Some(8),
            RxPacket => Some(9),
            RxCrcError => Some(10),
            RxHeaderError => Some(11),
            RxRadioError => Some(12),
            _ => None,
        };
        if let Some(i) = counter {
            self.increment(i);
        }
        // Ordinary unrelated traffic contributes to totals without per-frame log flooding.
        if event.link_id.is_none()
            && !matches!(
                event.phase,
                RxAwaiting
                    | RxReplaced
                    | RxRejected
                    | RxCrcError
                    | RxHeaderError
                    | RxRadioError
                    | RxReset
                    | TxRadioError
                    | TxFramingError
            )
        {
            return;
        }
        let sequence = self.increment(0);
        if sequence > LORA_LINK_TRACE_LIMIT {
            self.increment(2);
            if sequence != LORA_LINK_TRACE_LIMIT + 1 {
                return;
            }
            event.phase = LimitReached;
            event.link_id = None;
            event.packet = LoRaTracePacket::Other;
            event.age_ms = None;
        }
        event.sequence = sequence;
        if (self.observer)(event) {
            self.increment(1);
        } else {
            self.increment(3);
        }
    }
}

pub(super) fn identity(bytes: &[u8]) -> (LoRaTracePacket, Option<LinkId>) {
    let Ok((header, payload)) = WirePacketHeader::parse(bytes) else {
        return (LoRaTracePacket::Other, None);
    };
    // IFAC masks the header; interpreting masked bytes would create false correlations.
    if header.ifac_flag != IfacFlag::Open {
        return (LoRaTracePacket::Other, None);
    }
    if header.packet_type == PacketType::LinkRequest
        && header.destination_type == DestinationType::Single
    {
        if let Ok(request) = link_request_from(&header, payload) {
            return (LoRaTracePacket::Request, Some(request.link_id));
        }
    } else if header.destination_type == DestinationType::Link {
        let kind = if header.packet_type == PacketType::Proof
            && header.context == WireContext::LinkRequestProof
        {
            LoRaTracePacket::Proof
        } else {
            LoRaTracePacket::LinkData
        };
        return (kind, Some(LinkId::from_address(header.address)));
    }
    (LoRaTracePacket::Other, None)
}

/// Borrowed metadata keeps the Xtensa ABI from expanding two Option<u64>
/// arguments across all argument registers and stack slots in nested helpers.
#[derive(Clone, Copy)]
pub(super) struct EventTime {
    pub at_ms: Option<u64>,
    pub enqueued_at: Option<u64>,
}
pub(super) struct EventContext {
    pub time: EventTime,
    pub queue_bytes: usize,
    pub active: bool,
}

pub(super) fn event(bytes: &[u8], phase: LoRaLinkPhase, context: &EventContext) -> LoRaLinkEvent {
    let (packet, link_id) = identity(bytes);
    LoRaLinkEvent {
        sequence: 0,
        at_ms: context.time.at_ms,
        link_id,
        packet,
        phase,
        age_ms: context
            .time
            .at_ms
            .zip(context.time.enqueued_at)
            .and_then(|(now, at)| now.checked_sub(at)),
        queue_bytes: u16::try_from(context.queue_bytes).unwrap_or(u16::MAX),
        active: context.active,
        frame_index: 0,
        frame_count: 0,
    }
}

pub(super) fn observe(trace: Option<&LoRaLinkTrace>, phase: LoRaLinkPhase, at_ms: Option<u64>) {
    if let Some(trace) = trace {
        trace.observe(event(
            &[],
            phase,
            &crate::lora::link_trace::EventContext {
                time: crate::lora::link_trace::EventTime {
                    at_ms,
                    enqueued_at: None,
                },
                queue_bytes: 0,
                active: false,
            },
        ));
    }
}
