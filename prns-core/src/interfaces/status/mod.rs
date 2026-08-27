mod connection;

pub use connection::ConnectionState;

use crate::interfaces::{InterfaceGravity, InterfaceId, InterfaceMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AirtimeUtilization {
    pub short_per_mille: u16,
    pub long_per_mille: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferRates {
    pub rx_bps: u32,
    pub tx_bps: u32,
}

/// Frame-level receive accounting, for telling "nothing arrived" apart from "something arrived
/// and was thrown away". Byte counters cannot make that distinction: a frame discarded before
/// reassembly still moves `rx_bytes`, so a silent decode failure and a healthy link look alike
/// from outside. These counters mark events at different receive layers, not a conservation
/// equation: split frames, control envelopes, and malformed candidates mean `frames_in` need not
/// equal the other counters' sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameAccounting {
    /// Receive units taken off the medium, after any self-addressed echo is filtered out. A
    /// datagram is one unit; a split LoRa packet contributes one unit per air frame.
    pub frames_in: u64,
    /// Complete receive units rejected as malformed by an authoritative RNS parser, either at
    /// initial classification or in deeper protocol parsing.
    pub malformed: u64,
    /// Receive candidates that violate interface framing or RNS protocol rules. This is a
    /// layered superset: every `malformed` or `undecodable` event is also a protocol violation,
    /// alongside structurally valid packets rejected by deeper semantic checks such as an
    /// invalid proof or an impossible link phase.
    pub protocol_violations: u64,
    /// Receive units discarded by interface framing or reassembly before a complete RNS frame
    /// could be handed off.
    pub undecodable: u64,
    /// Wire frames fully reassembled and handed to the engine. This counts the handoff, not the
    /// engine's verdict: a frame the engine goes on to ignore is still counted here, so
    /// `delivered` bounds what reached the engine rather than what it acted on.
    pub delivered: u64,
}

impl FrameAccounting {
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self {
            frames_in: self.frames_in.saturating_add(other.frames_in),
            malformed: self.malformed.saturating_add(other.malformed),
            protocol_violations: self
                .protocol_violations
                .saturating_add(other.protocol_violations),
            undecodable: self.undecodable.saturating_add(other.undecodable),
            delivered: self.delivered.saturating_add(other.delivered),
        }
    }
}

#[cfg(feature = "tokio-host")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameAccountingEvent {
    Received,
    Malformed,
    ProtocolViolation,
    Undecodable,
    Delivered,
}

#[cfg(feature = "tokio-host")]
pub trait RecordsFrameAccounting {
    fn record_frame_event(&self, event: FrameAccountingEvent);
}

#[cfg(feature = "tokio-host")]
#[derive(Clone)]
pub struct FrameAccountingRecorder {
    id: InterfaceId,
    record: std::sync::Arc<dyn Fn(FrameAccountingEvent) + Send + Sync>,
}

#[cfg(feature = "tokio-host")]
impl FrameAccountingRecorder {
    pub fn of<S>(status: S) -> Option<Self>
    where
        S: InterfaceStatus + RecordsFrameAccounting + Send + Sync + 'static,
    {
        status.frame_accounting()?;
        Some(Self {
            id: status.id(),
            record: std::sync::Arc::new(move |event| status.record_frame_event(event)),
        })
    }

    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    pub fn record(&self, event: FrameAccountingEvent) {
        (self.record)(event);
    }
}

pub trait InterfaceStatus {
    fn id(&self) -> InterfaceId;
    fn connection(&self) -> ConnectionState;
    fn failure_reason(&self) -> Option<&'static str> {
        None
    }
    fn rx_bytes(&self) -> u64;
    fn tx_bytes(&self) -> u64;
    /// `None` until the interface publishes — a link with no declared bitrate never does.
    fn airtime(&self) -> Option<AirtimeUtilization> {
        None
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        None
    }

    /// Frame-level receive accounting, when the family keeps it. `None` means the family does
    /// not account for frames, which a caller must not read as all-zero: unaccounted and
    /// "nothing arrived" are different answers.
    fn frame_accounting(&self) -> Option<FrameAccounting> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Independent,
    FleetMember { supervisor_id: InterfaceId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceVitals {
    pub id: InterfaceId,
    pub connection: ConnectionState,
    pub failure_reason: Option<&'static str>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub transfer_rates: Option<TransferRates>,
    pub frame_accounting: Option<FrameAccounting>,
}

impl InterfaceVitals {
    pub fn of(status: &impl InterfaceStatus) -> Self {
        Self {
            id: status.id(),
            connection: status.connection(),
            failure_reason: status.failure_reason(),
            rx_bytes: status.rx_bytes(),
            tx_bytes: status.tx_bytes(),
            transfer_rates: status.transfer_rates(),
            frame_accounting: status.frame_accounting(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceSnapshot {
    pub id: InterfaceId,
    pub mode: InterfaceMode,
    pub gravity: InterfaceGravity,
    pub connection: ConnectionState,
    pub failure_reason: Option<&'static str>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub transfer_rates: Option<TransferRates>,
    pub destinations: u32,
    pub links: u32,
    pub transported_links: u32,
    pub membership: Membership,
}

#[cfg(feature = "tokio-host")]
pub type StatusView = std::sync::Arc<dyn Fn() -> std::vec::Vec<InterfaceVitals> + Send + Sync>;

#[cfg(feature = "tokio-host")]
#[derive(Clone)]
pub struct ConnectionView {
    read: std::sync::Arc<dyn Fn() -> ConnectionState + Send + Sync>,
}

#[cfg(feature = "tokio-host")]
impl ConnectionView {
    pub fn of<S>(status: S) -> Self
    where
        S: InterfaceStatus + Send + Sync + 'static,
    {
        Self {
            read: std::sync::Arc::new(move || status.connection()),
        }
    }

    pub fn connection(&self) -> ConnectionState {
        (self.read)()
    }
}

#[cfg(feature = "tokio-host")]
pub trait ReportsStatus {
    fn status_view(&self) -> Option<StatusView> {
        None
    }

    fn connection_view(&self) -> Option<ConnectionView> {
        None
    }

    fn frame_accounting_recorder(&self) -> Option<FrameAccountingRecorder> {
        None
    }
}

impl<T: InterfaceStatus + ?Sized> InterfaceStatus for &T {
    fn id(&self) -> InterfaceId {
        (**self).id()
    }

    fn connection(&self) -> ConnectionState {
        (**self).connection()
    }

    fn failure_reason(&self) -> Option<&'static str> {
        (**self).failure_reason()
    }

    fn rx_bytes(&self) -> u64 {
        (**self).rx_bytes()
    }

    fn tx_bytes(&self) -> u64 {
        (**self).tx_bytes()
    }

    fn airtime(&self) -> Option<AirtimeUtilization> {
        (**self).airtime()
    }

    fn transfer_rates(&self) -> Option<TransferRates> {
        (**self).transfer_rates()
    }

    fn frame_accounting(&self) -> Option<FrameAccounting> {
        (**self).frame_accounting()
    }
}
