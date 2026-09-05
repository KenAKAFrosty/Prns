use super::*;
use core::future::Future;
use core::task::{Context, Poll, Waker};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use embedded_hal::digital::{
    Error as DigitalError, ErrorKind as DigitalErrorKind, ErrorType as DigitalErrorType, OutputPin,
};
use embedded_hal::spi::{
    Error as SpiError, ErrorKind as SpiErrorKind, ErrorType as SpiErrorType, Operation,
};
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::SpiDevice;
use prns_core::interfaces::lora::{TxPower, US915_AUTO_LORA_PROFILE};

const TEST_PA_CONFIGS: [PowerAmplifierConfig; 3] = [
    PowerAmplifierConfig {
        chip_output_power_dbm: 22,
        selection: PowerAmplifierSelection::HighPower,
        supply: PowerAmplifierSupply::Battery,
        duty_cycle: PowerAmplifierDutyCycle::new(4),
        high_power_selection: HighPowerSelection::new(7),
    },
    PowerAmplifierConfig {
        chip_output_power_dbm: 22,
        selection: PowerAmplifierSelection::HighPower,
        supply: PowerAmplifierSupply::Battery,
        duty_cycle: PowerAmplifierDutyCycle::new(5),
        high_power_selection: HighPowerSelection::new(7),
    },
    PowerAmplifierConfig {
        chip_output_power_dbm: 22,
        selection: PowerAmplifierSelection::HighPower,
        supply: PowerAmplifierSupply::Battery,
        duty_cycle: PowerAmplifierDutyCycle::new(6),
        high_power_selection: HighPowerSelection::new(7),
    },
];

#[derive(Debug)]
struct MockError;

impl SpiError for MockError {
    fn kind(&self) -> SpiErrorKind {
        SpiErrorKind::Other
    }
}

impl DigitalError for MockError {
    fn kind(&self) -> DigitalErrorKind {
        DigitalErrorKind::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceEvent {
    BusyLow,
    Dio1High,
    Dio1Low,
    Spi,
}

struct MockState {
    commands: Vec<Vec<u8>>,
    trace: Vec<TraceEvent>,
    pending_read: Option<Vec<u8>>,
    irq_statuses: VecDeque<u32>,
    firmware: FirmwareVersion,
    device_kind: u8,
    command_status: u8,
    rx_length: u8,
    rx_offset: u8,
    rx_memory: [u8; RX_BUFFER_BYTES],
    all_read_clocks_were_zero: bool,
}

impl MockState {
    fn new() -> Self {
        let mut rx_memory = [0; RX_BUFFER_BYTES];
        let payload = b"PRNS-LR1110-SMOK";
        let offset = 250usize;
        for (index, byte) in payload.iter().copied().enumerate() {
            rx_memory[(offset + index) % RX_BUFFER_BYTES] = byte;
        }
        Self {
            commands: Vec::new(),
            trace: Vec::new(),
            pending_read: None,
            irq_statuses: VecDeque::new(),
            firmware: FirmwareVersion(0x0308),
            device_kind: LR1110_DEVICE_KIND,
            command_status: 0x02,
            rx_length: payload.len() as u8,
            rx_offset: offset as u8,
            rx_memory,
            all_read_clocks_were_zero: true,
        }
    }

    fn fill_read(&mut self, command: &[u8], buffer: &mut [u8]) {
        let operation = u16::from_be_bytes([command[0], command[1]]);
        match operation {
            op::GET_VERSION => {
                buffer.copy_from_slice(&[
                    0x01,
                    self.device_kind,
                    self.firmware.0.to_be_bytes()[0],
                    self.firmware.0.to_be_bytes()[1],
                ]);
            }
            op::GET_RX_BUFFER_STATUS => {
                buffer.copy_from_slice(&[self.rx_length, self.rx_offset]);
            }
            op::GET_PACKET_STATUS => buffer.copy_from_slice(&[181, 0xf7, 184]),
            op::GET_RSSI_INSTANTANEOUS => buffer.copy_from_slice(&[172]),
            op::READ_BUFFER8 => {
                let offset = usize::from(command[2]);
                for (index, byte) in buffer.iter_mut().enumerate() {
                    *byte = self.rx_memory[offset + index];
                }
            }
            _ => buffer.fill(0),
        }
    }

    fn fill_status(&mut self, buffer: &mut [u8]) {
        let flags = self.irq_statuses.pop_front().unwrap_or(0).to_be_bytes();
        buffer.copy_from_slice(&[
            self.command_status << 1,
            0x00,
            flags[0],
            flags[1],
            flags[2],
            flags[3],
        ]);
    }
}

type SharedState = Rc<RefCell<MockState>>;

struct MockSpi {
    state: SharedState,
}

impl SpiErrorType for MockSpi {
    type Error = MockError;
}

impl SpiDevice<u8> for MockSpi {
    async fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), MockError> {
        let mut writes = Vec::new();
        for operation in operations.iter() {
            if let Operation::Write(bytes) = operation {
                writes.extend_from_slice(bytes);
            }
        }

        let mut state = self.state.borrow_mut();
        state.trace.push(TraceEvent::Spi);
        if !writes.is_empty() {
            state.commands.push(writes.clone());
        }

        for operation in operations.iter_mut() {
            if let Operation::TransferInPlace(buffer) = operation {
                state.all_read_clocks_were_zero &= buffer.iter().all(|byte| *byte == NOP);
                let pending = state.pending_read.take();
                if let Some(command) = pending {
                    state.fill_read(&command, buffer);
                } else {
                    state.fill_status(buffer);
                }
            }
        }

        if writes.len() >= 2 {
            let operation = u16::from_be_bytes([writes[0], writes[1]]);
            if matches!(
                operation,
                op::GET_VERSION
                    | op::GET_RX_BUFFER_STATUS
                    | op::GET_PACKET_STATUS
                    | op::GET_RSSI_INSTANTANEOUS
                    | op::READ_BUFFER8
            ) {
                state.pending_read = Some(writes);
            }
        }
        Ok(())
    }
}

struct MockBusy {
    state: SharedState,
}

impl DigitalErrorType for MockBusy {
    type Error = MockError;
}

impl Wait for MockBusy {
    async fn wait_for_high(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_low(&mut self) -> Result<(), MockError> {
        self.state.borrow_mut().trace.push(TraceEvent::BusyLow);
        Ok(())
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), MockError> {
        Ok(())
    }
}

struct MockDio1 {
    state: SharedState,
}

impl DigitalErrorType for MockDio1 {
    type Error = MockError;
}

impl Wait for MockDio1 {
    async fn wait_for_high(&mut self) -> Result<(), MockError> {
        self.state.borrow_mut().trace.push(TraceEvent::Dio1High);
        Ok(())
    }

    async fn wait_for_low(&mut self) -> Result<(), MockError> {
        self.state.borrow_mut().trace.push(TraceEvent::Dio1Low);
        Ok(())
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), MockError> {
        Ok(())
    }
}

struct MockOutput;

impl DigitalErrorType for MockOutput {
    type Error = MockError;
}

impl OutputPin for MockOutput {
    fn set_low(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), MockError> {
        Ok(())
    }
}

struct MockDelay;

impl DelayNs for MockDelay {
    async fn delay_ns(&mut self, _nanoseconds: u32) {}
}

struct BusyNeverLow;

impl DigitalErrorType for BusyNeverLow {
    type Error = MockError;
}

impl Wait for BusyNeverLow {
    async fn wait_for_high(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_low(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }
}

struct Dio1NeverHigh;

impl DigitalErrorType for Dio1NeverHigh {
    type Error = MockError;
}

impl Wait for Dio1NeverHigh {
    async fn wait_for_high(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }

    async fn wait_for_low(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }
}

struct Dio1NeverLow;

impl DigitalErrorType for Dio1NeverLow {
    type Error = MockError;
}

impl Wait for Dio1NeverLow {
    async fn wait_for_high(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_low(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), MockError> {
        Ok(())
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), MockError> {
        core::future::pending().await
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), MockError> {
        Ok(())
    }
}

type MockRadio = Lr1110<MockSpi, MockBusy, MockDio1, MockOutput, MockDelay>;

fn board() -> BoardConfig {
    BoardConfig {
        reference_clock: ReferenceClock::Tcxo {
            voltage: TcxoVoltage::V1_6,
            startup_time: TcxoStartupTime::from_rtc_ticks(164),
        },
        regulator: RegulatorMode::Dcdc,
        receive_gain: ReceiveGain::Boosted,
        rf_switch: RfSwitchConfig {
            enabled: RfSwitchPins::RFSW0
                .union(RfSwitchPins::RFSW1)
                .union(RfSwitchPins::RFSW2)
                .union(RfSwitchPins::RFSW3),
            standby: RfSwitchPins::NONE,
            receive: RfSwitchPins::RFSW0.union(RfSwitchPins::RFSW3),
            transmit: RfSwitchPins::RFSW0
                .union(RfSwitchPins::RFSW1)
                .union(RfSwitchPins::RFSW3),
            transmit_high_power: RfSwitchPins::RFSW1.union(RfSwitchPins::RFSW3),
            transmit_high_frequency: RfSwitchPins::NONE,
            gnss: RfSwitchPins::RFSW2,
            wifi: RfSwitchPins::NONE,
        },
        power_amplifier: PowerAmplifierTable::new(20, &TEST_PA_CONFIGS),
        transmit_ramp_time: TransmitRampTime::Us48,
        external_receive_gain_db: 0,
    }
}

fn mock_radio() -> (MockRadio, SharedState) {
    let state = Rc::new(RefCell::new(MockState::new()));
    let radio = Lr1110::new(
        MockSpi {
            state: state.clone(),
        },
        MockBusy {
            state: state.clone(),
        },
        MockDio1 {
            state: state.clone(),
        },
        MockOutput,
        MockDelay,
        board(),
    );
    (radio, state)
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = Box::pin(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}

fn profile_with_power(power_dbm: i8) -> RadioProfile {
    let mut profile = US915_AUTO_LORA_PROFILE;
    profile.tx_power = TxPower::new(power_dbm);
    profile
}

#[test]
fn reticulum_profile_maps_to_lr1110_configuration() {
    assert_eq!(
        radio_config(US915_AUTO_LORA_PROFILE),
        RadioConfig {
            frequency_hz: 921_500_000,
            modulation: LoraModulation {
                spreading_factor: SpreadingFactor::Sf7,
                bandwidth: Bandwidth::Bw500,
                coding_rate: CodingRate::Cr4_5,
            },
            packet: LoraPacket {
                preamble_symbols: 18,
                header: HeaderMode::Explicit,
                crc: PayloadCrc::Enabled,
                invert_iq: InvertIq::Standard,
            },
            network: LoRaNetwork::Reticulum,
            tx_power_dbm: 22,
        }
    );
}

#[test]
fn board_pa_table_owns_transmit_power_compatibility() {
    let (radio, _) = mock_radio();
    assert_eq!(radio.validate_profile(profile_with_power(20)), Ok(()));
    assert_eq!(radio.validate_profile(profile_with_power(22)), Ok(()));
    assert_eq!(
        radio.validate_profile(profile_with_power(19)),
        Err(
            RadioProfileCompatibilityError::TransmitPowerOutsideRadioRange {
                power_dbm: 19,
                minimum_dbm: 20,
                maximum_dbm: 22,
            }
        )
    );
}

#[test]
fn lr1110_recovery_classifies_every_error() {
    for error in [
        Error::Spi,
        Error::Busy,
        Error::Dio1,
        Error::Reset,
        Error::DeviceNotReady,
        Error::UnexpectedDevice(2),
        Error::CommandRejected,
        Error::NotInitialized,
        Error::Timeout,
        Error::UnexpectedInterrupt(irq::PREAMBLE_DETECTED),
    ] {
        assert_eq!(MockRadio::recovery(&error), RadioRecovery::Reinitialize);
    }
    for error in [
        Error::UnsupportedTransmitPower(19),
        Error::Crc,
        Error::BufferTooSmall,
    ] {
        assert_eq!(MockRadio::recovery(&error), RadioRecovery::Continue);
    }
}

#[test]
fn receive_irq_classification_preserves_channel_evidence() {
    assert_eq!(
        classify_receive_irq(irq::PREAMBLE_DETECTED),
        Ok(IrqEventKind::PreambleDetected)
    );
    assert_eq!(
        classify_receive_irq(irq::PREAMBLE_DETECTED | irq::HEADER_VALID),
        Ok(IrqEventKind::HeaderValid)
    );
    assert_eq!(
        classify_receive_irq(irq::RX_DONE | irq::HEADER_VALID),
        Ok(IrqEventKind::Frame)
    );
    assert_eq!(
        classify_receive_irq(irq::RX_DONE | irq::CRC_ERROR),
        Ok(IrqEventKind::CrcError)
    );
    assert_eq!(
        classify_receive_irq(irq::HEADER_ERROR),
        Ok(IrqEventKind::HeaderError)
    );
    assert_eq!(
        classify_receive_irq(irq::TIMEOUT),
        Ok(IrqEventKind::Timeout)
    );
    assert_eq!(
        classify_receive_irq(irq::TX_DONE),
        Ok(IrqEventKind::SpuriousInterrupt)
    );
    assert_eq!(
        classify_receive_irq(irq::COMMAND_ERROR),
        Err(Error::CommandRejected)
    );
}

#[test]
fn ldro_uses_the_actual_symbol_duration_boundary() {
    assert_eq!(lora_ldro(SpreadingFactor::Sf10, Bandwidth::Bw125), 0);
    assert_eq!(lora_ldro(SpreadingFactor::Sf11, Bandwidth::Bw125), 1);
    assert_eq!(lora_ldro(SpreadingFactor::Sf12, Bandwidth::Bw250), 1);
    assert_eq!(lora_ldro(SpreadingFactor::Sf12, Bandwidth::Bw500), 0);
}

#[test]
fn command_stream_matches_lr1110_protocol_and_board_policy() {
    let (mut radio, state) = mock_radio();
    let profile = profile_with_power(21);
    block_on(radio.initialize(profile)).expect("initialize");
    state
        .borrow_mut()
        .irq_statuses
        .extend([irq::TX_DONE, irq::RX_DONE, irq::RX_DONE]);
    block_on(radio.transmit(b"PRNS-LR1110-SMOK")).expect("transmit");
    let mut buffer = [0; MAX_LORA_PAYLOAD];
    let received = block_on(radio.receive(&mut buffer)).expect("receive");
    assert_eq!(&buffer[..received.len], b"PRNS-LR1110-SMOK");
    assert_eq!(
        received.phy,
        PacketPhyStats {
            rssi: Some(RssiDbm::new(-90)),
            snr: Some(SnrQuarterDb::new(-9)),
            quality: None,
        }
    );
    assert_eq!(block_on(radio.channel_rssi_dbm()), Ok(-86));

    let state = state.borrow();
    let has = |command: &[u8]| {
        state
            .commands
            .iter()
            .any(|candidate| candidate.as_slice() == command)
    };
    let count = |command: &[u8]| {
        state
            .commands
            .iter()
            .filter(|candidate| candidate.as_slice() == command)
            .count()
    };
    let position = |command: &[u8]| {
        state
            .commands
            .iter()
            .position(|candidate| candidate.as_slice() == command)
            .expect("command")
    };

    assert!(has(&[0x01, 0x1c, 0x00]));
    assert!(has(&[0x01, 0x0e]));
    assert!(has(&[0x01, 0x14, 0x00, 0xc0, 0x04, 0xfc]));
    assert!(has(&[0x01, 0x10, 0x01]));
    assert!(has(&[
        0x01, 0x12, 0x0f, 0x00, 0x09, 0x0b, 0x0a, 0x00, 0x04, 0x00
    ]));
    assert!(has(&[0x01, 0x17, 0x00, 0x00, 0x00, 0xa4]));
    assert!(has(&[0x01, 0x0f, 0x3f]));
    assert!(has(&[0x02, 0x0e, 0x02]));
    assert!(has(&[0x02, 0x2b, 0x12]));
    assert!(has(&[0x02, 0x0b, 0x36, 0xec, 0xf9, 0x60]));
    assert!(has(&[0x02, 0x0f, 0x07, 0x06, 0x01, 0x00]));
    assert!(has(&[0x02, 0x15, 0x01, 0x01, 0x05, 0x07]));
    assert!(has(&[0x02, 0x11, 22, 0x02]));
    assert!(has(&[0x02, 0x10, 0x00, 0x12, 0x00, 0xff, 0x01, 0x00]));
    assert!(has(&[0x02, 0x27, 0x01]));
    assert!(has(&[0x01, 0x13, 0x00, 0xc0, 0x04, 0xfc, 0, 0, 0, 0]));
    assert!(has(&[0x02, 0x10, 0x00, 0x12, 0x00, 16, 0x01, 0x00]));
    assert!(has(b"\x01\x09PRNS-LR1110-SMOK"));
    assert!(has(&[0x02, 0x0a, 0x00, 0x00, 0x00]));
    assert!(has(&[0x02, 0x09, 0xff, 0xff, 0xff]));
    assert!(has(&[0x01, 0x0a, 250, 6]));
    assert!(has(&[0x01, 0x0a, 0, 10]));
    assert_eq!(count(&[0x02, 0x0f, 0x07, 0x06, 0x01, 0x00]), 1);
    assert_eq!(count(&[0x02, 0x15, 0x01, 0x01, 0x05, 0x07]), 1);
    assert_eq!(count(&[0x02, 0x11, 22, 0x02]), 1);
    assert!(position(&[0x01, 0x0e]) < position(&[0x01, 0x17, 0x00, 0x00, 0x00, 0xa4]));
    assert!(
        position(&[0x01, 0x14, 0x00, 0xc0, 0x04, 0xfc])
            < position(&[0x01, 0x17, 0x00, 0x00, 0x00, 0xa4])
    );
    assert!(state.all_read_clocks_were_zero);
    for (index, event) in state.trace.iter().enumerate() {
        if *event == TraceEvent::Spi {
            assert!(index > 0);
            assert_eq!(state.trace[index - 1], TraceEvent::BusyLow);
        }
    }
}

#[test]
fn legacy_firmware_uses_the_compatible_private_network_command() {
    let (mut radio, state) = mock_radio();
    state.borrow_mut().firmware = FirmwareVersion(0x0302);
    block_on(radio.initialize(profile_with_power(22))).expect("initialize");
    let state = state.borrow();
    assert!(state
        .commands
        .iter()
        .any(|command| command.as_slice() == [0x02, 0x08, 0x00]));
    assert!(!state
        .commands
        .iter()
        .any(|command| command.as_slice() == [0x02, 0x2b, 0x12]));
}

#[test]
fn external_receive_gain_is_removed_from_reported_rssi() {
    assert_eq!(antenna_referred_rssi_dbm(172, 0), -86);
    assert_eq!(antenna_referred_rssi_dbm(172, 17), -103);
    assert_eq!(antenna_referred_rssi_dbm(172, 23), -109);
}

#[test]
fn operations_before_initialization_are_rejected() {
    let (mut radio, _) = mock_radio();
    assert_eq!(block_on(radio.arm_rx()), Err(Error::NotInitialized));
    assert_eq!(
        block_on(radio.transmit(b"PRNS-LR1110-SMOK")),
        Err(Error::NotInitialized)
    );
}

#[test]
fn wrong_radio_kind_is_reported() {
    let (mut radio, state) = mock_radio();
    state.borrow_mut().device_kind = 0x02;
    assert_eq!(
        block_on(radio.initialize(profile_with_power(22))),
        Err(Error::UnexpectedDevice(0x02))
    );
}

#[test]
fn initialization_rejects_a_radio_command_failure() {
    let (mut radio, state) = mock_radio();
    state.borrow_mut().command_status = COMMAND_STATUS_FAILED;
    assert_eq!(
        block_on(radio.initialize(profile_with_power(22))),
        Err(Error::CommandRejected)
    );
    assert_eq!(block_on(radio.arm_rx()), Err(Error::NotInitialized));
}

#[test]
fn a_wedged_busy_line_times_out() {
    let state = Rc::new(RefCell::new(MockState::new()));
    let mut radio = Lr1110::new(
        MockSpi {
            state: state.clone(),
        },
        BusyNeverLow,
        MockDio1 { state },
        MockOutput,
        MockDelay,
        board(),
    );
    assert_eq!(
        block_on(radio.initialize(profile_with_power(22))),
        Err(Error::Busy)
    );
}

#[test]
fn a_txdone_that_never_arrives_times_out() {
    let state = Rc::new(RefCell::new(MockState::new()));
    let mut radio = Lr1110::new(
        MockSpi {
            state: state.clone(),
        },
        MockBusy {
            state: state.clone(),
        },
        Dio1NeverHigh,
        MockOutput,
        MockDelay,
        board(),
    );
    block_on(radio.initialize(profile_with_power(22))).expect("initialize");
    assert_eq!(
        block_on(radio.transmit(b"PRNS-LR1110-SMOK")),
        Err(Error::Timeout)
    );
}

#[test]
fn a_txdone_that_never_releases_times_out() {
    let state = Rc::new(RefCell::new(MockState::new()));
    let mut radio = Lr1110::new(
        MockSpi {
            state: state.clone(),
        },
        MockBusy {
            state: state.clone(),
        },
        Dio1NeverLow,
        MockOutput,
        MockDelay,
        board(),
    );
    block_on(radio.initialize(profile_with_power(22))).expect("initialize");
    state.borrow_mut().irq_statuses.push_back(irq::TX_DONE);
    assert_eq!(
        block_on(radio.transmit(b"PRNS-LR1110-SMOK")),
        Err(Error::Timeout)
    );
}

#[test]
fn transmit_requires_txdone_not_just_an_interrupt() {
    let (mut radio, state) = mock_radio();
    block_on(radio.initialize(profile_with_power(22))).expect("initialize");
    state
        .borrow_mut()
        .irq_statuses
        .push_back(irq::PREAMBLE_DETECTED);
    assert_eq!(
        block_on(radio.transmit(b"PRNS-LR1110-SMOK")),
        Err(Error::UnexpectedInterrupt(irq::PREAMBLE_DETECTED))
    );
}
