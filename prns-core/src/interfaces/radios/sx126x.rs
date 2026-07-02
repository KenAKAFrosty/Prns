//! Our own SX1262 sub-GHz radio driver.
//!
//! HAL-agnostic: the one driver body is generic over `embedded-hal-async` (an
//! `SpiDevice`, BUSY and DIO1 as `Wait` pins, RESET as an `OutputPin`, and a `DelayNs`),
//! so the same code drives the SX1262 on any MCU — proven crossing real air between the
//! Heltec V4 (esp-hal / Xtensa) and the LilyGo T-Echo (embassy-nrf / Cortex-M). Only a
//! small [`BoardConfig`] differs per board.
//!
//! [`Modulation`] is the change point: today the [`Modulation::Lora`] arm issues the
//! SX1262's LoRa packet-engine commands; the GFSK/FSK arms are the reserved seam the
//! `SetPacketType` / `SetModulationParams` split lights up later.
//!
//! Build note: on nRF/Cortex-M (`thumbv7em`) this must be built with `lto = "thin"` or
//! `lto = false` — `lto = "fat"` miscompiles the command sequence into a layout-dependent
//! boot HardFault on that target (the Xtensa/esp-hal path is unaffected).

use core::future::{poll_fn, Future};
use core::task::Poll;

use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::{Operation, SpiDevice};

/// SX1262 command opcodes (datasheet table 11-1). Kept as a complete reference table; the
/// GFSK seam and sleep mode use the rest.
#[allow(dead_code)]
mod op {
    pub const SET_SLEEP: u8 = 0x84;
    pub const SET_STANDBY: u8 = 0x80;
    pub const SET_TX: u8 = 0x83;
    pub const SET_RX: u8 = 0x82;
    pub const SET_REGULATOR_MODE: u8 = 0x96;
    pub const CALIBRATE: u8 = 0x89;
    pub const CALIBRATE_IMAGE: u8 = 0x98;
    pub const SET_PA_CONFIG: u8 = 0x95;
    pub const WRITE_REGISTER: u8 = 0x0D;
    pub const READ_REGISTER: u8 = 0x1D;
    pub const WRITE_BUFFER: u8 = 0x0E;
    pub const READ_BUFFER: u8 = 0x1E;
    pub const SET_DIO_IRQ_PARAMS: u8 = 0x08;
    pub const GET_IRQ_STATUS: u8 = 0x12;
    pub const CLEAR_IRQ_STATUS: u8 = 0x02;
    pub const SET_DIO2_AS_RF_SWITCH_CTRL: u8 = 0x9D;
    pub const SET_DIO3_AS_TCXO_CTRL: u8 = 0x97;
    pub const SET_RF_FREQUENCY: u8 = 0x86;
    pub const SET_PACKET_TYPE: u8 = 0x8A;
    pub const SET_TX_PARAMS: u8 = 0x8E;
    pub const SET_MODULATION_PARAMS: u8 = 0x8B;
    pub const SET_PACKET_PARAMS: u8 = 0x8C;
    pub const SET_BUFFER_BASE_ADDRESS: u8 = 0x8F;
    pub const GET_RX_BUFFER_STATUS: u8 = 0x13;
    pub const GET_RSSI_INST: u8 = 0x15;
    pub const GET_STATUS: u8 = 0xC0;
    pub const CLEAR_DEVICE_ERRORS: u8 = 0x07;
    pub const SET_STOP_RX_TIMER_ON_PREAMBLE: u8 = 0x9F;
    pub const SET_LORA_SYMB_NUM_TIMEOUT: u8 = 0xA0;
}

/// SX1262 registers we touch (datasheet table 12-1).
mod reg {
    /// LoRa sync word, high byte (low byte at +1).
    pub const LORA_SYNC_WORD_MSB: u16 = 0x0740;
    /// TX clamp config — errata 15.2 workaround.
    pub const TX_CLAMP_CONFIG: u16 = 0x08D8;
    /// RX gain — 0x96 for boosted-gain RX.
    pub const RX_GAIN: u16 = 0x08AC;
    /// TX modulation quality — errata 15.1 workaround (bit 2).
    pub const TX_MODULATION: u16 = 0x0889;
    /// IQ polarity — errata 15.4 workaround (bit 2).
    pub const IQ_POLARITY: u16 = 0x0736;
}

/// IRQ status bit masks (datasheet table 13-29). Complete reference; not every bit is
/// discriminated in the LoRa path.
#[allow(dead_code)]
mod irq {
    pub const TX_DONE: u16 = 1 << 0;
    pub const RX_DONE: u16 = 1 << 1;
    pub const PREAMBLE_DETECTED: u16 = 1 << 2;
    pub const HEADER_VALID: u16 = 1 << 4;
    pub const HEADER_ERR: u16 = 1 << 5;
    pub const CRC_ERR: u16 = 1 << 6;
    pub const TIMEOUT: u16 = 1 << 9;
    /// The SX1262's literal "all IRQs" mask (lora-phy unmasks 0xFFFF onto DIO1 for RX).
    pub const ALL: u16 = 0xFFFF;
}

/// The TCXO supply voltage the SX1262 drives out of DIO3 (datasheet table 13-35).
/// Both our boards use 1.8 V.
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

/// LoRa spreading factor — the SX1262 byte is the SF number itself.
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

/// LoRa bandwidth — the SX1262 modulation-param code (datasheet table 13-38).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Bandwidth {
    Bw125 = 0x04,
    Bw250 = 0x05,
    Bw500 = 0x06,
}

/// LoRa coding rate — the SX1262 modulation-param code (datasheet table 13-39).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodingRate {
    Cr4_5 = 0x01,
    Cr4_6 = 0x02,
    Cr4_7 = 0x03,
    Cr4_8 = 0x04,
}

/// The change point. Today only LoRa is driven; the GFSK arm is the reserved seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    Lora {
        spreading_factor: SpreadingFactor,
        bandwidth: Bandwidth,
        coding_rate: CodingRate,
    },
}

/// LoRa on-air packet shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoraPacket {
    pub preamble_symbols: u16,
    pub explicit_header: bool,
    pub crc_on: bool,
    pub invert_iq: bool,
}

/// The private LoRa network sync word (RNode): 0x1424.
pub const PRIVATE_SYNC_WORD: u16 = 0x1424;

/// SX1262 LoRa max payload — the on-air length field is a single byte.
const MAX_LORA_PAYLOAD: usize = 255;

/// Longest the SX1262 should ever hold BUSY: command processing is tens of microseconds; the worst
/// legitimate case is the cold-start calibration with TCXO startup (~15 ms). BUSY gates every SPI
/// command, so an unbounded wait here lets a single wedged-high line (lost TCXO, a brown-out during a
/// coex current spike, an SPI desync) hang the whole radio task — AND every recovery command with it.
/// Past this we surface [`Error::Busy`] instead, so the caller can hard-reset the chip and move on.
const BUSY_TIMEOUT_MS: u32 = 100;

/// Longest a single LoRa frame can sit on air before TxDone. The worst supported case — SF12 / BW125,
/// a full 255-byte frame at CR4:8 with LDRO — is ~14 s of airtime, so this clears even that with
/// margin and never aborts a legitimate transmit. `SetTx` runs with the chip's own timeout disabled
/// (single-shot), so the TxDone IRQ is otherwise unbounded; a wait past this means the PA or IRQ path
/// faulted and never will, surfaced as [`Error::Timeout`] for a re-init.
const TX_DONE_TIMEOUT_MS: u32 = 20_000;

/// Race a hardware-wait future against the board delay, so a pin that never reaches its level (a
/// wedged SX1262) becomes a recoverable error instead of an infinite hang. The radio's own
/// `DelayNs` is the clock, so this stays HAL-agnostic — no `embassy-time` dependency in the driver.
/// `pin_err` is returned if the wait itself errors; `timeout_err` if the deadline wins first.
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

/// Per-board wiring/analog facts the one driver body needs.
#[derive(Debug, Clone, Copy)]
pub struct BoardConfig {
    /// `Some(v)` if a TCXO is fed from DIO3 at voltage `v`; `None` for a bare XTAL.
    pub tcxo_voltage: Option<TcxoVoltage>,
    /// Use the on-chip DC-DC (vs LDO-only).
    pub use_dcdc: bool,
    /// Apply the boosted-gain RX register.
    pub rx_boost: bool,
    /// Let the SX1262 drive its own RF switch off DIO2 (true for both our boards).
    pub dio2_as_rf_switch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Spi,
    Busy,
    Dio1,
    Reset,
    /// An on-air receive failed its CRC.
    Crc,
    /// A TX or RX completed with the SX1262's timeout IRQ set.
    Timeout,
    /// The receive buffer was smaller than the frame.
    BufferTooSmall,
}

/// The SX1262 driver. One body for every board; the pin/bus types are the only
/// per-platform variation.
pub struct Sx126x<SPI, BUSY, DIO1, RST, DLY> {
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
    /// RAM staging for the TX FIFO write — DMA-class SPI can't source a flash-resident
    /// payload. A field, not a per-call stack local, so it never bloats the `transmit`
    /// future or the shared node stack.
    tx_staging: [u8; MAX_LORA_PAYLOAD],
}

impl<SPI, BUSY, DIO1, RST, DLY> Sx126x<SPI, BUSY, DIO1, RST, DLY>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    pub fn new(
        spi: SPI,
        busy: BUSY,
        dio1: DIO1,
        reset: RST,
        delay: DLY,
        config: BoardConfig,
    ) -> Self {
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
                preamble_symbols: 8,
                explicit_header: true,
                crc_on: true,
                invert_iq: false,
            },
            tx_power_dbm: 14,
            tx_staging: [0u8; MAX_LORA_PAYLOAD],
        }
    }

    /// Wait for the SX1262 to lower BUSY before clocking the next command, bounded by
    /// [`BUSY_TIMEOUT_MS`] so a wedged-high line can't hang the radio task forever.
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

    /// Hard-reset the chip: hold RESET low, release, wait for BUSY low. Timing matches
    /// the lora-phy sx126x interface variant (10 ms settle, 20 ms low, 10 ms rise).
    async fn hard_reset(&mut self) -> Result<(), Error> {
        self.delay.delay_ms(10).await;
        self.reset.set_low().map_err(|_| Error::Reset)?;
        self.delay.delay_ms(20).await;
        self.reset.set_high().map_err(|_| Error::Reset)?;
        self.delay.delay_ms(10).await;
        self.wait_busy().await
    }

    /// Send one command frame (opcode + params) in a single CS assertion.
    async fn command(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        self.spi.write(bytes).await.map_err(|_| Error::Spi)
    }

    /// Send `opcode`, then clock `buf.len()` bytes back: `buf[0]` is the status byte,
    /// `buf[1..]` the requested data.
    async fn read_command(&mut self, opcode: u8, buf: &mut [u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        self.spi
            .transaction(&mut [Operation::Write(&[opcode]), Operation::Read(buf)])
            .await
            .map_err(|_| Error::Spi)
    }

    async fn write_register(&mut self, addr: u16, data: &[u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        let header = [op::WRITE_REGISTER, (addr >> 8) as u8, addr as u8];
        self.spi
            .transaction(&mut [Operation::Write(&header), Operation::Write(data)])
            .await
            .map_err(|_| Error::Spi)
    }

    async fn read_register(&mut self, addr: u16, buf: &mut [u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        // opcode, addr hi/lo, one status NOP, then the register bytes.
        let header = [op::READ_REGISTER, (addr >> 8) as u8, addr as u8, 0x00];
        self.spi
            .transaction(&mut [Operation::Write(&header), Operation::Read(buf)])
            .await
            .map_err(|_| Error::Spi)
    }

    /// Write the first `len` bytes of the `tx_staging` field into the SX1262 FIFO at offset 0.
    /// Splits the `spi` / `tx_staging` borrows so the staged payload feeds the transaction
    /// without an extra copy.
    async fn write_tx_payload(&mut self, len: usize) -> Result<(), Error> {
        self.wait_busy().await?;
        let Self {
            spi, tx_staging, ..
        } = self;
        let header = [op::WRITE_BUFFER, 0x00];
        spi.transaction(&mut [
            Operation::Write(&header),
            Operation::Write(&tx_staging[..len]),
        ])
        .await
        .map_err(|_| Error::Spi)
    }

    async fn read_buffer(&mut self, offset: u8, buf: &mut [u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        // opcode, offset, one status NOP, then the payload bytes.
        let header = [op::READ_BUFFER, offset, 0x00];
        self.spi
            .transaction(&mut [Operation::Write(&header), Operation::Read(buf)])
            .await
            .map_err(|_| Error::Spi)
    }

    async fn irq_status(&mut self) -> Result<u16, Error> {
        let mut buf = [0u8; 3];
        self.read_command(op::GET_IRQ_STATUS, &mut buf).await?;
        Ok(((buf[1] as u16) << 8) | buf[2] as u16)
    }

    async fn clear_irq(&mut self, mask: u16) -> Result<(), Error> {
        self.command(&[op::CLEAR_IRQ_STATUS, (mask >> 8) as u8, mask as u8])
            .await
    }
}

impl<SPI, BUSY, DIO1, RST, DLY> Sx126x<SPI, BUSY, DIO1, RST, DLY>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    /// Reset the SX1262, run the LoRa cold-start sequence, then apply the channel config
    /// via [`configure`](Self::configure). Leaves the chip in standby, fully configured for
    /// `frequency_hz` / `modulation` / `packet` / `tx_power_dbm`; call
    /// [`transmit`](Self::transmit) / [`receive`](Self::receive) next.
    pub async fn init(
        &mut self,
        frequency_hz: u32,
        modulation: Modulation,
        packet: LoraPacket,
        tx_power_dbm: i8,
    ) -> Result<(), Error> {
        self.freq_hz = frequency_hz;
        self.modulation = modulation;
        self.packet = packet;
        self.tx_power_dbm = tx_power_dbm;

        self.hard_reset().await?;
        self.command(&[op::GET_STATUS, 0x00]).await?;
        self.standby().await?;
        if self.config.use_dcdc {
            self.command(&[op::SET_REGULATOR_MODE, 0x01]).await?;
        }
        if self.config.dio2_as_rf_switch {
            self.command(&[op::SET_DIO2_AS_RF_SWITCH_CTRL, 0x01])
                .await?;
        }
        if let Some(voltage) = self.config.tcxo_voltage {
            self.command(&[op::CLEAR_DEVICE_ERRORS, 0x00, 0x00]).await?;
            // TCXO startup timeout 640 (= 10 ms / 15.625 us), MSB-first.
            self.command(&[op::SET_DIO3_AS_TCXO_CTRL, voltage as u8, 0x00, 0x02, 0x80])
                .await?;
            self.command(&[op::CALIBRATE, 0x7F]).await?;
        }
        self.command(&[op::SET_PACKET_TYPE, 0x01]).await?;
        self.write_register(reg::LORA_SYNC_WORD_MSB, &PRIVATE_SYNC_WORD.to_be_bytes())
            .await?;
        self.command(&[op::SET_BUFFER_BASE_ADDRESS, 0x00, 0x00])
            .await?;
        let [image_cal_a, image_cal_b] = image_calibration_pair(frequency_hz);
        self.command(&[op::CALIBRATE_IMAGE, image_cal_a, image_cal_b])
            .await?;
        self.configure().await?;
        self.route_irqs_and_tune_rx().await
    }

    /// Apply the channel config — modulation, frequency, TX power, packet shape — to the
    /// chip. The SX1262 RETAINS these in its registers across SetStandby / SetTx / SetRx
    /// (only Sleep or reset clears them), so this runs ONCE from [`init`](Self::init), and
    /// again only on a discrete channel change — never per packet. The per-packet path
    /// then only restamps the payload length and writes the buffer.
    pub async fn configure(&mut self) -> Result<(), Error> {
        self.set_modulation_params().await?; // + TxModulation errata
        self.set_tx_power().await?; // TxClampCfg errata + PA config + tx params
        self.set_packet_params(0xFF).await?; // + IQPolarity errata; 0xFF = RX-friendly max
        self.set_rf_frequency().await
    }

    /// Route IRQs and arm the RX front-end — once, after [`configure`](Self::configure). The
    /// SX1262 RETAINS all of these across SetStandby / SetTx / SetRx (proven on hardware: a TX
    /// completes with the IRQ mask set only here, and a read-back showed the boosted RX gain
    /// holding at 0x96 cycle after cycle), so they belong in [`init`](Self::init), not the
    /// per-frame path. All IRQs are unmasked onto DIO1 (TxDone / RxDone / CrcErr / Timeout are
    /// discriminated in software for both directions); the RX preamble timer, symbol timeout,
    /// and gain take their listening values. Independent of the channel, so a channel change
    /// re-runs `configure` but not this.
    async fn route_irqs_and_tune_rx(&mut self) -> Result<(), Error> {
        let all = irq::ALL.to_be_bytes();
        self.command(&[
            op::SET_DIO_IRQ_PARAMS,
            all[0],
            all[1],
            all[0],
            all[1],
            0,
            0,
            0,
            0,
        ])
        .await?;
        self.command(&[op::SET_STOP_RX_TIMER_ON_PREAMBLE, 0x01])
            .await?;
        self.command(&[op::SET_LORA_SYMB_NUM_TIMEOUT, 0x00]).await?;
        if self.config.rx_boost {
            self.write_register(reg::RX_GAIN, &[0x96]).await?;
        }
        Ok(())
    }

    /// Transmit one LoRa frame and wait for TxDone. The channel must already be configured
    /// (via [`init`](Self::init) / [`configure`](Self::configure)); only the payload length
    /// is restamped per frame, so this is safe to interleave with [`receive`](Self::receive).
    pub async fn transmit(&mut self, payload: &[u8]) -> Result<(), Error> {
        let len = payload.len();
        if len > MAX_LORA_PAYLOAD {
            return Err(Error::BufferTooSmall);
        }
        // EasyDMA (and most SPI DMA) can only source from RAM; the caller's payload may
        // be flash-resident (`&'static`), so stage it through the RAM `tx_staging` field.
        self.tx_staging[..len].copy_from_slice(payload);

        self.standby().await?;
        // Modulation / power / frequency / packet shape were set by `configure`, and IRQs by
        // `route_irqs_and_tune_rx`; the chip retains both. Only the per-frame payload length
        // and FIFO contents change here.
        self.set_payload_length(len as u8).await?;
        self.write_tx_payload(len).await?;
        self.clear_irq(irq::ALL).await?;
        // SetTx with timeout 0 = single shot, no chip timeout — so the TxDone wait is bounded here
        // ([`TX_DONE_TIMEOUT_MS`]); a TX that never completes must not trap the radio task forever.
        self.command(&[op::SET_TX, 0x00, 0x00, 0x00]).await?;
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

    /// Arm continuous RX: restamp the RX-side max payload length, clear stale IRQs, and enter
    /// SetRx continuous. The radio then listens until a frame completes; [`read_frame`](Self::read_frame)
    /// waits for it WITHOUT re-arming, so a host-side select that cancels the read mid-listen leaves
    /// the radio receiving (the RxDone IRQ latches) rather than guillotining an in-flight frame — the
    /// difference between catching a long packet (a multi-hundred-ms LoRa announce) and never seeing
    /// one. Channel config / IRQ routing / RX front-end persist from init, so this only re-arms.
    pub async fn arm_rx(&mut self) -> Result<(), Error> {
        self.standby().await?;
        self.set_payload_length(0xFF).await?;
        self.clear_irq(irq::ALL).await?;
        // SetRx 0xFFFFFF = continuous.
        self.command(&[op::SET_RX, 0xFF, 0xFF, 0xFF]).await
    }

    /// Wait for one frame on an already-[`arm_rx`](Self::arm_rx)'d radio, written into `buf`. The
    /// radio stays in continuous RX, so call again for the next frame. `Err(Crc)` on a CRC failure,
    /// `Err(BufferTooSmall)` if the frame exceeds `buf`. Blocks until RxDone — bound it with a
    /// host-side timeout/select; cancelling the wait does NOT drop the radio's RX.
    pub async fn read_frame(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        loop {
            self.dio1.wait_for_high().await.map_err(|_| Error::Dio1)?;
            let flags = self.irq_status().await?;
            self.clear_irq(flags).await?;
            if flags & irq::RX_DONE != 0 {
                if flags & irq::CRC_ERR != 0 {
                    return Err(Error::Crc);
                }
                let mut status = [0u8; 3];
                self.read_command(op::GET_RX_BUFFER_STATUS, &mut status)
                    .await?;
                let len = status[1] as usize;
                let offset = status[2];
                if len > buf.len() {
                    return Err(Error::BufferTooSmall);
                }
                self.read_buffer(offset, &mut buf[..len]).await?;
                return Ok(len);
            }
            if flags & irq::TIMEOUT != 0 {
                return Err(Error::Timeout);
            }
        }
    }

    /// Arm RX and wait for one frame — [`arm_rx`](Self::arm_rx) then [`read_frame`](Self::read_frame).
    /// Convenient for a request/response or test caller; a continuous listener should `arm_rx` once
    /// and loop on `read_frame` so the per-frame re-arm never races a long packet's airtime.
    pub async fn receive(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        self.arm_rx().await?;
        self.read_frame(buf).await
    }

    /// Read the instantaneous channel RSSI in dBm — valid while the radio is in RX (armed by
    /// [`arm_rx`](Self::arm_rx)). This is the carrier-sense a listen-before-talk transmitter checks:
    /// a level well above the noise floor means a frame is on air, so it should hold off.
    pub async fn channel_rssi_dbm(&mut self) -> Result<i16, Error> {
        let mut buf = [0u8; 2];
        self.read_command(op::GET_RSSI_INST, &mut buf).await?;
        // SX1262 datasheet 13.5.3: RssiInst = -RssiInst_byte / 2 dBm.
        Ok(-(buf[1] as i16) / 2)
    }

    async fn standby(&mut self) -> Result<(), Error> {
        self.command(&[op::SET_STANDBY, 0x00]).await
    }

    async fn set_rf_frequency(&mut self) -> Result<(), Error> {
        // PLL step = freq * 2^25 / 32 MHz (u64 to avoid overflow).
        let steps = (((self.freq_hz as u64) << 25) / 32_000_000) as u32;
        let b = steps.to_be_bytes();
        self.command(&[op::SET_RF_FREQUENCY, b[0], b[1], b[2], b[3]])
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
                    op::SET_MODULATION_PARAMS,
                    spreading_factor as u8,
                    bandwidth as u8,
                    coding_rate as u8,
                    ldro,
                ])
                .await?;
                // TxModulation errata (DS 15.1): set bit 2 unless BW500.
                let mut v = [0u8; 1];
                self.read_register(reg::TX_MODULATION, &mut v).await?;
                let fixed = if bandwidth == Bandwidth::Bw500 {
                    v[0] & !0x04
                } else {
                    v[0] | 0x04
                };
                self.write_register(reg::TX_MODULATION, &[fixed]).await
            }
        }
    }

    /// Lean per-frame call: SetPacketParams with the stored (constant) packet shape and
    /// `payload_len`. No IQ-polarity errata RMW — [`set_packet_params`](Self::set_packet_params)
    /// applies that once in [`configure`](Self::configure).
    async fn set_payload_length(&mut self, payload_len: u8) -> Result<(), Error> {
        let pre = self.packet.preamble_symbols.to_be_bytes();
        let header = u8::from(!self.packet.explicit_header);
        let crc = u8::from(self.packet.crc_on);
        let iq = u8::from(self.packet.invert_iq);
        self.command(&[
            op::SET_PACKET_PARAMS,
            pre[0],
            pre[1],
            header,
            payload_len,
            crc,
            iq,
        ])
        .await
    }

    async fn set_packet_params(&mut self, payload_len: u8) -> Result<(), Error> {
        self.set_payload_length(payload_len).await?;
        // IQPolarity errata (DS 15.4): set bit 2 unless inverted IQ.
        let mut v = [0u8; 1];
        self.read_register(reg::IQ_POLARITY, &mut v).await?;
        let fixed = if self.packet.invert_iq {
            v[0] & !0x04
        } else {
            v[0] | 0x04
        };
        self.write_register(reg::IQ_POLARITY, &[fixed]).await
    }

    async fn set_tx_power(&mut self) -> Result<(), Error> {
        // TxClampCfg errata (DS 15.2): set bits 1-4.
        let mut v = [0u8; 1];
        self.read_register(reg::TX_CLAMP_CONFIG, &mut v).await?;
        self.write_register(reg::TX_CLAMP_CONFIG, &[v[0] | 0x1E])
            .await?;

        let (pa_duty, hp_max, tx_power) = pa_params(self.tx_power_dbm);
        self.command(&[op::SET_PA_CONFIG, pa_duty, hp_max, 0x00, 0x01])
            .await?;
        // Ramp 40 us (0x02) for a TX preparation.
        self.command(&[op::SET_TX_PARAMS, tx_power, 0x02]).await
    }
}

/// LoRa low-data-rate optimize: on only for the slow SF/BW combos (DS 6.1.4).
fn lora_ldro(sf: SpreadingFactor, bw: Bandwidth) -> u8 {
    u8::from(matches!(
        (sf, bw),
        (
            SpreadingFactor::Sf11 | SpreadingFactor::Sf12,
            Bandwidth::Bw125
        ) | (SpreadingFactor::Sf12, Bandwidth::Bw250)
    ))
}

fn image_calibration_pair(frequency_hz: u32) -> [u8; 2] {
    match frequency_hz {
        430_000_000..=440_000_000 => [0x6B, 0x6F],
        470_000_000..=510_000_000 => [0x75, 0x81],
        779_000_000..=787_000_000 => [0xC1, 0xC5],
        863_000_000..=870_000_000 => [0xD7, 0xDB],
        902_000_000..=928_000_000 => [0xE1, 0xE9],
        _ => [0xE1, 0xE9],
    }
}

/// SX1262 high-power-PA config for a clamped dBm: (paDutyCycle, hpMax, SetTxParams power).
fn pa_params(power_dbm: i8) -> (u8, u8, u8) {
    let txp = power_dbm.clamp(-9, 22);
    match txp {
        21..=22 => (0x04, 0x07, 22),
        18..=20 => (0x03, 0x05, (txp + 2) as u8),
        15..=17 => (0x02, 0x03, (txp + 5) as u8),
        _ => (0x02, 0x02, (txp + 8) as u8),
    }
}

#[cfg(test)]
mod tests {
    //! Drives `init`/`transmit`/`receive` against a mock `SpiDevice` that records every
    //! command and answers reads with canned values, then asserts the recorded command
    //! stream byte-for-byte against the lora-phy oracle — pins the wire-relevant encoding
    //! (opcodes, parameter order, syncword, frequency, errata RMWs) with no hardware.

    use super::*;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};
    use std::cell::RefCell;
    use std::rc::Rc;

    use embedded_hal::digital::{
        Error as DigError, ErrorKind as DigErrorKind, ErrorType as DigErrorType, OutputPin,
    };
    use embedded_hal::spi::{
        Error as SpiError, ErrorKind as SpiErrorKind, ErrorType as SpiErrorType, Operation,
    };
    use embedded_hal_async::delay::DelayNs;
    use embedded_hal_async::digital::Wait;
    use embedded_hal_async::spi::SpiDevice;

    #[derive(Debug)]
    struct MockErr;
    impl SpiError for MockErr {
        fn kind(&self) -> SpiErrorKind {
            SpiErrorKind::Other
        }
    }
    impl DigError for MockErr {
        fn kind(&self) -> DigErrorKind {
            DigErrorKind::Other
        }
    }

    type Log = Rc<RefCell<Vec<Vec<u8>>>>;

    struct MockSpi {
        log: Log,
    }
    impl SpiErrorType for MockSpi {
        type Error = MockErr;
    }
    impl SpiDevice<u8> for MockSpi {
        async fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), MockErr> {
            // The leading run of writes is the command header (opcode + addr/offset).
            let mut header = Vec::new();
            for op in ops.iter() {
                match op {
                    Operation::Write(w) => header.extend_from_slice(w),
                    _ => break,
                }
            }
            let mut full = Vec::new();
            for op in ops.iter() {
                if let Operation::Write(w) = op {
                    full.extend_from_slice(w);
                }
            }
            if !full.is_empty() {
                self.log.borrow_mut().push(full);
            }
            for op in ops.iter_mut() {
                if let Operation::Read(buf) = op {
                    fill_read(&header, buf);
                }
            }
            Ok(())
        }
    }

    /// Canned read responses keyed on the command opcode.
    fn fill_read(header: &[u8], buf: &mut [u8]) {
        match header.first().copied().unwrap_or(0) {
            // GetIrqStatus -> status + TxDone|RxDone, so both flows complete.
            0x12 => {
                if buf.len() >= 3 {
                    buf[0] = 0x00;
                    buf[1] = 0x00;
                    buf[2] = 0x03;
                }
            }
            // GetRxBufferStatus -> status, len 16, offset 0.
            0x13 => {
                if buf.len() >= 3 {
                    buf[0] = 0x00;
                    buf[1] = 16;
                    buf[2] = 0x00;
                }
            }
            // ReadBuffer -> a 16-byte canned payload.
            0x1E => {
                let p = b"PRNS-HELTEC-SMOK";
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = p.get(i).copied().unwrap_or(0);
                }
            }
            // ReadRegister and the rest read back 0 (errata RMW then sets bit 2 -> 0x04).
            _ => buf.iter_mut().for_each(|b| *b = 0),
        }
    }

    struct MockWait;
    impl DigErrorType for MockWait {
        type Error = MockErr;
    }
    impl Wait for MockWait {
        async fn wait_for_high(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        async fn wait_for_low(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        async fn wait_for_rising_edge(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        async fn wait_for_falling_edge(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        async fn wait_for_any_edge(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
    }

    struct MockOut;
    impl DigErrorType for MockOut {
        type Error = MockErr;
    }
    impl OutputPin for MockOut {
        fn set_low(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
    }

    struct MockDelay;
    impl DelayNs for MockDelay {
        async fn delay_ns(&mut self, _ns: u32) {}
    }

    /// A pin whose `wait_for_low` never resolves — models a BUSY line wedged high (lost TCXO,
    /// brown-out, SPI desync). Under the old unbounded `wait_busy` this would hang the radio task
    /// forever; the deadline must convert it to `Error::Busy`.
    struct StuckLow;
    impl DigErrorType for StuckLow {
        type Error = MockErr;
    }
    impl Wait for StuckLow {
        async fn wait_for_high(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        async fn wait_for_low(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
        async fn wait_for_rising_edge(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
        async fn wait_for_falling_edge(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
        async fn wait_for_any_edge(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
    }

    /// A pin whose `wait_for_high` never resolves — models a DIO1 line that never raises TxDone (a
    /// PA/IRQ fault). `transmit`'s TxDone wait must convert it to `Error::Timeout`.
    struct StuckHigh;
    impl DigErrorType for StuckHigh {
        type Error = MockErr;
    }
    impl Wait for StuckHigh {
        async fn wait_for_high(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
        async fn wait_for_low(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        async fn wait_for_rising_edge(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
        async fn wait_for_falling_edge(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
        async fn wait_for_any_edge(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
    }

    fn board() -> BoardConfig {
        BoardConfig {
            tcxo_voltage: Some(TcxoVoltage::V1_8),
            use_dcdc: true,
            rx_boost: true,
            dio2_as_rf_switch: true,
        }
    }

    /// Every mock future is immediately ready, so a noop-waker poll loop drives the driver
    /// future to completion. No `unsafe`: `Waker::noop` is stable (Rust 1.85+).
    fn block_on<F: Future>(f: F) -> F::Output {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut f = Box::pin(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn command_stream_matches_lora_phy_oracle() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let board = BoardConfig {
            tcxo_voltage: Some(TcxoVoltage::V1_8),
            use_dcdc: true,
            rx_boost: true,
            dio2_as_rf_switch: true,
        };
        let mut radio = Sx126x::new(
            MockSpi { log: log.clone() },
            MockWait,
            MockWait,
            MockOut,
            MockDelay,
            board,
        );
        let modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf8,
            bandwidth: Bandwidth::Bw125,
            coding_rate: CodingRate::Cr4_5,
        };
        let packet = LoraPacket {
            preamble_symbols: 18,
            explicit_header: true,
            crc_on: true,
            invert_iq: false,
        };

        block_on(radio.init(915_000_000, modulation, packet, 14)).expect("init");
        block_on(radio.transmit(b"PRNS-HELTEC-SMOK")).expect("transmit");
        let mut buf = [0u8; 255];
        let n = block_on(radio.receive(&mut buf)).expect("receive");
        assert_eq!(n, 16, "received frame length");
        assert_eq!(&buf[..n], b"PRNS-HELTEC-SMOK");
        // A second receive: the once-only RX setup must NOT be re-issued — only arming repeats.
        let n2 = block_on(radio.receive(&mut buf)).expect("receive 2");
        assert_eq!(n2, 16, "second received frame length");

        let cmds = log.borrow();
        let has = |bytes: &[u8]| cmds.iter().any(|c| c.as_slice() == bytes);
        let count = |bytes: &[u8]| cmds.iter().filter(|c| c.as_slice() == bytes).count();

        // init — the channel-defining bytes
        assert!(has(&[0x80, 0x00]), "SetStandby RC");
        assert!(has(&[0x96, 0x01]), "SetRegulatorMode DCDC");
        assert!(has(&[0x9D, 0x01]), "SetDIO2AsRfSwitch");
        assert!(has(&[0x97, 0x02, 0x00, 0x02, 0x80]), "SetTCXOMode 1.8V/640");
        assert!(has(&[0x89, 0x7F]), "Calibrate all");
        assert!(has(&[0x8A, 0x01]), "SetPacketType LoRa");
        assert!(
            has(&[0x0D, 0x07, 0x40, 0x14, 0x24]),
            "Syncword 0x1424 (private)"
        );
        assert!(has(&[0x8F, 0x00, 0x00]), "SetBufferBaseAddress");
        assert!(has(&[0x98, 0xE1, 0xE9]), "CalibrateImage 915 band");
        // configure (once, in init): modulation / PA / tx params / freq
        assert!(
            has(&[0x8B, 0x08, 0x04, 0x01, 0x00]),
            "SetModulationParams SF8/BW125/CR45/LDRO0"
        );
        assert!(has(&[0x95, 0x02, 0x02, 0x00, 0x01]), "SetPaConfig 14dBm");
        assert!(has(&[0x8E, 0x16, 0x02]), "SetTxParams power22/ramp40us");
        assert!(
            has(&[0x86, 0x39, 0x30, 0x00, 0x00]),
            "SetRfFrequency 915 MHz"
        );
        // per-frame: payload-length restamp + arm
        assert!(
            has(&[0x8C, 0x00, 0x12, 0x00, 16, 0x01, 0x00]),
            "SetPacketParams TX preamble18/explicit/len16/crc"
        );
        assert!(
            has(&[0x8C, 0x00, 0x12, 0x00, 0xFF, 0x01, 0x00]),
            "SetPacketParams RX max len"
        );
        // errata read-modify-writes (mock reads 0x00, bit-2 set -> 0x04 / bits1-4 -> 0x1E)
        assert!(has(&[0x0D, 0x08, 0x89, 0x04]), "TxModulation errata bit2");
        assert!(has(&[0x0D, 0x07, 0x36, 0x04]), "IQPolarity errata bit2");
        assert!(has(&[0x0D, 0x08, 0xD8, 0x1E]), "TxClampCfg errata bits1-4");

        // The optimization, pinned (hardware-proven bidirectional): IRQ routing + RX tuning
        // issued exactly ONCE in init, never per frame, even across two receives. Only arming
        // (SetTx / SetRx) repeats.
        assert_eq!(
            count(&[0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]),
            1,
            "unified all-IRQ mask set once"
        );
        assert_eq!(count(&[0x9F, 0x01]), 1, "SetStopRxTimerOnPreamble once");
        assert_eq!(count(&[0xA0, 0x00]), 1, "SetLoRaSymbNumTimeout once");
        assert_eq!(count(&[0x0D, 0x08, 0xAC, 0x96]), 1, "RxGain boost once");
        assert!(
            !has(&[0x08, 0x02, 0x01, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00]),
            "TX no longer re-issues its own IRQ mask"
        );
        assert_eq!(
            count(&[0x83, 0x00, 0x00, 0x00]),
            1,
            "SetTx once (one transmit)"
        );
        assert_eq!(
            count(&[0x82, 0xFF, 0xFF, 0xFF]),
            2,
            "SetRx armed once per receive (two)"
        );
    }

    #[test]
    fn image_calibration_tracks_rnode_sx126x_bands() {
        assert_eq!(image_calibration_pair(433_900_000), [0x6B, 0x6F]);
        assert_eq!(image_calibration_pair(470_000_000), [0x75, 0x81]);
        assert_eq!(image_calibration_pair(780_000_000), [0xC1, 0xC5]);
        assert_eq!(image_calibration_pair(868_000_000), [0xD7, 0xDB]);
        assert_eq!(image_calibration_pair(915_000_000), [0xE1, 0xE9]);
    }

    #[test]
    fn a_wedged_busy_line_times_out_instead_of_hanging() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut radio = Sx126x::new(
            MockSpi { log },
            StuckLow,
            MockWait,
            MockOut,
            MockDelay,
            board(),
        );
        // `arm_rx` opens with `standby` → `command` → `wait_busy`; with BUSY stuck high the old
        // driver blocked here forever. It must now surface `Error::Busy` so the worker can recover.
        let result = block_on(radio.arm_rx());
        assert_eq!(result, Err(Error::Busy));
    }

    #[test]
    fn a_txdone_that_never_fires_times_out_instead_of_hanging() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        // BUSY behaves (MockWait), but DIO1 never raises TxDone (StuckHigh). The bounded TxDone wait
        // must convert that into `Error::Timeout` rather than trapping the radio task mid-transmit.
        let mut radio = Sx126x::new(
            MockSpi { log },
            MockWait,
            StuckHigh,
            MockOut,
            MockDelay,
            board(),
        );
        let result = block_on(radio.transmit(b"PRNS-HELTEC-SMOK"));
        assert_eq!(result, Err(Error::Timeout));
    }
}
