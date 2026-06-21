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
    /// GFSK sync word, first of up to 8 consecutive bytes.
    pub const GFSK_SYNC_WORD_0: u16 = 0x06C0;
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

/// GFSK pulse shaping — the chip's Gaussian-filter BT byte, or none (datasheet table 13-41).
/// GMSK is `GaussianBt05` paired with a half-bitrate deviation (modulation index 0.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PulseShape {
    None = 0x00,
    GaussianBt03 = 0x08,
    GaussianBt05 = 0x09,
    GaussianBt07 = 0x0A,
    GaussianBt10 = 0x0B,
}

/// The change point: which packet engine the SX1262 runs. The LoRa arm drives the chirp modem;
/// the GFSK arm drives the 2-FSK modem (the speed-mode / GMSK path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    Lora {
        spreading_factor: SpreadingFactor,
        bandwidth: Bandwidth,
        coding_rate: CodingRate,
    },
    /// 2-GFSK: on-air bitrate (bps) and frequency deviation (Hz), plus the Gaussian pulse shape.
    /// The RX bandwidth is derived from those two by Carson's rule, so it is not a separate knob.
    Gfsk {
        bitrate_bps: u32,
        fdev_hz: u32,
        pulse_shape: PulseShape,
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

/// GFSK on-air packet shape: a variable-length frame (an explicit length byte) with the preamble
/// sized in bits and CRC / data-whitening toggles. The sync word value is the shared
/// [`GFSK_SYNC_WORD`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GfskPacket {
    pub preamble_bits: u16,
    pub crc_on: bool,
    pub whitening_on: bool,
}

/// The on-air packet shape, paired with the active [`Modulation`] arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketParams {
    Lora(LoraPacket),
    Gfsk(GfskPacket),
}

/// The private LoRa network sync word (RNode): 0x1424.
pub const PRIVATE_SYNC_WORD: u16 = 0x1424;

/// The GFSK sync word (4 bytes) both endpoints must share — a high-autocorrelation pattern,
/// distinct from the 0x55/0xAA preamble, so noise and foreign in-band energy don't fake a frame.
pub const GFSK_SYNC_WORD: [u8; 4] = [0x93, 0x0B, 0x51, 0xDE];

/// SX1262 max single-frame payload — the on-air length field is a single byte (LoRa and GFSK alike).
const MAX_LORA_PAYLOAD: usize = 255;

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
    packet: PacketParams,
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
            packet: PacketParams::Lora(LoraPacket {
                preamble_symbols: 8,
                explicit_header: true,
                crc_on: true,
                invert_iq: false,
            }),
            tx_power_dbm: 14,
            tx_staging: [0u8; MAX_LORA_PAYLOAD],
        }
    }

    /// Wait for the SX1262 to lower BUSY before clocking the next command.
    async fn wait_busy(&mut self) -> Result<(), Error> {
        self.busy.wait_for_low().await.map_err(|_| Error::Busy)
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
        packet: PacketParams,
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
        match self.modulation {
            Modulation::Lora { .. } => {
                self.command(&[op::SET_PACKET_TYPE, 0x01]).await?;
                self.write_register(reg::LORA_SYNC_WORD_MSB, &PRIVATE_SYNC_WORD.to_be_bytes())
                    .await?;
            }
            Modulation::Gfsk { .. } => {
                self.command(&[op::SET_PACKET_TYPE, 0x00]).await?;
                self.write_register(reg::GFSK_SYNC_WORD_0, &GFSK_SYNC_WORD)
                    .await?;
            }
        }
        self.command(&[op::SET_BUFFER_BASE_ADDRESS, 0x00, 0x00])
            .await?;
        // Image calibration for the 902-928 MHz band.
        self.command(&[op::CALIBRATE_IMAGE, 0xE1, 0xE9]).await?;
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
        if matches!(self.modulation, Modulation::Lora { .. }) {
            self.command(&[op::SET_LORA_SYMB_NUM_TIMEOUT, 0x00]).await?;
        }
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
        // SetTx with timeout 0 = single shot, no timeout.
        self.command(&[op::SET_TX, 0x00, 0x00, 0x00]).await?;
        self.dio1.wait_for_high().await.map_err(|_| Error::Dio1)?;
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
            Modulation::Gfsk {
                bitrate_bps,
                fdev_hz,
                pulse_shape,
            } => {
                // BR = 32 * Fxtal / bitrate (Fxtal 32 MHz) = 1.024e9 / bitrate; the low 3 bytes.
                let br = (1_024_000_000u64 / bitrate_bps.max(1) as u64) as u32;
                let br = br.to_be_bytes();
                // Fdev = fdev * 2^25 / Fxtal; the low 3 bytes, same scaling as the carrier.
                let fdev = (((fdev_hz as u64) << 25) / 32_000_000) as u32;
                let fdev = fdev.to_be_bytes();
                let rx_bw = gfsk_rx_bandwidth(bitrate_bps, fdev_hz);
                // No TxModulation errata here: that is a LoRa-bandwidth workaround.
                self.command(&[
                    op::SET_MODULATION_PARAMS,
                    br[1],
                    br[2],
                    br[3],
                    pulse_shape as u8,
                    rx_bw,
                    fdev[1],
                    fdev[2],
                    fdev[3],
                ])
                .await
            }
        }
    }

    /// Lean per-frame call: SetPacketParams with the stored (constant) packet shape and
    /// `payload_len`. No IQ-polarity errata RMW — [`set_packet_params`](Self::set_packet_params)
    /// applies that once in [`configure`](Self::configure).
    async fn set_payload_length(&mut self, payload_len: u8) -> Result<(), Error> {
        match self.packet {
            PacketParams::Lora(p) => {
                let pre = p.preamble_symbols.to_be_bytes();
                let header = u8::from(!p.explicit_header);
                let crc = u8::from(p.crc_on);
                let iq = u8::from(p.invert_iq);
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
            PacketParams::Gfsk(p) => {
                let pre = p.preamble_bits.to_be_bytes();
                self.command(&[
                    op::SET_PACKET_PARAMS,
                    pre[0],
                    pre[1],
                    0x05, // preamble detector: 16 bits — reject noise / foreign in-band energy
                    0x20, // sync word length: 32 bits (4 bytes)
                    0x00, // address filtering off
                    0x01, // variable-length frame (an explicit length byte)
                    payload_len,
                    if p.crc_on { 0x02 } else { 0x00 }, // 2-byte CRC: 1/65536 phantom pass-rate
                    if p.whitening_on { 0x01 } else { 0x00 },
                ])
                .await
            }
        }
    }

    async fn set_packet_params(&mut self, payload_len: u8) -> Result<(), Error> {
        self.set_payload_length(payload_len).await?;
        // IQPolarity errata (DS 15.4) is LoRa-only: set bit 2 unless inverted IQ.
        if let PacketParams::Lora(p) = self.packet {
            let mut v = [0u8; 1];
            self.read_register(reg::IQ_POLARITY, &mut v).await?;
            let fixed = if p.invert_iq {
                v[0] & !0x04
            } else {
                v[0] | 0x04
            };
            self.write_register(reg::IQ_POLARITY, &[fixed]).await?;
        }
        Ok(())
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

/// GFSK RX double-sideband bandwidth code (datasheet table 13-32): the narrowest setting that
/// still passes the signal's Carson bandwidth (2·fdev + bitrate). RX-side only — it has no effect
/// on the transmitted spectrum — but kept honest so a receiver decodes the frame.
fn gfsk_rx_bandwidth(bitrate_bps: u32, fdev_hz: u32) -> u8 {
    let needed = 2 * fdev_hz + bitrate_bps;
    // (Hz, code) ascending.
    const TABLE: [(u32, u8); 21] = [
        (4_800, 0x1F),
        (5_800, 0x17),
        (7_300, 0x0F),
        (9_700, 0x1E),
        (11_700, 0x16),
        (14_600, 0x0E),
        (19_500, 0x1D),
        (23_400, 0x15),
        (29_300, 0x0D),
        (39_000, 0x1C),
        (46_900, 0x14),
        (58_600, 0x0C),
        (78_200, 0x1B),
        (93_800, 0x13),
        (117_300, 0x0B),
        (156_200, 0x1A),
        (187_200, 0x12),
        (234_300, 0x0A),
        (312_000, 0x19),
        (373_600, 0x11),
        (467_000, 0x09),
    ];
    let mut i = 0;
    while i < TABLE.len() {
        if TABLE[i].0 >= needed {
            return TABLE[i].1;
        }
        i += 1;
    }
    0x09
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
        let packet = PacketParams::Lora(LoraPacket {
            preamble_symbols: 18,
            explicit_header: true,
            crc_on: true,
            invert_iq: false,
        });

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
    fn gfsk_command_stream() {
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
        // 50 kbps, 25 kHz deviation, no pulse shaping (plain 2-FSK).
        let modulation = Modulation::Gfsk {
            bitrate_bps: 50_000,
            fdev_hz: 25_000,
            pulse_shape: PulseShape::None,
        };
        let packet = PacketParams::Gfsk(GfskPacket {
            preamble_bits: 32,
            crc_on: true,
            whitening_on: true,
        });

        block_on(radio.init(915_000_000, modulation, packet, 14)).expect("init");
        block_on(radio.transmit(b"PRNS-HELTEC-SMOK")).expect("transmit");

        let cmds = log.borrow();
        let has = |bytes: &[u8]| cmds.iter().any(|c| c.as_slice() == bytes);

        assert!(has(&[0x8A, 0x00]), "SetPacketType GFSK");
        assert!(
            has(&[0x0D, 0x06, 0xC0, 0x93, 0x0B, 0x51, 0xDE]),
            "GFSK 4-byte syncword written to 0x06C0"
        );
        // BR = 1.024e9 / 50000 = 20480 = 0x005000; Fdev = 25000 * 2^25 / 32e6 = 26214 = 0x006666;
        // RX bw for Carson 2*25k+50k=100k -> 117.3 kHz code 0x0B.
        assert!(
            has(&[0x8B, 0x00, 0x50, 0x00, 0x00, 0x0B, 0x00, 0x66, 0x66]),
            "SetModulationParams 50kbps / no-shaping / 117kHz / 25kHz-dev"
        );
        // GFSK SetPacketParams: preamble 32 bits, 8-bit detector, 24-bit sync, variable len 16,
        // CRC on, whitening on.
        assert!(
            has(&[0x8C, 0x00, 0x20, 0x05, 0x20, 0x00, 0x01, 16, 0x02, 0x01]),
            "SetPacketParams GFSK preamble32 / det16 / sync32 / var / len16 / crc2 / whiten"
        );
        // The LoRa-only commands must NOT appear on the GFSK path.
        assert!(!has(&[0x8A, 0x01]), "no LoRa SetPacketType");
        assert!(!has(&[0xA0, 0x00]), "no LoRa symbol-timeout");
    }
}
