//! On nRF/Cortex-M (`thumbv7em`) this must be built with `lto = "thin"` or `lto = false`; `lto = "fat"` miscompiles the command sequence into a layout-dependent boot HardFault on that target (the Xtensa/esp-hal path is unaffected).

use core::future::{poll_fn, Future};
use core::task::Poll;

use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::{Operation, SpiDevice};

use prns_core::interfaces::lora::{
    CodingRate as ProfileCodingRate, LoRaNetwork, LoraBandwidth as ProfileBandwidth,
    Modulation as ProfileModulation, RadioProfile, RadioProfileCompatibilityError,
    SpreadingFactor as ProfileSpreadingFactor, RNODE_LORA_SYNC_WORD,
};
use prns_core::interfaces::{PacketPhyStats, RssiDbm, SnrQuarterDb};

use super::{LoRaRadio, RadioRecovery};
pub use super::{RadioEvent, ReceivedAirFrame};

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
    pub const GET_PACKET_STATUS: u8 = 0x14;
    pub const GET_RSSI_INST: u8 = 0x15;
    pub const GET_STATUS: u8 = 0xC0;
    pub const CLEAR_DEVICE_ERRORS: u8 = 0x07;
    pub const SET_STOP_RX_TIMER_ON_PREAMBLE: u8 = 0x9F;
    pub const SET_LORA_SYMB_NUM_TIMEOUT: u8 = 0xA0;
}

mod reg {
    pub const LORA_SYNC_WORD_MSB: u16 = 0x0740;
    pub const TX_CLAMP_CONFIG: u16 = 0x08D8;
    pub const RX_GAIN: u16 = 0x08AC;
    pub const TX_MODULATION: u16 = 0x0889;
    pub const IQ_POLARITY: u16 = 0x0736;
}

#[allow(dead_code)]
mod irq {
    pub const TX_DONE: u16 = 1 << 0;
    pub const RX_DONE: u16 = 1 << 1;
    pub const PREAMBLE_DETECTED: u16 = 1 << 2;
    pub const HEADER_VALID: u16 = 1 << 4;
    pub const HEADER_ERR: u16 = 1 << 5;
    pub const CRC_ERR: u16 = 1 << 6;
    pub const TIMEOUT: u16 = 1 << 9;
    pub const ALL: u16 = 0xFFFF;
}

/// The TCXO supply voltage the SX1262 drives out of DIO3 (datasheet table 13-35).
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

#[derive(Debug, PartialEq, Eq)]
pub struct RadioConfig {
    pub frequency_hz: u32,
    pub modulation: Modulation,
    pub packet: LoraPacket,
    pub network: LoRaNetwork,
    pub tx_power_dbm: i8,
}

/// SX1262 LoRa max payload — the on-air length field is a single byte.
const MAX_LORA_PAYLOAD: usize = 255;

const STATUS_NOP: u8 = 0x00;
const MIN_TX_POWER_DBM: i8 = -9;
const MAX_TX_POWER_DBM: i8 = 22;
const SX126X_XTAL_HZ: u64 = 32_000_000;
const SX126X_PLL_SHIFT: u32 = 25;
const TCXO_STARTUP_TIMEOUT_TICKS: u32 = 640;
const TX_RAMP_40_US: u8 = 0x02;

/// Longest the SX1262 should ever hold BUSY: commands process in tens of microseconds and the worst legitimate case is cold-start calibration with TCXO startup (~15 ms). BUSY gates every SPI command, so an unbounded wait lets one wedged-high line hang the radio task and every recovery command with it; past this we surface [`Error::Busy`] so the caller can hard-reset.
const BUSY_TIMEOUT_MS: u32 = 100;
const DIO1_RELEASE_TIMEOUT_MS: u32 = 100;

/// Longest a single LoRa frame can sit on air before TxDone: the worst supported case (SF12 / BW125, a full 255-byte frame at CR4:8 with LDRO) is ~14 s, so this clears it with margin. `SetTx` runs with the chip's own timeout disabled (single-shot), so the TxDone IRQ is otherwise unbounded; a wait past this means the PA or IRQ path faulted, surfaced as [`Error::Timeout`].
const TX_DONE_TIMEOUT_MS: u32 = 20_000;

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

#[derive(Debug, Clone, Copy)]
pub struct ExternalPowerAmplifier {
    /// Lowest supported antenna-referred output power.
    pub minimum_output_power_dbm: i8,
    /// Highest supported antenna-referred output power.
    pub maximum_output_power_dbm: i8,
    /// Convert an antenna-referred output-power request into the power programmed into the SX126x.
    pub chip_power_dbm: fn(i8) -> i8,
}

#[derive(Debug, Clone, Copy)]
pub struct BoardConfig {
    /// `Some(v)` if a TCXO is fed from DIO3 at voltage `v`; `None` for a bare XTAL.
    pub tcxo_voltage: Option<TcxoVoltage>,
    pub use_dcdc: bool,
    pub rx_boost: bool,
    pub dio2_as_rf_switch: bool,
    /// Receive-path gain ahead of the SX126x, removed from RSSI reports so callers see the signal
    /// level at the antenna rather than the amplified level at the transceiver input.
    pub external_rx_gain_db: u8,
    /// External transmit PA behavior. When present, profiles remain antenna-referred while the
    /// driver programs the lower chip power required to produce that output through the PA.
    pub external_power_amplifier: Option<ExternalPowerAmplifier>,
    /// Optional external PA/LNA switch into transmit. Invoked immediately before `SetTx`.
    pub enter_transmit: Option<fn()>,
    /// Optional external PA/LNA switch into receive. Invoked immediately before `SetRx`.
    pub enter_receive: Option<fn()>,
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
enum IrqEventKind {
    PreambleDetected,
    HeaderValid,
    Frame,
    HeaderError,
    CrcError,
    Timeout,
    Other,
}

fn classify_rx_irq(flags: u16) -> IrqEventKind {
    if flags & irq::RX_DONE != 0 {
        return if flags & irq::CRC_ERR != 0 {
            IrqEventKind::CrcError
        } else {
            IrqEventKind::Frame
        };
    }
    if flags & irq::HEADER_ERR != 0 {
        return IrqEventKind::HeaderError;
    }
    if flags & irq::HEADER_VALID != 0 {
        return IrqEventKind::HeaderValid;
    }
    if flags & irq::PREAMBLE_DETECTED != 0 {
        return IrqEventKind::PreambleDetected;
    }
    if flags & irq::CRC_ERR != 0 {
        return IrqEventKind::CrcError;
    }
    if flags & irq::TIMEOUT != 0 {
        return IrqEventKind::Timeout;
    }
    IrqEventKind::Other
}

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
    /// RAM staging for the TX FIFO write — DMA-class SPI can't source a flash-resident payload. A field, not a per-call stack local, so it never bloats the `transmit` future or the shared node stack.
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
        self.delay.delay_ms(10).await;
        self.reset.set_low().map_err(|_| Error::Reset)?;
        self.delay.delay_ms(20).await;
        self.reset.set_high().map_err(|_| Error::Reset)?;
        self.delay.delay_ms(10).await;
        self.wait_busy().await
    }

    async fn command(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        self.spi.write(bytes).await.map_err(|_| Error::Spi)
    }

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
        let header = [op::READ_REGISTER, (addr >> 8) as u8, addr as u8, STATUS_NOP];
        self.spi
            .transaction(&mut [Operation::Write(&header), Operation::Read(buf)])
            .await
            .map_err(|_| Error::Spi)
    }

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
        let header = [op::READ_BUFFER, offset, STATUS_NOP];
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

    async fn wait_for_dio1_release(&mut self) -> Result<(), Error> {
        let Self { dio1, delay, .. } = self;
        deadline(
            dio1.wait_for_low(),
            delay,
            DIO1_RELEASE_TIMEOUT_MS,
            Error::Dio1,
            Error::Timeout,
        )
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
    /// Reset the chip, run the LoRa cold-start sequence, then apply the channel config. Leaves the chip in standby, fully configured.
    pub async fn init(&mut self, config: RadioConfig) -> Result<(), Error> {
        let RadioConfig {
            frequency_hz,
            modulation,
            packet,
            network,
            tx_power_dbm,
        } = config;
        self.freq_hz = frequency_hz;
        self.modulation = modulation;
        self.packet = packet;
        self.tx_power_dbm = self
            .config
            .external_power_amplifier
            .map_or(tx_power_dbm, |amplifier| {
                (amplifier.chip_power_dbm)(tx_power_dbm)
            })
            .clamp(MIN_TX_POWER_DBM, MAX_TX_POWER_DBM);

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
            let startup_timeout = TCXO_STARTUP_TIMEOUT_TICKS.to_be_bytes();
            self.command(&[
                op::SET_DIO3_AS_TCXO_CTRL,
                voltage as u8,
                startup_timeout[1],
                startup_timeout[2],
                startup_timeout[3],
            ])
            .await?;
            self.command(&[op::CALIBRATE, 0x7F]).await?;
        }
        self.command(&[op::SET_PACKET_TYPE, 0x01]).await?;
        self.write_register(
            reg::LORA_SYNC_WORD_MSB,
            &sync_word_for_network(network).to_be_bytes(),
        )
        .await?;
        self.command(&[op::SET_BUFFER_BASE_ADDRESS, 0x00, 0x00])
            .await?;
        let [image_cal_a, image_cal_b] = image_calibration_pair(frequency_hz);
        self.command(&[op::CALIBRATE_IMAGE, image_cal_a, image_cal_b])
            .await?;
        self.configure().await?;
        self.route_irqs_and_tune_rx().await?;
        invoke_frontend(self.config.enter_receive);
        Ok(())
    }

    /// Apply the channel config (modulation, frequency, TX power, packet shape). The SX1262 RETAINS these registers across SetStandby / SetTx / SetRx (only Sleep or reset clears them), so this runs ONCE from [`init`](Self::init) and again only on a discrete channel change, never per packet; the per-packet path only restamps the payload length.
    async fn configure(&mut self) -> Result<(), Error> {
        self.set_modulation_params().await?; // + TxModulation errata
        self.set_tx_power().await?; // TxClampCfg errata + PA config + tx params
        self.set_packet_params(0xFF).await?; // + IQPolarity errata; 0xFF = RX-friendly max
        self.set_rf_frequency().await
    }

    /// Route IRQs and arm the RX front-end, once, after [`configure`](Self::configure). The SX1262 RETAINS all of these across SetStandby / SetTx / SetRx (proven on hardware: the boosted RX gain read back 0x96 cycle after cycle), so they belong in init, not the per-frame path. All IRQs are unmasked onto DIO1 and discriminated in software. Independent of the channel, so a channel change re-runs `configure` but not this.
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

    /// Transmit one LoRa frame and wait for TxDone. The channel must already be configured; only the payload length is restamped per frame, so this interleaves with [`receive`](Self::receive).
    pub async fn transmit(&mut self, payload: &[u8]) -> Result<(), Error> {
        let len = payload.len();
        if len > MAX_LORA_PAYLOAD {
            return Err(Error::BufferTooSmall);
        }
        // EasyDMA (and most SPI DMA) can only source from RAM; the caller's payload may be flash-resident (`&'static`), so stage it through the RAM `tx_staging` field.
        self.tx_staging[..len].copy_from_slice(payload);

        invoke_frontend(self.config.enter_transmit);
        self.standby().await?;
        self.set_packet_params(len as u8).await?;
        self.write_tx_payload(len).await?;
        self.clear_irq(irq::ALL).await?;
        // SetTx with timeout 0 = single shot, no chip timeout — so the TxDone wait is bounded here ([`TX_DONE_TIMEOUT_MS`]); a TX that never completes must not trap the radio task forever.
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
        self.wait_for_dio1_release().await?;
        if flags & irq::TIMEOUT != 0 {
            return Err(Error::Timeout);
        }
        Ok(())
    }

    /// Arm continuous RX: restamp the RX-side max payload length, clear stale IRQs, enter SetRx continuous. [`read_frame`](Self::read_frame) waits WITHOUT re-arming, so a host-side select that cancels the read mid-listen leaves the radio receiving (the RxDone IRQ latches) rather than guillotining an in-flight multi-hundred-ms LoRa frame.
    pub async fn arm_rx(&mut self) -> Result<(), Error> {
        invoke_frontend(self.config.enter_receive);
        self.standby().await?;
        self.set_packet_params(0xFF).await?;
        self.clear_irq(irq::ALL).await?;
        // SetRx 0xFFFFFF = continuous.
        self.command(&[op::SET_RX, 0xFF, 0xFF, 0xFF]).await
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
        flags: u16,
        buf: &mut [u8],
    ) -> Result<RadioEvent, Error> {
        match classify_rx_irq(flags) {
            IrqEventKind::Frame => {
                let mut status = [0u8; 3];
                self.read_command(op::GET_RX_BUFFER_STATUS, &mut status)
                    .await?;
                let len = status[1] as usize;
                let offset = status[2];
                if len > buf.len() {
                    return Err(Error::BufferTooSmall);
                }
                let mut packet_status = [0u8; 4];
                self.read_command(op::GET_PACKET_STATUS, &mut packet_status)
                    .await?;
                let phy = PacketPhyStats {
                    rssi: Some(RssiDbm::new(antenna_referred_rssi_dbm(
                        packet_status[1],
                        self.config.external_rx_gain_db,
                    ))),
                    snr: Some(SnrQuarterDb::new(i16::from(i8::from_be_bytes([
                        packet_status[2],
                    ])))),
                    quality: None,
                };
                self.read_buffer(offset, &mut buf[..len]).await?;
                Ok(RadioEvent::Frame(ReceivedAirFrame { len, phy }))
            }
            IrqEventKind::PreambleDetected => Ok(RadioEvent::PreambleDetected),
            IrqEventKind::HeaderValid => Ok(RadioEvent::HeaderValid),
            IrqEventKind::HeaderError => Ok(RadioEvent::HeaderError),
            IrqEventKind::CrcError => Ok(RadioEvent::CrcError),
            IrqEventKind::Timeout => Ok(RadioEvent::Timeout),
            IrqEventKind::Other => Ok(RadioEvent::SpuriousInterrupt),
        }
    }

    /// Wait for one complete frame while preserving the legacy frame-only API.
    /// Channel-access code should use [`read_event`](Self::read_event) so it
    /// sees preamble and header evidence.
    pub async fn read_frame(&mut self, buf: &mut [u8]) -> Result<ReceivedAirFrame, Error> {
        loop {
            match self.read_event(buf).await? {
                RadioEvent::Frame(frame) => return Ok(frame),
                RadioEvent::CrcError => return Err(Error::Crc),
                RadioEvent::Timeout => return Err(Error::Timeout),
                RadioEvent::PreambleDetected
                | RadioEvent::HeaderValid
                | RadioEvent::HeaderError
                | RadioEvent::SpuriousInterrupt => {}
            }
        }
    }

    pub async fn receive(&mut self, buf: &mut [u8]) -> Result<ReceivedAirFrame, Error> {
        self.arm_rx().await?;
        self.read_frame(buf).await
    }

    /// The instantaneous channel RSSI in dBm, valid while armed in RX: the carrier-sense a listen-before-talk transmitter checks before holding off.
    pub async fn channel_rssi_dbm(&mut self) -> Result<i16, Error> {
        let mut buf = [0u8; 2];
        self.read_command(op::GET_RSSI_INST, &mut buf).await?;
        Ok(antenna_referred_rssi_dbm(
            buf[1],
            self.config.external_rx_gain_db,
        ))
    }

    async fn standby(&mut self) -> Result<(), Error> {
        self.command(&[op::SET_STANDBY, 0x00]).await
    }

    async fn set_rf_frequency(&mut self) -> Result<(), Error> {
        let steps = (((self.freq_hz as u64) << SX126X_PLL_SHIFT) / SX126X_XTAL_HZ) as u32;
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

    async fn set_packet_params(&mut self, payload_len: u8) -> Result<(), Error> {
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
        .await?;
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

        let pa = pa_config(self.tx_power_dbm);
        self.command(&[op::SET_PA_CONFIG, pa.duty_cycle, pa.hp_max, 0x00, 0x01])
            .await?;
        self.command(&[op::SET_TX_PARAMS, pa.tx_power, TX_RAMP_40_US])
            .await
    }
}

impl<SPI, BUSY, DIO1, RST, DLY> LoRaRadio for Sx126x<SPI, BUSY, DIO1, RST, DLY>
where
    SPI: SpiDevice,
    BUSY: Wait,
    DIO1: Wait,
    RST: OutputPin,
    DLY: DelayNs,
{
    type Error = Error;

    fn validate_profile(
        &self,
        profile: RadioProfile,
    ) -> Result<(), RadioProfileCompatibilityError> {
        let power_dbm = profile.tx_power.dbm();
        let (minimum_dbm, maximum_dbm) = self.config.external_power_amplifier.map_or(
            (MIN_TX_POWER_DBM, MAX_TX_POWER_DBM),
            |amplifier| {
                (
                    amplifier.minimum_output_power_dbm,
                    amplifier.maximum_output_power_dbm,
                )
            },
        );
        if !(minimum_dbm..=maximum_dbm).contains(&power_dbm) {
            return Err(
                RadioProfileCompatibilityError::TransmitPowerOutsideRadioRange {
                    power_dbm,
                    minimum_dbm,
                    maximum_dbm,
                },
            );
        }
        Ok(())
    }

    fn recovery(error: &Self::Error) -> RadioRecovery {
        match error {
            Error::Spi | Error::Busy | Error::Dio1 | Error::Reset | Error::Timeout => {
                RadioRecovery::Reinitialize
            }
            Error::Crc | Error::BufferTooSmall => RadioRecovery::Continue,
        }
    }

    async fn initialize(&mut self, profile: RadioProfile) -> Result<(), Self::Error> {
        Sx126x::init(self, radio_config(profile)).await
    }

    async fn arm_rx(&mut self) -> Result<(), Self::Error> {
        Sx126x::arm_rx(self).await
    }

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), Self::Error> {
        Sx126x::transmit(self, payload).await
    }

    async fn channel_rssi_dbm(&mut self) -> Result<i16, Self::Error> {
        Sx126x::channel_rssi_dbm(self).await
    }

    async fn read_event(&mut self, buffer: &mut [u8]) -> Result<RadioEvent, Self::Error> {
        Sx126x::read_event(self, buffer).await
    }

    async fn poll_event(&mut self, buffer: &mut [u8]) -> Result<Option<RadioEvent>, Self::Error> {
        Sx126x::poll_event(self, buffer).await
    }
}

fn lora_ldro(sf: SpreadingFactor, bw: Bandwidth) -> u8 {
    u8::from(matches!(
        (sf, bw),
        (
            SpreadingFactor::Sf11 | SpreadingFactor::Sf12,
            Bandwidth::Bw125
        ) | (SpreadingFactor::Sf12, Bandwidth::Bw250)
    ))
}

fn sync_word_for_network(network: LoRaNetwork) -> u16 {
    match network {
        LoRaNetwork::Reticulum => RNODE_LORA_SYNC_WORD,
    }
}

fn radio_config(profile: RadioProfile) -> RadioConfig {
    let ProfileModulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    let spreading_factor = match spreading_factor {
        ProfileSpreadingFactor::Sf5 => SpreadingFactor::Sf5,
        ProfileSpreadingFactor::Sf6 => SpreadingFactor::Sf6,
        ProfileSpreadingFactor::Sf7 => SpreadingFactor::Sf7,
        ProfileSpreadingFactor::Sf8 => SpreadingFactor::Sf8,
        ProfileSpreadingFactor::Sf9 => SpreadingFactor::Sf9,
        ProfileSpreadingFactor::Sf10 => SpreadingFactor::Sf10,
        ProfileSpreadingFactor::Sf11 => SpreadingFactor::Sf11,
        ProfileSpreadingFactor::Sf12 => SpreadingFactor::Sf12,
    };
    let bandwidth = match bandwidth {
        ProfileBandwidth::Bw125kHz => Bandwidth::Bw125,
        ProfileBandwidth::Bw250kHz => Bandwidth::Bw250,
        ProfileBandwidth::Bw500kHz => Bandwidth::Bw500,
    };
    let coding_rate = match coding_rate {
        ProfileCodingRate::Cr45 => CodingRate::Cr4_5,
        ProfileCodingRate::Cr46 => CodingRate::Cr4_6,
        ProfileCodingRate::Cr47 => CodingRate::Cr4_7,
        ProfileCodingRate::Cr48 => CodingRate::Cr4_8,
    };
    RadioConfig {
        frequency_hz: profile.frequency.hz(),
        modulation: Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        packet: LoraPacket {
            preamble_symbols: profile.preamble.count(),
            explicit_header: true,
            crc_on: true,
            invert_iq: false,
        },
        network: LoRaNetwork::Reticulum,
        tx_power_dbm: profile.tx_power.dbm(),
    }
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

fn decode_rssi_dbm(encoded: u8) -> i16 {
    -i16::from(encoded) / 2
}

fn invoke_frontend(hook: Option<fn()>) {
    if let Some(enter) = hook {
        enter();
    }
}

fn antenna_referred_rssi_dbm(encoded: u8, external_rx_gain_db: u8) -> i16 {
    decode_rssi_dbm(encoded).saturating_sub(i16::from(external_rx_gain_db))
}

struct PaConfig {
    duty_cycle: u8,
    hp_max: u8,
    tx_power: u8,
}

fn pa_config(power_dbm: i8) -> PaConfig {
    let txp = power_dbm.clamp(-9, 22);
    let (duty_cycle, hp_max, tx_power) = match txp {
        21..=22 => (0x04, 0x07, 22),
        18..=20 => (0x03, 0x05, (txp + 2) as u8),
        15..=17 => (0x02, 0x03, (txp + 5) as u8),
        _ => (0x02, 0x02, (txp + 8) as u8),
    };
    PaConfig {
        duty_cycle,
        hp_max,
        tx_power,
    }
}

#[cfg(test)]
mod tests {
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
    use prns_core::interfaces::lora::{TxPower, DEFAULT_915_PROFILE};

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

    fn fill_read(header: &[u8], buf: &mut [u8]) {
        match header.first().copied().unwrap_or(0) {
            op::GET_IRQ_STATUS => {
                if buf.len() >= 3 {
                    buf[0] = 0x00;
                    buf[1] = 0x00;
                    buf[2] = 0x03;
                }
            }
            op::GET_RX_BUFFER_STATUS => {
                if buf.len() >= 3 {
                    buf[0] = 0x00;
                    buf[1] = 16;
                    buf[2] = 0x00;
                }
            }
            op::GET_PACKET_STATUS => {
                if buf.len() >= 4 {
                    buf[0] = 0x00;
                    buf[1] = 181;
                    buf[2] = 0xF7;
                    buf[3] = 184;
                }
            }
            op::GET_RSSI_INST => {
                if buf.len() >= 2 {
                    buf[0] = 0x00;
                    buf[1] = 172;
                }
            }
            op::READ_BUFFER => {
                let p = b"PRNS-HELTEC-SMOK";
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = p.get(i).copied().unwrap_or(0);
                }
            }
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

    type MockRadio = Sx126x<MockSpi, MockWait, MockWait, MockOut, MockDelay>;

    fn mock_radio() -> MockRadio {
        Sx126x::new(
            MockSpi {
                log: Rc::new(RefCell::new(Vec::new())),
            },
            MockWait,
            MockWait,
            MockOut,
            MockDelay,
            board(),
        )
    }

    struct BusyNeverLow;
    impl DigErrorType for BusyNeverLow {
        type Error = MockErr;
    }
    impl Wait for BusyNeverLow {
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

    struct Dio1NeverHigh;
    impl DigErrorType for Dio1NeverHigh {
        type Error = MockErr;
    }
    impl Wait for Dio1NeverHigh {
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

    struct Dio1NeverReleases;
    impl DigErrorType for Dio1NeverReleases {
        type Error = MockErr;
    }
    impl Wait for Dio1NeverReleases {
        async fn wait_for_high(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        async fn wait_for_low(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
        async fn wait_for_rising_edge(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
        async fn wait_for_falling_edge(&mut self) -> Result<(), MockErr> {
            core::future::pending::<Result<(), MockErr>>().await
        }
        async fn wait_for_any_edge(&mut self) -> Result<(), MockErr> {
            Ok(())
        }
    }

    fn board() -> BoardConfig {
        BoardConfig {
            tcxo_voltage: Some(TcxoVoltage::V1_8),
            use_dcdc: true,
            rx_boost: true,
            dio2_as_rf_switch: true,
            external_rx_gain_db: 0,
            external_power_amplifier: None,
            enter_transmit: None,
            enter_receive: None,
        }
    }

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
    fn reticulum_profile_maps_to_the_existing_sx126x_configuration() {
        assert_eq!(
            radio_config(DEFAULT_915_PROFILE),
            RadioConfig {
                frequency_hz: 915_000_000,
                modulation: Modulation::Lora {
                    spreading_factor: SpreadingFactor::Sf9,
                    bandwidth: Bandwidth::Bw250,
                    coding_rate: CodingRate::Cr4_5,
                },
                packet: LoraPacket {
                    preamble_symbols: 18,
                    explicit_header: true,
                    crc_on: true,
                    invert_iq: false,
                },
                network: LoRaNetwork::Reticulum,
                tx_power_dbm: 22,
            }
        );
        assert_eq!(
            sync_word_for_network(LoRaNetwork::Reticulum),
            RNODE_LORA_SYNC_WORD
        );
    }

    #[test]
    fn sx126x_owns_its_transmit_power_compatibility() {
        let radio = mock_radio();
        let mut profile = DEFAULT_915_PROFILE;
        profile.tx_power = TxPower::new(MIN_TX_POWER_DBM - 1);
        assert_eq!(profile.validate(), Ok(()));
        assert_eq!(
            radio.validate_profile(profile),
            Err(
                RadioProfileCompatibilityError::TransmitPowerOutsideRadioRange {
                    power_dbm: MIN_TX_POWER_DBM - 1,
                    minimum_dbm: MIN_TX_POWER_DBM,
                    maximum_dbm: MAX_TX_POWER_DBM,
                }
            )
        );
    }

    #[test]
    fn external_pa_limits_and_chip_power_are_antenna_referred() {
        fn chip_power_dbm(requested_output_dbm: i8) -> i8 {
            requested_output_dbm - 14
        }

        let mut board = board();
        board.external_power_amplifier = Some(ExternalPowerAmplifier {
            minimum_output_power_dbm: 5,
            maximum_output_power_dbm: 22,
            chip_power_dbm,
        });
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut radio = Sx126x::new(
            MockSpi { log: log.clone() },
            MockWait,
            MockWait,
            MockOut,
            MockDelay,
            board,
        );

        let mut profile = DEFAULT_915_PROFILE;
        profile.tx_power = TxPower::new(4);
        assert_eq!(
            radio.validate_profile(profile),
            Err(
                RadioProfileCompatibilityError::TransmitPowerOutsideRadioRange {
                    power_dbm: 4,
                    minimum_dbm: 5,
                    maximum_dbm: 22,
                }
            )
        );

        block_on(radio.init(radio_config(DEFAULT_915_PROFILE))).expect("init");
        assert_eq!(radio.tx_power_dbm, 8);
        assert!(
            log.borrow()
                .iter()
                .any(|command| command.as_slice() == [op::SET_TX_PARAMS, 0x10, TX_RAMP_40_US]),
            "22 dBm antenna-referred maps to 8 dBm at the chip"
        );
    }

    #[test]
    fn sx126x_recovery_classifies_every_error() {
        for error in [
            Error::Spi,
            Error::Busy,
            Error::Dio1,
            Error::Reset,
            Error::Timeout,
        ] {
            assert_eq!(MockRadio::recovery(&error), RadioRecovery::Reinitialize);
        }
        for error in [Error::Crc, Error::BufferTooSmall] {
            assert_eq!(MockRadio::recovery(&error), RadioRecovery::Continue);
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
            external_rx_gain_db: 0,
            external_power_amplifier: None,
            enter_transmit: None,
            enter_receive: None,
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

        let config = RadioConfig {
            frequency_hz: 915_000_000,
            modulation,
            packet,
            network: LoRaNetwork::Reticulum,
            tx_power_dbm: 14,
        };
        block_on(radio.init(config)).expect("init");
        block_on(radio.transmit(b"PRNS-HELTEC-SMOK")).expect("transmit");
        let mut buf = [0u8; 255];
        let received = block_on(radio.receive(&mut buf)).expect("receive");
        let n = received.len;
        assert_eq!(n, 16, "received frame length");
        assert_eq!(&buf[..n], b"PRNS-HELTEC-SMOK");
        assert_eq!(
            received.phy,
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-90)),
                snr: Some(SnrQuarterDb::new(-9)),
                quality: None,
            }
        );
        let received2 = block_on(radio.receive(&mut buf)).expect("receive 2");
        assert_eq!(received2.len, 16, "second received frame length");

        let cmds = log.borrow();
        let has = |bytes: &[u8]| cmds.iter().any(|c| c.as_slice() == bytes);
        let count = |bytes: &[u8]| cmds.iter().filter(|c| c.as_slice() == bytes).count();

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
        assert!(
            has(&[0x8B, 0x08, 0x04, 0x01, 0x00]),
            "SetModulationParams SF8/BW125/CR45/LDRO0"
        );
        assert!(has(&[0x95, 0x02, 0x02, 0x00, 0x01]), "SetPaConfig 14 dBm");
        assert!(
            has(&[0x8E, 0x16, 0x02]),
            "SetTxParams power 22 dBm/ramp 40 µs"
        );
        assert!(
            has(&[0x86, 0x39, 0x30, 0x00, 0x00]),
            "SetRfFrequency 915 MHz"
        );
        assert!(
            has(&[0x8C, 0x00, 0x12, 0x00, 16, 0x01, 0x00]),
            "SetPacketParams TX preamble18/explicit/len16/crc"
        );
        assert!(
            has(&[0x8C, 0x00, 0x12, 0x00, 0xFF, 0x01, 0x00]),
            "SetPacketParams RX max len"
        );
        assert!(has(&[0x0D, 0x08, 0x89, 0x04]), "TxModulation errata bit2");
        assert_eq!(
            count(&[0x0D, 0x07, 0x36, 0x04]),
            4,
            "IQPolarity errata follows initial, TX, and explicit RX packet parameters"
        );
        assert!(has(&[0x0D, 0x08, 0xD8, 0x1E]), "TxClampCfg errata bits1-4");

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
        assert_eq!(count(&[0x14]), 2, "GetPacketStatus once per receive");
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
    fn external_receive_gain_is_removed_from_reported_rssi() {
        assert_eq!(antenna_referred_rssi_dbm(172, 0), -86);
        assert_eq!(antenna_referred_rssi_dbm(172, 17), -103);
        assert_eq!(antenna_referred_rssi_dbm(172, 23), -109);
    }

    #[test]
    fn configured_receive_gain_normalizes_channel_and_packet_rssi() {
        let mut board = board();
        board.external_rx_gain_db = 17;
        let mut radio = Sx126x::new(
            MockSpi {
                log: Rc::new(RefCell::new(Vec::new())),
            },
            MockWait,
            MockWait,
            MockOut,
            MockDelay,
            board,
        );

        assert_eq!(block_on(radio.channel_rssi_dbm()), Ok(-103));

        let mut buf = [0u8; 255];
        let received = block_on(radio.receive(&mut buf)).expect("receive");
        assert_eq!(received.phy.rssi, Some(RssiDbm::new(-107)));
    }

    #[test]
    fn receive_irq_classification_preserves_channel_evidence() {
        assert_eq!(
            classify_rx_irq(irq::PREAMBLE_DETECTED),
            IrqEventKind::PreambleDetected
        );
        assert_eq!(
            classify_rx_irq(irq::PREAMBLE_DETECTED | irq::HEADER_VALID),
            IrqEventKind::HeaderValid
        );
        assert_eq!(
            classify_rx_irq(irq::RX_DONE | irq::PREAMBLE_DETECTED | irq::HEADER_VALID),
            IrqEventKind::Frame
        );
        assert_eq!(
            classify_rx_irq(irq::RX_DONE | irq::CRC_ERR),
            IrqEventKind::CrcError
        );
        assert_eq!(classify_rx_irq(irq::HEADER_ERR), IrqEventKind::HeaderError);
        assert_eq!(classify_rx_irq(irq::TIMEOUT), IrqEventKind::Timeout);
        assert_eq!(classify_rx_irq(irq::TX_DONE), IrqEventKind::Other);
    }

    #[test]
    fn a_wedged_busy_line_times_out_instead_of_hanging() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut radio = Sx126x::new(
            MockSpi { log },
            BusyNeverLow,
            MockWait,
            MockOut,
            MockDelay,
            board(),
        );
        let result = block_on(radio.arm_rx());
        assert_eq!(result, Err(Error::Busy));
    }

    #[test]
    fn a_txdone_that_never_fires_times_out_instead_of_hanging() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut radio = Sx126x::new(
            MockSpi { log },
            MockWait,
            Dio1NeverHigh,
            MockOut,
            MockDelay,
            board(),
        );
        let result = block_on(radio.transmit(b"PRNS-HELTEC-SMOK"));
        assert_eq!(result, Err(Error::Timeout));
    }

    #[test]
    fn a_txdone_that_never_releases_times_out_instead_of_reentering_receive() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut radio = Sx126x::new(
            MockSpi { log },
            MockWait,
            Dio1NeverReleases,
            MockOut,
            MockDelay,
            board(),
        );
        let result = block_on(radio.transmit(b"PRNS-HELTEC-SMOK"));
        assert_eq!(result, Err(Error::Timeout));
    }
}
