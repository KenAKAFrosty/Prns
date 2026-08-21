mod config;
mod protocol;

use core::future::{poll_fn, Future};
use core::task::Poll;

use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::{Operation, SpiDevice};
use prns_core::interfaces::lora::{LoRaNetwork, RadioProfile, RadioProfileCompatibilityError};
use prns_core::interfaces::{PacketPhyStats, RssiDbm, SnrQuarterDb};

pub use config::{
    BoardConfig, HighPowerSelection, PowerAmplifierConfig, PowerAmplifierDutyCycle,
    PowerAmplifierSelection, PowerAmplifierSupply, PowerAmplifierTable, ReceiveGain,
    ReferenceClock, RegulatorMode, RfSwitchConfig, RfSwitchPins, TcxoStartupTime, TcxoVoltage,
    TransmitRampTime,
};

use super::{LoRaRadio, RadioRecovery};
pub use super::{RadioEvent, ReceivedAirFrame};
use protocol::{
    antenna_referred_rssi_dbm, classify_receive_irq, command_with_u32, command_with_u8, irq,
    lora_ldro, op, opcode, radio_config, FirmwareVersion, IrqEventKind, LoraModulation, LoraPacket,
};

#[cfg(test)]
use protocol::{
    Bandwidth, CodingRate, HeaderMode, InvertIq, PayloadCrc, RadioConfig, SpreadingFactor,
};

const NOP: u8 = 0x00;
const MAX_LORA_PAYLOAD: usize = u8::MAX as usize;
const RX_BUFFER_BYTES: usize = u8::MAX as usize + 1;
const BUSY_TIMEOUT_MS: u32 = 100;
const DIO1_RELEASE_TIMEOUT_MS: u32 = 100;
const TX_DONE_TIMEOUT_MS: u32 = 20_000;
const RESET_ASSERT_MS: u32 = 1;
const RESET_BOOT_MS: u32 = 150;
const VERSION_POLL_ATTEMPTS: usize = 200;
const VERSION_POLL_INTERVAL_MS: u32 = 10;
const POST_CALIBRATION_DELAY_MS: u32 = 5;
const LR1110_DEVICE_KIND: u8 = 0x01;
const COMMAND_STATUS_FAILED: u8 = 0x00;
const COMMAND_STATUS_PARAMETER_ERROR: u8 = 0x01;
const CALIBRATE_ALL: u8 = 0x3f;
const STANDBY_RC: u8 = 0x00;
const PACKET_TYPE_LORA: u8 = 0x02;
const LORA_PRIVATE_NETWORK: u8 = 0x00;
const RETICULUM_LR11XX_SYNC_WORD: u8 = 0x12;
const RECEIVE_MAXIMUM_PAYLOAD: u8 = u8::MAX;
const TX_SINGLE_SHOT_TIMEOUT: [u8; 3] = [0x00, 0x00, 0x00];
const RX_CONTINUOUS_TIMEOUT: [u8; 3] = [0xff, 0xff, 0xff];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadioState {
    Uninitialized,
    Ready { packet: LoraPacket },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Spi,
    Busy,
    Dio1,
    Reset,
    DeviceNotReady,
    UnexpectedDevice(u8),
    CommandRejected,
    NotInitialized,
    UnsupportedTransmitPower(i8),
    Crc,
    Timeout,
    BufferTooSmall,
    UnexpectedInterrupt(u32),
}

async fn deadline<F, E, D>(
    future: F,
    delay: &mut D,
    timeout_ms: u32,
    pin_error: Error,
    timeout_error: Error,
) -> Result<(), Error>
where
    F: Future<Output = Result<(), E>>,
    D: DelayNs,
{
    let mut future = core::pin::pin!(future);
    let mut timeout = core::pin::pin!(delay.delay_ms(timeout_ms));
    poll_fn(move |context| {
        if let Poll::Ready(result) = future.as_mut().poll(context) {
            return Poll::Ready(result.map_err(|_| pin_error));
        }
        if timeout.as_mut().poll(context).is_ready() {
            return Poll::Ready(Err(timeout_error));
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
    board: BoardConfig,
    state: RadioState,
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
    pub fn new(
        spi: SPI,
        busy: BUSY,
        dio1: DIO1,
        reset: RST,
        delay: DLY,
        board: BoardConfig,
    ) -> Self {
        Self {
            spi,
            busy,
            dio1,
            reset,
            delay,
            board,
            state: RadioState::Uninitialized,
            tx_staging: [0; MAX_LORA_PAYLOAD],
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

    async fn hard_reset(&mut self) -> Result<(), Error> {
        self.reset.set_low().map_err(|_| Error::Reset)?;
        self.delay.delay_ms(RESET_ASSERT_MS).await;
        self.reset.set_high().map_err(|_| Error::Reset)?;
        self.delay.delay_ms(RESET_BOOT_MS).await;
        self.wait_busy().await
    }

    async fn write_command(&mut self, command: &[u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        self.spi.write(command).await.map_err(|_| Error::Spi)
    }

    async fn read_command(&mut self, command: &[u8], data: &mut [u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        self.spi.write(command).await.map_err(|_| Error::Spi)?;
        self.wait_busy().await?;
        data.fill(NOP);
        self.spi
            .transaction(&mut [Operation::Write(&[NOP]), Operation::TransferInPlace(data)])
            .await
            .map_err(|_| Error::Spi)
    }

    async fn direct_read(&mut self, data: &mut [u8]) -> Result<(), Error> {
        self.wait_busy().await?;
        data.fill(NOP);
        self.spi
            .transaction(&mut [Operation::TransferInPlace(data)])
            .await
            .map_err(|_| Error::Spi)
    }

    async fn irq_status(&mut self) -> Result<u32, Error> {
        let mut status = [0; 6];
        self.direct_read(&mut status).await?;
        let command_status = status[0] >> 1;
        if matches!(
            command_status,
            COMMAND_STATUS_FAILED | COMMAND_STATUS_PARAMETER_ERROR
        ) {
            return Err(Error::CommandRejected);
        }
        Ok(u32::from_be_bytes([
            status[2], status[3], status[4], status[5],
        ]))
    }

    async fn clear_irq(&mut self, mask: u32) -> Result<(), Error> {
        let mask = mask.to_be_bytes();
        self.write_command(&command_with_u32(op::CLEAR_IRQ, mask))
            .await
    }

    async fn write_tx_payload(&mut self, length: usize) -> Result<(), Error> {
        self.wait_busy().await?;
        let header = opcode(op::WRITE_BUFFER8);
        let Self {
            spi, tx_staging, ..
        } = self;
        spi.transaction(&mut [
            Operation::Write(&header),
            Operation::Write(&tx_staging[..length]),
        ])
        .await
        .map_err(|_| Error::Spi)
    }

    async fn read_buffer(&mut self, offset: u8, buffer: &mut [u8]) -> Result<(), Error> {
        let command = [
            opcode(op::READ_BUFFER8)[0],
            opcode(op::READ_BUFFER8)[1],
            offset,
            buffer.len() as u8,
        ];
        self.read_command(&command, buffer).await
    }

    fn packet(&self) -> Result<LoraPacket, Error> {
        match self.state {
            RadioState::Uninitialized => Err(Error::NotInitialized),
            RadioState::Ready { packet } => Ok(packet),
        }
    }

    async fn initialize_profile(&mut self, profile: RadioProfile) -> Result<(), Error> {
        self.state = RadioState::Uninitialized;
        let config = radio_config(profile);
        self.hard_reset().await?;
        let firmware = self.wait_for_lr1110().await?;
        self.set_standby().await?;
        self.write_command(&opcode(op::CLEAR_ERRORS)).await?;
        self.clear_irq(irq::RADIO_EVENTS).await?;
        self.set_regulator().await?;
        self.set_rf_switch().await?;
        self.set_reference_clock().await?;
        self.write_command(&command_with_u8(op::CALIBRATE, CALIBRATE_ALL))
            .await?;
        self.delay.delay_ms(POST_CALIBRATION_DELAY_MS).await;
        self.write_command(&command_with_u8(op::SET_PACKET_TYPE, PACKET_TYPE_LORA))
            .await?;
        self.set_network(config.network, firmware).await?;
        self.set_rf_frequency(config.frequency_hz).await?;
        self.set_modulation_params(config.modulation).await?;
        self.set_transmit_power(config.tx_power_dbm).await?;
        self.set_packet_params(config.packet, RECEIVE_MAXIMUM_PAYLOAD)
            .await?;
        self.set_receive_gain().await?;
        self.route_irqs().await?;
        self.validate_initialization().await?;
        self.state = RadioState::Ready {
            packet: config.packet,
        };
        Ok(())
    }

    async fn wait_for_lr1110(&mut self) -> Result<FirmwareVersion, Error> {
        let mut version = [0; 4];
        for _ in 0..VERSION_POLL_ATTEMPTS {
            self.read_command(&opcode(op::GET_VERSION), &mut version)
                .await?;
            match version[1] {
                LR1110_DEVICE_KIND => {
                    return Ok(FirmwareVersion(u16::from_be_bytes([
                        version[2], version[3],
                    ])));
                }
                0x00 => self.delay.delay_ms(VERSION_POLL_INTERVAL_MS).await,
                device_kind => return Err(Error::UnexpectedDevice(device_kind)),
            }
        }
        Err(Error::DeviceNotReady)
    }

    async fn set_standby(&mut self) -> Result<(), Error> {
        self.write_command(&command_with_u8(op::SET_STANDBY, STANDBY_RC))
            .await
    }

    async fn set_regulator(&mut self) -> Result<(), Error> {
        self.write_command(&command_with_u8(
            op::SET_REG_MODE,
            self.board.regulator as u8,
        ))
        .await
    }

    async fn set_rf_switch(&mut self) -> Result<(), Error> {
        let config = self.board.rf_switch;
        self.write_command(&[
            opcode(op::SET_DIO_AS_RF_SWITCH)[0],
            opcode(op::SET_DIO_AS_RF_SWITCH)[1],
            config.enabled.bits(),
            config.standby.bits(),
            config.receive.bits(),
            config.transmit.bits(),
            config.transmit_high_power.bits(),
            config.transmit_high_frequency.bits(),
            config.gnss.bits(),
            config.wifi.bits(),
        ])
        .await
    }

    async fn set_reference_clock(&mut self) -> Result<(), Error> {
        let ReferenceClock::Tcxo {
            voltage,
            startup_time,
        } = self.board.reference_clock
        else {
            return Ok(());
        };
        let startup_time = startup_time.rtc_ticks().to_be_bytes();
        self.write_command(&[
            opcode(op::SET_TCXO_MODE)[0],
            opcode(op::SET_TCXO_MODE)[1],
            voltage as u8,
            startup_time[1],
            startup_time[2],
            startup_time[3],
        ])
        .await
    }

    async fn set_network(
        &mut self,
        network: LoRaNetwork,
        firmware: FirmwareVersion,
    ) -> Result<(), Error> {
        match (
            network,
            firmware >= FirmwareVersion::MODERN_SYNC_WORD_MINIMUM,
        ) {
            (LoRaNetwork::Reticulum, true) => {
                self.write_command(&command_with_u8(
                    op::SET_LORA_SYNC_WORD,
                    RETICULUM_LR11XX_SYNC_WORD,
                ))
                .await
            }
            (LoRaNetwork::Reticulum, false) => {
                self.write_command(&command_with_u8(
                    op::SET_LORA_PUBLIC_NETWORK,
                    LORA_PRIVATE_NETWORK,
                ))
                .await
            }
        }
    }

    async fn set_rf_frequency(&mut self, frequency_hz: u32) -> Result<(), Error> {
        self.write_command(&command_with_u32(
            op::SET_RF_FREQUENCY,
            frequency_hz.to_be_bytes(),
        ))
        .await
    }

    async fn set_modulation_params(&mut self, modulation: LoraModulation) -> Result<(), Error> {
        self.write_command(&[
            opcode(op::SET_MODULATION_PARAMS)[0],
            opcode(op::SET_MODULATION_PARAMS)[1],
            modulation.spreading_factor as u8,
            modulation.bandwidth as u8,
            modulation.coding_rate as u8,
            lora_ldro(modulation.spreading_factor, modulation.bandwidth),
        ])
        .await
    }

    async fn set_packet_params(
        &mut self,
        packet: LoraPacket,
        payload_length: u8,
    ) -> Result<(), Error> {
        let preamble = packet.preamble_symbols.to_be_bytes();
        self.write_command(&[
            opcode(op::SET_PACKET_PARAMS)[0],
            opcode(op::SET_PACKET_PARAMS)[1],
            preamble[0],
            preamble[1],
            packet.header as u8,
            payload_length,
            packet.crc as u8,
            packet.invert_iq as u8,
        ])
        .await
    }

    async fn set_transmit_power(&mut self, output_power_dbm: i8) -> Result<(), Error> {
        let Some(config) = self.board.power_amplifier.configuration(output_power_dbm) else {
            return Err(Error::UnsupportedTransmitPower(output_power_dbm));
        };
        self.write_command(&[
            opcode(op::SET_PA_CONFIG)[0],
            opcode(op::SET_PA_CONFIG)[1],
            config.selection as u8,
            config.supply as u8,
            config.duty_cycle.value(),
            config.high_power_selection.value(),
        ])
        .await?;
        self.write_command(&[
            opcode(op::SET_TX_PARAMS)[0],
            opcode(op::SET_TX_PARAMS)[1],
            config.chip_output_power_dbm as u8,
            self.board.transmit_ramp_time as u8,
        ])
        .await
    }

    async fn set_receive_gain(&mut self) -> Result<(), Error> {
        self.write_command(&command_with_u8(
            op::SET_RX_BOOSTED,
            self.board.receive_gain as u8,
        ))
        .await
    }

    async fn route_irqs(&mut self) -> Result<(), Error> {
        let routed = irq::RADIO_EVENTS.to_be_bytes();
        self.write_command(&[
            opcode(op::SET_DIO_IRQ_PARAMS)[0],
            opcode(op::SET_DIO_IRQ_PARAMS)[1],
            routed[0],
            routed[1],
            routed[2],
            routed[3],
            0,
            0,
            0,
            0,
        ])
        .await
    }

    async fn validate_initialization(&mut self) -> Result<(), Error> {
        let flags = self.irq_status().await?;
        if flags != 0 {
            self.clear_irq(flags).await?;
        }
        if flags & irq::HARDWARE_ERRORS != 0 {
            return Err(Error::CommandRejected);
        }
        Ok(())
    }

    pub async fn transmit(&mut self, payload: &[u8]) -> Result<(), Error> {
        let packet = self.packet()?;
        if payload.len() > MAX_LORA_PAYLOAD {
            return Err(Error::BufferTooSmall);
        }
        self.tx_staging[..payload.len()].copy_from_slice(payload);
        self.set_standby().await?;
        self.set_packet_params(packet, payload.len() as u8).await?;
        self.write_tx_payload(payload.len()).await?;
        self.clear_irq(irq::RADIO_EVENTS).await?;
        self.write_command(&[
            opcode(op::SET_TX)[0],
            opcode(op::SET_TX)[1],
            TX_SINGLE_SHOT_TIMEOUT[0],
            TX_SINGLE_SHOT_TIMEOUT[1],
            TX_SINGLE_SHOT_TIMEOUT[2],
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
        self.wait_for_dio1_release().await?;
        if flags & irq::HARDWARE_ERRORS != 0 {
            return Err(Error::CommandRejected);
        }
        if flags & irq::TIMEOUT != 0 {
            return Err(Error::Timeout);
        }
        if flags & irq::TX_DONE == 0 {
            return Err(Error::UnexpectedInterrupt(flags));
        }
        Ok(())
    }

    pub async fn arm_rx(&mut self) -> Result<(), Error> {
        let packet = self.packet()?;
        self.set_standby().await?;
        self.set_packet_params(packet, RECEIVE_MAXIMUM_PAYLOAD)
            .await?;
        self.clear_irq(irq::RADIO_EVENTS).await?;
        self.write_command(&[
            opcode(op::SET_RX)[0],
            opcode(op::SET_RX)[1],
            RX_CONTINUOUS_TIMEOUT[0],
            RX_CONTINUOUS_TIMEOUT[1],
            RX_CONTINUOUS_TIMEOUT[2],
        ])
        .await
    }

    pub async fn read_event(&mut self, buffer: &mut [u8]) -> Result<RadioEvent, Error> {
        self.packet()?;
        self.dio1.wait_for_high().await.map_err(|_| Error::Dio1)?;
        let flags = self.irq_status().await?;
        self.clear_irq(flags).await?;
        self.decode_radio_event(flags, buffer).await
    }

    pub async fn poll_event(&mut self, buffer: &mut [u8]) -> Result<Option<RadioEvent>, Error> {
        self.packet()?;
        let flags = self.irq_status().await?;
        if flags == 0 {
            return Ok(None);
        }
        self.clear_irq(flags).await?;
        self.decode_radio_event(flags, buffer).await.map(Some)
    }

    async fn decode_radio_event(
        &mut self,
        flags: u32,
        buffer: &mut [u8],
    ) -> Result<RadioEvent, Error> {
        match classify_receive_irq(flags)? {
            IrqEventKind::Frame => self
                .read_received_frame(buffer)
                .await
                .map(RadioEvent::Frame),
            IrqEventKind::PreambleDetected => Ok(RadioEvent::PreambleDetected),
            IrqEventKind::HeaderValid => Ok(RadioEvent::HeaderValid),
            IrqEventKind::HeaderError => Ok(RadioEvent::HeaderError),
            IrqEventKind::CrcError => Ok(RadioEvent::CrcError),
            IrqEventKind::Timeout => Ok(RadioEvent::Timeout),
            IrqEventKind::SpuriousInterrupt => Ok(RadioEvent::SpuriousInterrupt),
        }
    }

    async fn read_received_frame(&mut self, buffer: &mut [u8]) -> Result<ReceivedAirFrame, Error> {
        let mut buffer_status = [0; 2];
        self.read_command(&opcode(op::GET_RX_BUFFER_STATUS), &mut buffer_status)
            .await?;
        let length = usize::from(buffer_status[0]);
        let offset = buffer_status[1];
        if length > buffer.len() {
            return Err(Error::BufferTooSmall);
        }
        let mut packet_status = [0; 3];
        self.read_command(&opcode(op::GET_PACKET_STATUS), &mut packet_status)
            .await?;
        let phy = PacketPhyStats {
            rssi: Some(RssiDbm::new(antenna_referred_rssi_dbm(
                packet_status[0],
                self.board.external_receive_gain_db,
            ))),
            snr: Some(SnrQuarterDb::new(i16::from(i8::from_be_bytes([
                packet_status[1],
            ])))),
            quality: None,
        };
        if usize::from(offset) + length <= RX_BUFFER_BYTES {
            self.read_buffer(offset, &mut buffer[..length]).await?;
        } else {
            let first_length = RX_BUFFER_BYTES - usize::from(offset);
            self.read_buffer(offset, &mut buffer[..first_length])
                .await?;
            self.read_buffer(0, &mut buffer[first_length..length])
                .await?;
        }
        Ok(ReceivedAirFrame { len: length, phy })
    }

    pub async fn read_frame(&mut self, buffer: &mut [u8]) -> Result<ReceivedAirFrame, Error> {
        loop {
            match self.read_event(buffer).await? {
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

    pub async fn receive(&mut self, buffer: &mut [u8]) -> Result<ReceivedAirFrame, Error> {
        self.arm_rx().await?;
        self.read_frame(buffer).await
    }

    pub async fn channel_rssi_dbm(&mut self) -> Result<i16, Error> {
        self.packet()?;
        let mut rssi = [0];
        self.read_command(&opcode(op::GET_RSSI_INSTANTANEOUS), &mut rssi)
            .await?;
        Ok(antenna_referred_rssi_dbm(
            rssi[0],
            self.board.external_receive_gain_db,
        ))
    }
}

impl<SPI, BUSY, DIO1, RST, DLY> LoRaRadio for Lr1110<SPI, BUSY, DIO1, RST, DLY>
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
        let minimum_dbm = self.board.power_amplifier.minimum_output_power_dbm();
        let maximum_dbm = self.board.power_amplifier.maximum_output_power_dbm();
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
            Error::Spi
            | Error::Busy
            | Error::Dio1
            | Error::Reset
            | Error::DeviceNotReady
            | Error::UnexpectedDevice(_)
            | Error::CommandRejected
            | Error::NotInitialized
            | Error::Timeout
            | Error::UnexpectedInterrupt(_) => RadioRecovery::Reinitialize,
            Error::UnsupportedTransmitPower(_) | Error::Crc | Error::BufferTooSmall => {
                RadioRecovery::Continue
            }
        }
    }

    async fn initialize(&mut self, profile: RadioProfile) -> Result<(), Self::Error> {
        self.initialize_profile(profile).await
    }

    async fn arm_rx(&mut self) -> Result<(), Self::Error> {
        Lr1110::arm_rx(self).await
    }

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), Self::Error> {
        Lr1110::transmit(self, payload).await
    }

    async fn channel_rssi_dbm(&mut self) -> Result<i16, Self::Error> {
        Lr1110::channel_rssi_dbm(self).await
    }

    async fn read_event(&mut self, buffer: &mut [u8]) -> Result<RadioEvent, Self::Error> {
        Lr1110::read_event(self, buffer).await
    }

    async fn poll_event(&mut self, buffer: &mut [u8]) -> Result<Option<RadioEvent>, Self::Error> {
        Lr1110::poll_event(self, buffer).await
    }
}

#[cfg(test)]
mod tests;
