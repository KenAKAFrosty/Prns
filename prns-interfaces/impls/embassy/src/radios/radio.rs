//! Chip-agnostic LoRa radio contract shared by the embedded Hopspot faces.
//!
//! `LoRaInterface` (in `crate::lora`) is generic over [`Radio`] so the same
//! spectrum-access state machine drives every supported LoRa chip without
//! re-implementing the airtime, queue, and rejoin logic per device. A chip
//! driver (`crate::radios::sx126x::Sx126x`, `crate::radios::lr1110::Lr1110`)
//! implements [`Radio`] and the board wires a concrete chip into
//! [`LoRaInterfaceInput`](crate::lora::LoRaInterfaceInput).
//!
//! The async methods mirror the de-facto contract the SX126x path established:
//! `init` → `arm_rx` → a `read_event`/`poll_event`/`channel_rssi_dbm` RX loop,
//! with `transmit` for outbound. `RadioEvent` and `ReceivedAirFrame` live here
//! (not per-driver) so `LoRaInterface` can match on event variants without a
//! per-chip event type.

use prns_core::interfaces::lora::RadioProfile;
use prns_core::interfaces::PacketPhyStats;

/// One received air frame: the byte length (the caller owns the buffer the chip
/// filled) and the physical-layer stats the chip reported alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedAirFrame {
    pub len: usize,
    pub phy: PacketPhyStats,
}

/// Event the radio surfaces through DIO1. `LoRaInterface` discriminates these to
/// drive the spectrum-access state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioEvent {
    Frame(ReceivedAirFrame),
    PreambleDetected,
    HeaderValid,
    HeaderError,
    CrcError,
    Timeout,
    Other,
}

/// Chip-agnostic LoRa radio contract.
///
/// `Config` and `Error` are associated types because each chip speaks its own
/// register/config vocabulary and reports its own fault set; `RadioEvent` is
/// shared so the interface state machine need not be generic over the event.
pub trait Radio {
    /// Chip-specific radio configuration (frequency, modulation, packet shape,
    /// TX power, sync word).
    type Config;
    /// Chip-specific error. Must be `Debug` so the interface can log faults.
    type Error: core::fmt::Debug;

    /// Build the chip config from a Prns [`RadioProfile`].
    fn config_from_profile(profile: &RadioProfile) -> Self::Config;

    /// Construct the "payload too large" error. This is an encode-side failure
    /// (an air frame part did not fit), not a radio fault, so
    /// [`is_fault`](Self::is_fault) returns `false` for it.
    fn buffer_too_small_error() -> Self::Error;

    /// Whether an error means the radio itself is faulting (BUSY stuck, SPI
    /// dead, DIO1 wedged, reset/timeout) and needs a hard re-init, versus a
    /// routine or encode-side failure that leaves the chip usable.
    fn is_fault(e: &Self::Error) -> bool;

    /// Cold-start / reconfigure the chip to `config` and route IRQs. Leaves the
    /// chip in standby, fully configured.
    async fn init(&mut self, config: Self::Config) -> Result<(), Self::Error>;

    /// Arm continuous RX.
    async fn arm_rx(&mut self) -> Result<(), Self::Error>;

    /// Transmit one frame and wait for TxDone.
    async fn transmit(&mut self, payload: &[u8]) -> Result<(), Self::Error>;

    /// Instantaneous channel RSSI in dBm, valid while armed in RX (carrier
    /// sense for listen-before-talk).
    async fn channel_rssi_dbm(&mut self) -> Result<i16, Self::Error>;

    /// Wait for one receive-side IRQ event on an already-armed radio.
    async fn read_event(&mut self, buf: &mut [u8]) -> Result<RadioEvent, Self::Error>;

    /// Read an already-latched IRQ event without waiting for DIO1. The final
    /// race-closing check before a listen-before-talk transmitter leaves RX.
    async fn poll_event(&mut self, buf: &mut [u8]) -> Result<Option<RadioEvent>, Self::Error>;
}