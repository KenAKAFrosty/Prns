//! Semtech LR1110 LoRa radio driver for the Prns embedded Hopspot.
//!
//! Ported from the RNode firmware's Arduino LR1110 driver (`lr1110.cpp`, MIT,
//! Mark Qvist) which itself wraps Semtech's vendored LR11xx C command driver
//! (Clear BSD, Copyright Semtech Corporation 2021). This Rust driver re-
//! implements the LR11xx command set against `embedded-hal-async` traits;
//! the command opcodes, byte layouts, and PA tables are derived from those
//! sources and retain their attribution. No Semtech source code is included.
//!
//! Board reference: Seeed SenseCAP Wio Tracker T1000-E (nRF52840 + LR1110).
//! The RF switch and TCXO are internal to the LR1110 (driven through its own
//! DIO pins via `SetDioAsRfSwitch` / `SetTcxoMode`), so unlike the SX1262 path
//! the board contributes no MCU GPIO for antenna bias or TCXO enable.
//!
//! The public surface mirrors `sx126x` so a later `Radio` trait unification
//! (generalizing `LoRaInterface` over the radio chip) is a mechanical change.

use core::future::{poll_fn, Future};
use core::task::Poll;

use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::{Operation, SpiDevice};

use prns_core::interfaces::{PacketPhyStats, RssiDbm, SnrQuarterDb};

/// LR11xx NOP/dummy byte written on MOSI while reading status/data on MISO.
const LR11XX_NOP: u8 = 0x00;

/// LR11xx commands are 2-byte opcodes, sent big-endian (high byte first).
#[allow(dead_code)]
mod op {
    // System
    pub const GET_STATUS: u16 = 0x0100;
    pub const GET_VERSION: u16 = 0x0101;
    pub const CALIBRATE: u16 = 0x010F;
    pub const SET_REG_MODE: u16 = 0x0110;
    pub const CALIBRATE_IMAGE: u16 = 0x0111;
    pub const SET_DIO_AS_RF_SWITCH: u16 = 0x0112;
    pub const SET_DIO_IRQ_PARAMS: u16 = 0x0113;
    pub const CLEAR_IRQ: u16 = 0x0114;
    pub const SET_TCXO_MODE: u16 = 0x0117;
    pub const SET_SLEEP: u16 = 0x011B;
    pub const SET_STANDBY: u16 = 0x011C;
    pub const SET_FS: u16 = 0x011D;
    pub const GET_RANDOM: u16 = 0x0120;
    // Register / memory
    pub const WRITE_BUFFER8: u16 = 0x0109;
    pub const READ_BUFFER8: u16 = 0x010A;
    // Radio
    pub const GET_RX_BUFFER_STATUS: u16 = 0x0203;
    pub const GET_PKT_STATUS: u16 = 0x0204;
    pub const GET_RSSI_INST: u16 = 0x0205;
    pub const SET_RX: u16 = 0x0209;
    pub const SET_TX: u16 = 0x020A;
    pub const SET_RF_FREQUENCY: u16 = 0x020B;
    pub const SET_PKT_TYPE: u16 = 0x020E;
    pub const SET_MODULATION_PARAM: u16 = 0x020F;
    pub const SET_PKT_PARAM: u16 = 0x0210;
    pub const SET_TX_PARAMS: u16 = 0x0211;
    pub const SET_PA_CFG: u16 = 0x0215;
    pub const STOP_TIMEOUT_ON_PREAMBLE: u16 = 0x0217;
    pub const SET_RX_BOOSTED: u16 = 0x0227;
    pub const SET_LORA_SYNC_WORD: u16 = 0x022B;
}

/// LR11xx IRQ mask is a 32-bit field (vs 16 on the SX126x).
mod irq {
    pub const TX_DONE: u32 = 1 << 2;
    pub const RX_DONE: u32 = 1 << 3;
    pub const PREAMBLE_DETECTED: u32 = 1 << 4;
    pub const HEADER_VALID: u32 = 1 << 5; // SYNC_WORD_HEADER_VALID
    pub const HEADER_ERROR: u32 = 1 << 6;
    pub const CRC_ERROR: u32 = 1 << 7;
    pub const TIMEOUT: u32 = 1 << 10;
    /// IRQs we route to DIO1 and discriminate in software: every event the LoRa
    /// state machine needs, mirroring the SX1262 unified-mask approach.
    pub const LORA_ROUTE: u32 =
        TX_DONE | RX_DONE | PREAMBLE_DETECTED | HEADER_VALID | HEADER_ERROR | CRC_ERROR | TIMEOUT;
}

/// Calibration parameter mask: all blocks the T1000-E cold-start calibrates.
const CALIB_ALL: u8 = 0x3F; // LF_RC | HF_RC | PLL | ADC | IMG | PLL_TX

/// TCXO startup timeout in 32.768 kHz RTC ticks (30 ms). The LR11xx TCXO is
/// internal (driven out of a radio DIO), so this is a radio command, not a
/// board GPIO sequence.
const TCXO_STARTUP_TICKS: u32 = 983; // ~30 ms @ 32768 Hz

/// LR1110 LoRa max payload — the on-air length field is a single byte.
const MAX_LORA_PAYLOAD: usize = 255;

/// RX buffer is 256 bytes; reads are linear (the chip does not auto-wrap, so a
/// frame straddling the top of the buffer is split by the caller if needed).
const RX_BUFFER_BYTES: usize = 256;

/// Longest the LR1110 should hold BUSY: commands process in tens of
/// microseconds, cold-start calibration with TCXO startup ~30 ms. BUSY gates
/// every SPI cycle, so an unbounded wait lets one wedged-high line hang the
/// radio task and every recovery command; past this we surface [`Error::Busy`]
/// so the caller can hard-reset.
const BUSY_TIMEOUT_MS: u32 = 100;

/// Longest a single LoRa frame can sit on air before TxDone (worst case
/// SF12 / BW125, 255 bytes, CR4:8 with LDRO ~14 s); clears it with margin.
/// `SetTx` is single-shot (timeout 0), so the TxDone IRQ is otherwise
/// unbounded; a wait past this means the PA or IRQ path faulted.
const TX_DONE_TIMEOUT_MS: u32 = 20_000;

const RAMP_48_US: u8 = 0x02;

/// TCXO supply voltage the LR1110 drives out of its TCXO DIO. The LR11xx
/// encoding matches the SX1262's `TcxoVoltage` exactly (Clear BSD Semtech
/// table), so the values stay 1:1 for the later `Radio` trait unification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TcxoVoltage {
    V1_6 = 0x00,
    V1_7 = 0x01,
    V1_8 = 0x02,
    V2_2 = 0x03,
    V2_4 = 0x04,
    V2_7 = 0x05,
    V3_0 = 0x06,
    V3_3 = 0x07,
}

/// LoRa spreading factor — the LR11xx byte is the SF number itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SpreadingFactor {
    Sf5 = 0x05,
    Sf6 = 0x06,
    Sf7 = 0x07,
    Sf8 = 0x08,
    Sf9 = 0x09,
    Sf10 = 0x0A,
    Sf11 = 0x0B,
    Sf12 = 0x0C,
}

/// LoRa bandwidth — the LR11xx modulation-param code (Semtech table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Bandwidth {
    Bw125 = 0x04,
    Bw250 = 0x05,
    Bw500 = 0x06,
}

impl Bandwidth {
    fn khz(self) -> u32 {
        match self {
            Bandwidth::Bw125 => 125,
            Bandwidth::Bw250 => 250,
            Bandwidth::Bw500 => 500,
        }
    }
}

/// LoRa coding rate — the LR11xx modulation-param code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodingRate {
    Cr4_5 = 0x01,
    Cr4_6 = 0x02,
    Cr4_7 = 0x03,
    Cr4_8 = 0x04,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    Lora {
        spreading_factor: SpreadingFactor,
        bandwidth: Bandwidth,
        coding_rate: CodingRate,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoraPacket {
    pub preamble_symbols: u16,
    pub explicit_header: bool,
    pub crc_on: bool,
    pub invert_iq: bool,
}

pub struct RadioConfig {
    pub frequency_hz: u32,
    pub modulation: Modulation,
    pub packet: LoraPacket,
    /// LR11xx LoRa sync word is a single byte; the private-network value used
    /// by RNode/Reticulum is 0x12. The low byte of this field is sent.
    pub sync_word: u16,
    pub tx_power_dbm: i8,
}

/// T1000-E board-level radio config. The LR1110's RF switch and TCXO are
/// internal to the chip (driven via radio DIOs), so — unlike the SX1262 —
/// there is no `dio2_as_rf_switch` / external TCXO GPIO here; the RF switch
/// table is baked into [`Lr1110::init`] because it is LR1110-specific radio
/// config, not MCU wiring.
#[derive(Debug, Clone, Copy)]
pub struct BoardConfig {
    pub tcxo_voltage: Option<TcxoVoltage>,
    pub use_dcdc: bool,
    pub rx_boost: bool,
    pub external_rx_gain_db: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Spi,
    Busy,
    Dio1,
    Reset,
    Crc,
    Timeout,
    BufferTooSmall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedAirFrame {
    pub len: usize,
    pub phy: PacketPhyStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioEvent {
    PreambleDetected,
    HeaderValid,
    Frame(ReceivedAirFrame),
    HeaderError,
    CrcError,
    Timeout,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IrqEventKind {
    PreambleDetected,
    HeaderValid,
    Frame,
    HeaderError,
    CrcError,
    Timeout,
    Other,
}

fn classify_irq(flags: u32) -> IrqEventKind {
    if flags & irq::RX_DONE != 0 {
        return if flags & irq::CRC_ERROR != 0 {
            IrqEventKind::CrcError
        } else {
            IrqEventKind::Frame
        };
    }
    if flags & irq::HEADER_ERROR != 0 {
        return IrqEventKind::HeaderError;
    }
    if flags & irq::HEADER_VALID != 0 {
        return IrqEventKind::HeaderValid;
    }
    if flags & irq::PREAMBLE_DETECTED != 0 {
        return IrqEventKind::PreambleDetected;
    }
    if flags & irq::CRC_ERROR != 0 {
        return IrqEventKind::CrcError;
    }
    if flags & irq::TIMEOUT != 0 {
        return IrqEventKind::Timeout;
    }
    IrqEventKind::Other
}

async fn deadline<F, E, D>(
    fut: F,
    delay: &mut D,
    timeout_ms: u32,
    pin_err: Error,
    timeout_err: Error,
) -> Result<(), Error>
where
    F: Future<Output = Result<(), E>>,
    D: DelayNs,
{
    let mut fut = core::pin::pin!(fut);
    let mut timeout = core::pin::pin!(delay.delay_ms(timeout_ms));
    poll_fn(move |cx| {
        if let Poll::Ready(result) = fut.as_mut().poll(cx) {
            return Poll::Ready(result.map_err(|_| pin_err));
        }
        if timeout.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(timeout_err));
        }
        Poll::Pending
    })
    .await
}

pub struct Lr1110<SPI, BUSY, DIO1, RST, DLY> {
    spi: SPI,
    busy: BUSY,
    dio1: DIO1,
    reset: RST,
    delay: DLY,
    config: BoardConfig,
    freq_hz: u32,
    modulation: Modulation,
    packet: LoraPacket,
    tx_power_dbm: i8,
    /// RAM staging for the TX buffer write — DMA-class SPI can't source a
    /// flash-resident payload. A field, not a per-call stack local, so it
    /// never bloats the `transmit` future or the shared node stack.
    tx_staging: [u8; MAX_LORA_PAYLOAD],
}

impl<SPI, BUSY, DIO1, RST, DLY> Lr1110<SPI, BUSY, DIO1, RST, DLY>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    pub fn new(spi: SPI, busy: BUSY, dio1: DIO1, reset: RST, delay: DLY, config: BoardConfig) -> Self {
        Self {
            spi,
            busy,
            dio1,
            reset,
            delay,
            config,
            freq_hz: 915_000_000,
            modulation: Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf7,
                bandwidth: Bandwidth::Bw125,
                coding_rate: CodingRate::Cr4_5,
            },
            packet: LoraPacket {
                preamble_symbols: 18,
                explicit_header: true,
                crc_on: true,
                invert_iq: false,
            },
            tx_power_dbm: 2,
            tx_staging: [0u8; MAX_LORA_PAYLOAD],
        }
    }

    async fn wait_busy(&mut self) -> Result<(), Error> {
        let Self { busy, delay, .. } = self;
        deadline(
            busy.wait_for_low(),
            delay,
            BUSY_TIMEOUT_MS,
            Error::Busy,
            Error::Busy,
        )
        .await
    }

    async fn hard_reset(&mut self) -> Result<(), Error> {
        self.reset.set_low().map_err(|_| Error::Reset)?;
        self.delay.delay_ms(1).await;
        self.reset.set_high().map_err(|_| Error::Reset)?;
        self.delay.delay_ms(150).await; // LR11xx boot time after reset
        self.wait_busy().await
    }

    /// One SPI write cycle (CS assert, command + optional data, CS deassert).
    async fn write_command(&mut self, cmd: &[u8], data: &[u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        self.spi
            .transaction(&mut [Operation::Write(cmd), Operation::Write(data)])
            .await
            .map_err(|_| Error::Spi)
    }

    async fn command(&mut self, cmd: &[u8]) -> Result<(), Error> {
        self.write_command(cmd, &[]).await
    }

    /// LR11xx read = two CS cycles: CS1 sends the command, BUSY asserts while
    /// the chip processes it, CS2 writes a NOP dummy (status byte discarded)
    /// and then clocks out the data. The data buffer carries no status prefix.
    async fn read_command(&mut self, cmd: &[u8], data: &mut [u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        self.spi.write(cmd).await.map_err(|_| Error::Spi)?;
        self.wait_busy().await?;
        self.spi
            .transaction(&mut [Operation::Write(&[LR11XX_NOP]), Operation::Read(data)])
            .await
            .map_err(|_| Error::Spi)
    }

    /// Direct status read (no command): CS, read 6 bytes — [stat1, stat2,
    /// irq_b3, irq_b2, irq_b1, irq_b0]. This is how the LR11xx exposes live IRQ
    /// status without a GetIrqStatus opcode (the SX1262 has one; the LR11xx
    /// folds it into the always-available status direct-read).
    async fn direct_read(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        self.spi
            .transaction(&mut [Operation::Read(buf)])
            .await
            .map_err(|_| Error::Spi)
    }

    async fn irq_status(&mut self) -> Result<u32, Error> {
        let mut buf = [0u8; 6];
        self.direct_read(&mut buf).await?;
        Ok(((buf[2] as u32) << 24)
            | ((buf[3] as u32) << 16)
            | ((buf[4] as u32) << 8)
            | (buf[5] as u32))
    }

    async fn clear_irq(&mut self, mask: u32) -> Result<(), Error> {
        self.command(&[
            (op::CLEAR_IRQ >> 8) as u8,
            op::CLEAR_IRQ as u8,
            (mask >> 24) as u8,
            (mask >> 16) as u8,
            (mask >> 8) as u8,
            mask as u8,
        ])
        .await
    }

    async fn write_tx_payload(&mut self, len: usize) -> Result<(), Error> {
        let header = [(op::WRITE_BUFFER8 >> 8) as u8, op::WRITE_BUFFER8 as u8];
        let Self {
            spi, tx_staging, ..
        } = self;
        spi.transaction(&mut [
            Operation::Write(&header),
            Operation::Write(&tx_staging[..len]),
        ])
        .await
        .map_err(|_| Error::Spi)
    }

    async fn read_buffer(&mut self, offset: u8, buf: &mut [u8]) -> Result<(), Error> {
        let cmd = [
            (op::READ_BUFFER8 >> 8) as u8,
            op::READ_BUFFER8 as u8,
            offset,
            buf.len() as u8,
        ];
        self.read_command(&cmd, buf).await
    }
}

impl<SPI, BUSY, DIO1, RST, DLY> Lr1110<SPI, BUSY, DIO1, RST, DLY>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    /// Reset the chip, verify it reports as an LR1110, run the LoRa cold-start
    /// sequence (regulator, RF switch, TCXO, calibrate, packet type, sync word),
    /// then apply the channel config and route IRQs to DIO1. Leaves the chip in
    /// standby, fully configured.
    pub async fn init(&mut self, config: RadioConfig) -> Result<(), Error> {
        let RadioConfig {
            frequency_hz,
            modulation,
            packet,
            sync_word,
            tx_power_dbm,
        } = config;
        self.freq_hz = frequency_hz;
        self.modulation = modulation;
        self.packet = packet;
        self.tx_power_dbm = tx_power_dbm;

        self.hard_reset().await?;
        self.wait_for_lr1110().await?;
        self.standby_rc().await?;
        if self.config.use_dcdc {
            self.set_reg_mode_dcdc().await?;
        }
        self.set_dio_as_rf_switch().await?;
        if let Some(voltage) = self.config.tcxo_voltage {
            self.set_tcxo_mode(voltage).await?;
        }
        self.calibrate().await?;
        self.delay.delay_ms(5).await;
        self.set_pkt_type_lora().await?;
        self.set_lora_sync_word(sync_word).await?;
        self.set_rf_frequency().await?;
        self.set_tx_power().await?;
        self.configure().await?;
        if self.config.rx_boost {
            self.cfg_rx_boosted(true).await?;
        }
        self.route_irqs().await
    }

    /// Poll `GetVersion` until the chip reports `type == LR1110` (0x01), bounded
    /// so a dead/absent radio surfaces as [`Error::Reset`] instead of hanging.
    /// 200 attempts × 10 ms = 2 s overall budget, matching the RNode firmware's
    /// cold-start poll.
    async fn wait_for_lr1110(&mut self) -> Result<(), Error> {
        let mut version = [0u8; 4]; // [hw, type, fw_hi, fw_lo]
        for _ in 0..200 {
            self.read_command(&[(op::GET_VERSION >> 8) as u8, op::GET_VERSION as u8], &mut version)
                .await?;
            if version[1] == LR1110_VERSION_TYPE {
                return Ok(());
            }
            self.delay.delay_ms(10).await;
        }
        Err(Error::Reset)
    }

    /// Apply the channel config (modulation, packet shape). The LR1110 RETAINS
    /// these across SetStandby / SetTx / SetRx (only Sleep or reset clears
    /// them), so this runs ONCE from [`init`](Self::init) and again only on a
    /// discrete channel change, never per packet; the per-packet path only
    /// restamps the payload length.
    async fn configure(&mut self) -> Result<(), Error> {
        self.set_modulation_params().await?;
        self.set_packet_params(0xFF).await?; // 0xFF = RX-friendly max
        Ok(())
    }

    /// Route the LoRa IRQ set onto DIO1 (DIO2 gets none) so a single async
    /// `wait_for_high` observes every event the state machine consumes. The
    /// LR1110 IRQ status register always carries all IRQs regardless of DIO
    /// routing, so `read_event` discriminates in software after reading.
    async fn route_irqs(&mut self) -> Result<(), Error> {
        let m = irq::LORA_ROUTE.to_be_bytes();
        self.command(&[
            (op::SET_DIO_IRQ_PARAMS >> 8) as u8,
            op::SET_DIO_IRQ_PARAMS as u8,
            m[0],
            m[1],
            m[2],
            m[3], // irqs_to_enable_dio1
            0,
            0,
            0,
            0, // irqs_to_enable_dio2 = none
        ])
        .await
    }

    /// Transmit one LoRa frame and wait for TxDone. The channel must already be
    /// configured; only the payload length is restamped per frame.
    pub async fn transmit(&mut self, payload: &[u8]) -> Result<(), Error> {
        let len = payload.len();
        if len > MAX_LORA_PAYLOAD {
            return Err(Error::BufferTooSmall);
        }
        // DMA-class SPI can only source from RAM; the caller's payload may be
        // flash-resident, so stage it through the RAM `tx_staging` field.
        self.tx_staging[..len].copy_from_slice(payload);

        self.standby_rc().await?;
        self.set_packet_params(len as u8).await?;
        self.write_tx_payload(len).await?;
        self.clear_irq(irq::LORA_ROUTE | irq::TIMEOUT).await?;
        // SetTx with timeout 0 = single shot, no chip timeout — the TxDone wait
        // is bounded here ([`TX_DONE_TIMEOUT_MS`]).
        self.command(&[
            (op::SET_TX >> 8) as u8,
            op::SET_TX as u8,
            0x00,
            0x00,
            0x00,
        ])
        .await?;
        {
            let Self { dio1, delay, .. } = self;
            deadline(
                dio1.wait_for_high(),
                delay,
                TX_DONE_TIMEOUT_MS,
                Error::Dio1,
                Error::Timeout,
            )
            .await?;
        }
        let flags = self.irq_status().await?;
        self.clear_irq(flags).await?;
        if flags & irq::TIMEOUT != 0 {
            return Err(Error::Timeout);
        }
        Ok(())
    }

    /// Arm continuous RX: restamp the RX-side max payload length, clear stale
    /// IRQs, enter SetRx continuous (timeout 0xFFFFFF = no timeout).
    pub async fn arm_rx(&mut self) -> Result<(), Error> {
        self.standby_rc().await?;
        self.set_packet_params(0xFF).await?;
        self.clear_irq(irq::LORA_ROUTE | irq::TIMEOUT).await?;
        self.command(&[
            (op::SET_RX >> 8) as u8,
            op::SET_RX as u8,
            0xFF,
            0xFF,
            0xFF,
        ])
        .await
    }

    /// Wait for one receive-side IRQ event on an already
    /// [`arm_rx`](Self::arm_rx)'d radio. The radio remains in continuous RX.
    pub async fn read_event(&mut self, buf: &mut [u8]) -> Result<RadioEvent, Error> {
        self.dio1.wait_for_high().await.map_err(|_| Error::Dio1)?;
        let flags = self.irq_status().await?;
        self.clear_irq(flags).await?;
        self.decode_radio_event(flags, buf).await
    }

    /// Read an already-latched IRQ event without waiting for DIO1. This is the
    /// final race-closing check immediately before a listen-before-talk
    /// transmitter changes the radio out of RX.
    pub async fn poll_event(&mut self, buf: &mut [u8]) -> Result<Option<RadioEvent>, Error> {
        let flags = self.irq_status().await?;
        if flags == 0 {
            return Ok(None);
        }
        self.clear_irq(flags).await?;
        self.decode_radio_event(flags, buf).await.map(Some)
    }

    async fn decode_radio_event(
        &mut self,
        flags: u32,
        buf: &mut [u8],
    ) -> Result<RadioEvent, Error> {
        match classify_irq(flags) {
            IrqEventKind::Frame => {
                let mut status = [0u8; 2]; // [pld_len_in_bytes, buffer_start_pointer]
                self.read_command(
                    &[(op::GET_RX_BUFFER_STATUS >> 8) as u8, op::GET_RX_BUFFER_STATUS as u8],
                    &mut status,
                )
                .await?;
                let len = status[0] as usize;
                let offset = status[1];
                if len > buf.len() {
                    return Err(Error::BufferTooSmall);
                }
                let mut pkt_status = [0u8; 3]; // [rssi_pkt, snr_pkt, signal_rssi_pkt]
                self.read_command(
                    &[(op::GET_PKT_STATUS >> 8) as u8, op::GET_PKT_STATUS as u8],
                    &mut pkt_status,
                )
                .await?;
                let phy = PacketPhyStats {
                    rssi: Some(RssiDbm::new(antenna_referred_rssi_dbm(
                        pkt_status[0],
                        self.config.external_rx_gain_db,
                    ))),
                    snr: Some(SnrQuarterDb::new(i16::from(pkt_status[1] as i8))),
                    quality: None,
                };
                // LR1110 buffer read is linear; a frame straddling the top of
                // the 256-byte buffer is split rather than wrapped by the chip.
                if offset as usize + len <= RX_BUFFER_BYTES {
                    self.read_buffer(offset, &mut buf[..len]).await?;
                } else {
                    let first = RX_BUFFER_BYTES - offset as usize;
                    self.read_buffer(offset, &mut buf[..first]).await?;
                    self.read_buffer(0, &mut buf[first..len]).await?;
                }
                Ok(RadioEvent::Frame(ReceivedAirFrame { len, phy }))
            }
            IrqEventKind::PreambleDetected => Ok(RadioEvent::PreambleDetected),
            IrqEventKind::HeaderValid => Ok(RadioEvent::HeaderValid),
            IrqEventKind::HeaderError => Ok(RadioEvent::HeaderError),
            IrqEventKind::CrcError => Ok(RadioEvent::CrcError),
            IrqEventKind::Timeout => Ok(RadioEvent::Timeout),
            IrqEventKind::Other => Ok(RadioEvent::Other),
        }
    }

    pub async fn read_frame(&mut self, buf: &mut [u8]) -> Result<ReceivedAirFrame, Error> {
        loop {
            match self.read_event(buf).await? {
                RadioEvent::Frame(frame) => return Ok(frame),
                RadioEvent::CrcError => return Err(Error::Crc),
                RadioEvent::Timeout => return Err(Error::Timeout),
                RadioEvent::PreambleDetected
                | RadioEvent::HeaderValid
                | RadioEvent::HeaderError
                | RadioEvent::Other => {}
            }
        }
    }

    pub async fn receive(&mut self, buf: &mut [u8]) -> Result<ReceivedAirFrame, Error> {
        self.arm_rx().await?;
        self.read_frame(buf).await
    }

    /// The instantaneous channel RSSI in dBm, valid while armed in RX: the
    /// carrier-sense a listen-before-talk transmitter checks before holding
    /// off. LR11xx `GetRssiInst` returns a 0.5 dB-step magnitude that is
    /// negated to dBm (Semtech convention, Clear BSD).
    pub async fn channel_rssi_dbm(&mut self) -> Result<i16, Error> {
        let mut rssi = [0u8; 1];
        self.read_command(
            &[(op::GET_RSSI_INST >> 8) as u8, op::GET_RSSI_INST as u8],
            &mut rssi,
        )
        .await?;
        Ok(antenna_referred_rssi_dbm(rssi[0], self.config.external_rx_gain_db))
    }

    async fn standby_rc(&mut self) -> Result<(), Error> {
        self.command(&[(op::SET_STANDBY >> 8) as u8, op::SET_STANDBY as u8, 0x00])
            .await
    }

    async fn set_reg_mode_dcdc(&mut self) -> Result<(), Error> {
        self.command(&[(op::SET_REG_MODE >> 8) as u8, op::SET_REG_MODE as u8, 0x01])
            .await
    }

    async fn set_dio_as_rf_switch(&mut self) -> Result<(), Error> {
        // T1000-E RF switch table (Seeed ral_lr11xx_bsp): enable all RFSW,
        // RX on RFSW0, TX on RFSW0|RFSW1, TX_HP on RFSW1. The LR1110 drives these
        // through its own DIO pins, so there is no MCU GPIO involvement.
        const RFSW0: u8 = 1 << 0;
        const RFSW1: u8 = 1 << 1;
        const RFSW2: u8 = 1 << 2;
        const RFSW3: u8 = 1 << 3;
        self.command(&[
            (op::SET_DIO_AS_RF_SWITCH >> 8) as u8,
            op::SET_DIO_AS_RF_SWITCH as u8,
            RFSW0 | RFSW1 | RFSW2 | RFSW3, // enable
            0,                              // standby
            RFSW0,                          // rx
            RFSW0 | RFSW1,                  // tx
            RFSW1,                          // tx_hp
            0,                              // tx_hf
            RFSW2,                          // gnss
            RFSW3,                          // wifi
        ])
        .await
    }

    async fn set_tcxo_mode(&mut self, voltage: TcxoVoltage) -> Result<(), Error> {
        let t = TCXO_STARTUP_TICKS.to_be_bytes();
        self.command(&[
            (op::SET_TCXO_MODE >> 8) as u8,
            op::SET_TCXO_MODE as u8,
            voltage as u8,
            t[1], // timeout is 24-bit: >>16, >>8, >>0
            t[2],
            t[3],
        ])
        .await
    }

    async fn calibrate(&mut self) -> Result<(), Error> {
        self.command(&[(op::CALIBRATE >> 8) as u8, op::CALIBRATE as u8, CALIB_ALL])
            .await
    }

    async fn set_pkt_type_lora(&mut self) -> Result<(), Error> {
        self.command(&[(op::SET_PKT_TYPE >> 8) as u8, op::SET_PKT_TYPE as u8, 0x02])
            .await
    }

    async fn set_lora_sync_word(&mut self, sync_word: u16) -> Result<(), Error> {
        self.command(&[
            (op::SET_LORA_SYNC_WORD >> 8) as u8,
            op::SET_LORA_SYNC_WORD as u8,
            sync_word as u8, // private network 0x12
        ])
        .await
    }

    async fn set_rf_frequency(&mut self) -> Result<(), Error> {
        let f = self.freq_hz.to_be_bytes();
        self.command(&[
            (op::SET_RF_FREQUENCY >> 8) as u8,
            op::SET_RF_FREQUENCY as u8,
            f[0],
            f[1],
            f[2],
            f[3],
        ])
        .await
    }

    async fn set_modulation_params(&mut self) -> Result<(), Error> {
        match self.modulation {
            Modulation::Lora {
                spreading_factor,
                bandwidth,
                coding_rate,
            } => {
                let ldro = lora_ldro(spreading_factor, bandwidth);
                self.command(&[
                    (op::SET_MODULATION_PARAM >> 8) as u8,
                    op::SET_MODULATION_PARAM as u8,
                    spreading_factor as u8,
                    bandwidth as u8,
                    coding_rate as u8,
                    ldro,
                ])
                .await
            }
        }
    }

    async fn set_packet_params(&mut self, payload_len: u8) -> Result<(), Error> {
        let pre = self.packet.preamble_symbols.to_be_bytes();
        let header = u8::from(!self.packet.explicit_header); // explicit=0, implicit=1
        let crc = u8::from(self.packet.crc_on);
        let iq = u8::from(self.packet.invert_iq);
        self.command(&[
            (op::SET_PKT_PARAM >> 8) as u8,
            op::SET_PKT_PARAM as u8,
            pre[0],
            pre[1],
            header,
            payload_len,
            crc,
            iq,
        ])
        .await
    }

    async fn set_tx_power(&mut self) -> Result<(), Error> {
        let pa = pa_config(self.tx_power_dbm);
        self.command(&[
            (op::SET_PA_CFG >> 8) as u8,
            op::SET_PA_CFG as u8,
            pa.pa_sel,
            pa.pa_reg_supply,
            pa.pa_duty_cycle,
            pa.pa_hp_sel,
        ])
        .await?;
        self.command(&[
            (op::SET_TX_PARAMS >> 8) as u8,
            op::SET_TX_PARAMS as u8,
            pa.tx_power as u8, // signed dBm
            RAMP_48_US,
        ])
        .await
    }

    async fn cfg_rx_boosted(&mut self, enable: bool) -> Result<(), Error> {
        self.command(&[
            (op::SET_RX_BOOSTED >> 8) as u8,
            op::SET_RX_BOOSTED as u8,
            u8::from(enable),
        ])
        .await
    }
}

/// LR1110 version `type` field that identifies the chip (vs LR1120/LR1121).
const LR1110_VERSION_TYPE: u8 = 0x01;

fn lora_ldro(sf: SpreadingFactor, bw: Bandwidth) -> u8 {
    // RNode convention: enable LDRO when (2^SF) / BW_kHz > 16.
    u8::from((1u32 << (sf as u32)) / bw.khz() > 16)
}

/// LR11xx RSSI is a 0.5 dB-step magnitude that is negated to dBm. The +1 right
/// shift mirrors Semtech's `-(int8_t)(rssi >> 1)` (Clear BSD).
fn decode_rssi_dbm(encoded: u8) -> i16 {
    -i16::from(encoded >> 1)
}

fn antenna_referred_rssi_dbm(encoded: u8, external_rx_gain_db: u8) -> i16 {
    decode_rssi_dbm(encoded).saturating_sub(i16::from(external_rx_gain_db))
}

/// PA duty-cycle / hp_sel configs per requested dBm, transcribed verbatim from
/// Seeed's `ral_lr11xx_bsp.c` (`LR11XX_PA_LP_LF_CFG_TABLE` /
/// `LR11XX_PA_HP_LF_CFG_TABLE`) for the T1000-E board. A single fixed
/// duty_cycle/hp_sel pair is only correct for one specific dBm target — using
/// it indiscriminately mis-biases the PA outside that point.
const PA_LP_MIN_DBM: i8 = -17;
const PA_LP_MAX_DBM: i8 = 15;
const PA_HP_MIN_DBM: i8 = -9;
const PA_HP_MAX_DBM: i8 = 22;

const PA_LP_DUTY_CYCLE: [u8; 33] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x02, 0x03, 0x04,
    0x07,
];
const PA_LP_HP_SEL: [u8; 33] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00,
];
const PA_HP_DUTY_CYCLE: [u8; 32] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x02, 0x04, 0x00, 0x00, 0x01, 0x02, 0x00, 0x04, 0x02,
    0x01, 0x04, 0x00, 0x01, 0x02, 0x03, 0x00, 0x01, 0x04, 0x01, 0x02, 0x01, 0x03, 0x03, 0x04, 0x04,
];
const PA_HP_HP_SEL: [u8; 32] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x02, 0x01, 0x02,
    0x03, 0x02, 0x01, 0x01, 0x01, 0x01, 0x03, 0x03, 0x02, 0x04, 0x04, 0x06, 0x05, 0x07, 0x06, 0x07,
];

const PA_SEL_LP: u8 = 0x00;
const PA_SEL_HP: u8 = 0x01;
const PA_SUPPLY_VREG: u8 = 0x00;
const PA_SUPPLY_VBAT: u8 = 0x01;

struct PaConfig {
    pa_sel: u8,
    pa_reg_supply: u8,
    pa_duty_cycle: u8,
    pa_hp_sel: u8,
    tx_power: i8,
}

fn pa_config(power_dbm: i8) -> PaConfig {
    let level = power_dbm.clamp(PA_LP_MIN_DBM, PA_HP_MAX_DBM);
    if level <= PA_LP_MAX_DBM {
        let idx = (level - PA_LP_MIN_DBM) as usize;
        PaConfig {
            pa_sel: PA_SEL_LP,
            pa_reg_supply: PA_SUPPLY_VREG,
            pa_duty_cycle: PA_LP_DUTY_CYCLE[idx],
            pa_hp_sel: PA_LP_HP_SEL[idx],
            tx_power: level,
        }
    } else {
        let idx = (level - PA_HP_MIN_DBM) as usize;
        PaConfig {
            pa_sel: PA_SEL_HP,
            pa_reg_supply: PA_SUPPLY_VBAT,
            pa_duty_cycle: PA_HP_DUTY_CYCLE[idx],
            pa_hp_sel: PA_HP_HP_SEL[idx],
            tx_power: level,
        }
    }
}