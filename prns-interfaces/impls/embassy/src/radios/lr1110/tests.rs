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
use prns_core::interfaces::lora::TxPower;
use prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;

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
            device_kind: Lr11xxPart::Lr1110 as u8,
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
        core::future::poll_fn(|_| {
            let mut state = self.state.borrow_mut();
            if state.irq_statuses.is_empty() {
                return Poll::Pending;
            }
            state.trace.push(TraceEvent::Dio1High);
            Poll::Ready(Ok(()))
        })
        .await
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
    board_for(Lr11xxPart::Lr1110)
}

fn board_for(part: Lr11xxPart) -> BoardConfig {
    BoardConfig {
        #[cfg(feature = "lora-2g4")]
        high_frequency: HighFrequencyPath::Unavailable,
        part,
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
    mock_radio_for(board())
}

fn mock_radio_for(board: BoardConfig) -> (MockRadio, SharedState) {
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
        board,
    );
    (radio, state)
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = Box::pin(future);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "chip test failed to settle"
        );
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}

fn profile_with_power(power_dbm: i8) -> RadioProfile {
    US915_AUTO_LORA_PROFILE
        .with_tx_power(TxPower::new(power_dbm))
        .unwrap()
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
        Error::UnexpectedDevice {
            expected: Lr11xxPart::Lr1110,
            observed: 0x03,
        },
        Error::CommandRejected,
        Error::NotInitialized,
        Error::Timeout,
        Error::UnexpectedInterrupt(irq::PREAMBLE_DETECTED),
    ] {
        assert_eq!(MockRadio::recovery(&error), RadioRecovery::Reinitialize);
    }
    for error in [
        Error::UnsupportedTransmitPower(19),
        Error::UnsupportedProfile,
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
fn dropping_a_pending_receive_preserves_the_latched_frame() {
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
    block_on(radio.arm_rx()).expect("arm receive");
    state.borrow_mut().irq_statuses.push_back(irq::RX_DONE);

    let mut buffer = [0; MAX_LORA_PAYLOAD];
    {
        let mut receive = Box::pin(radio.read_event(&mut buffer));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(receive.as_mut().poll(&mut context).is_pending());
    }

    let event = block_on(radio.poll_event(&mut buffer)).expect("poll latched event");
    assert!(matches!(event, Some(RadioEvent::Frame(frame)) if frame.len == 16));
    assert_eq!(&buffer[..16], b"PRNS-LR1110-SMOK");
}

#[test]
fn an_lr1110_board_rejects_an_lr1121_chip() {
    let (mut radio, state) = mock_radio();
    state.borrow_mut().device_kind = Lr11xxPart::Lr1121 as u8;
    assert_eq!(
        block_on(radio.initialize(profile_with_power(22))),
        Err(Error::UnexpectedDevice {
            expected: Lr11xxPart::Lr1110,
            observed: 0x03,
        })
    );
}

#[test]
fn an_lr1121_board_rejects_an_lr1110_chip() {
    let (mut radio, state) = mock_radio_for(board_for(Lr11xxPart::Lr1121));
    state.borrow_mut().device_kind = Lr11xxPart::Lr1110 as u8;
    assert_eq!(
        block_on(radio.initialize(profile_with_power(22))),
        Err(Error::UnexpectedDevice {
            expected: Lr11xxPart::Lr1121,
            observed: 0x01,
        })
    );
}

#[test]
fn an_lr1121_board_initializes_and_sets_the_sync_word_on_early_firmware() {
    let (mut radio, state) = mock_radio_for(board_for(Lr11xxPart::Lr1121));
    {
        let mut state = state.borrow_mut();
        state.device_kind = Lr11xxPart::Lr1121 as u8;
        state.firmware = FirmwareVersion(0x0101);
    }
    block_on(radio.initialize(profile_with_power(22))).expect("initialize");
    let state = state.borrow();
    assert!(state
        .commands
        .iter()
        .any(|command| command.as_slice() == [0x02, 0x2b, 0x12]));
    assert!(!state
        .commands
        .iter()
        .any(|command| command.as_slice() == [0x02, 0x08, 0x00]));
}

#[test]
fn semtech_sub_ghz_table_spans_the_lr11xx_output_range() {
    assert_eq!(
        SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE.minimum_output_power_dbm(),
        -17
    );
    assert_eq!(
        SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE.maximum_output_power_dbm(),
        22
    );
    assert_eq!(
        SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE.configuration(22),
        Some(PowerAmplifierConfig {
            chip_output_power_dbm: 22,
            selection: PowerAmplifierSelection::HighPower,
            supply: PowerAmplifierSupply::Battery,
            duty_cycle: PowerAmplifierDutyCycle::new(4),
            high_power_selection: HighPowerSelection::new(7),
        })
    );
}

#[test]
fn semtech_high_power_rows_stay_within_the_duty_cycle_ceiling() {
    const HIGH_POWER_DUTY_CYCLE_CEILING: u8 = 4;
    for output_power_dbm in 16..=22 {
        let config = SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE
            .configuration(output_power_dbm)
            .expect("high-power row");
        assert_eq!(config.selection, PowerAmplifierSelection::HighPower);
        assert_eq!(config.supply, PowerAmplifierSupply::Battery);
        assert!(config.duty_cycle.value() <= HIGH_POWER_DUTY_CYCLE_CEILING);
    }
    for output_power_dbm in -17..=15 {
        let config = SEMTECH_SUB_GHZ_POWER_AMPLIFIER_TABLE
            .configuration(output_power_dbm)
            .expect("low-power row");
        assert_eq!(config.selection, PowerAmplifierSelection::LowPower);
        assert_eq!(config.supply, PowerAmplifierSupply::Regulator);
    }
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

#[cfg(feature = "lora-2g4")]
#[test]
fn lr1121_high_frequency_commands_use_the_hf_pa_and_return_to_subg() {
    use prns_core::interfaces::lora::{
        CodingRate as ProfileCr, LoraBandwidth, Modulation, SpreadingFactor as ProfileSf,
        GHZ24_BALANCED_PROFILE,
    };
    let mut board = board_for(Lr11xxPart::Lr1121);
    board.high_frequency = HighFrequencyPath::Regulated {
        maximum_power_dbm: 11,
    };
    let (mut radio, state) = mock_radio_for(board);
    state.borrow_mut().device_kind = Lr11xxPart::Lr1121 as u8;
    for (bandwidth, code) in [
        (LoraBandwidth::Bw203kHz, 0x0d),
        (LoraBandwidth::Bw406kHz, 0x0e),
        (LoraBandwidth::Bw812kHz, 0x0f),
    ] {
        let profile = GHZ24_BALANCED_PROFILE
            .with_modulation(Modulation::Lora {
                spreading_factor: ProfileSf::Sf7,
                bandwidth,
                coding_rate: ProfileCr::Cr45,
            })
            .unwrap();
        assert_eq!(
            radio.validate_band_profile(LoRaProfile::Ghz24(profile)),
            Ok(())
        );
        block_on(radio.initialize_band(LoRaProfile::Ghz24(profile))).unwrap();
        let state = state.borrow();
        assert!(state
            .commands
            .iter()
            .any(|command| command.as_slice() == [0x02, 0x0f, 7, code, 1, 0]));
        assert!(state
            .commands
            .iter()
            .any(|command| command.as_slice() == [0x02, 0x15, 2, 0, 0, 0]));
        assert!(state
            .commands
            .iter()
            .any(|command| command.as_slice() == [0x02, 0x11, 10, 2]));
        assert!(state
            .commands
            .iter()
            .any(|command| command.as_slice() == [0x02, 0x10, 0, 18, 0, 255, 1, 0]));
    }
    state.borrow_mut().commands.clear();
    block_on(radio.initialize(profile_with_power(21))).unwrap();
    assert!(state
        .borrow()
        .commands
        .iter()
        .any(|command| command.as_slice() == [0x02, 0x15, 1, 1, 5, 7]));
    assert_eq!(lora_ldro(SpreadingFactor::Sf12, Bandwidth::Bw203), 1);
    assert_eq!(lora_ldro(SpreadingFactor::Sf11, Bandwidth::Bw203), 0);
    assert_eq!(lora_ldro(SpreadingFactor::Sf12, Bandwidth::Bw406), 0);
    assert_eq!(lora_ldro(SpreadingFactor::Sf12, Bandwidth::Bw812), 0);
}

#[cfg(feature = "lora-2g4")]
#[test]
fn unsupported_hf_hardware_and_power_are_rejected_before_spi() {
    use prns_core::interfaces::lora::GHZ24_BALANCED_PROFILE;
    for (part, path) in [
        (Lr11xxPart::Lr1110, HighFrequencyPath::Unavailable),
        (
            Lr11xxPart::Lr1110,
            HighFrequencyPath::Regulated {
                maximum_power_dbm: 11,
            },
        ),
        (Lr11xxPart::Lr1121, HighFrequencyPath::Unavailable),
    ] {
        let mut board = board_for(part);
        board.high_frequency = path;
        let (mut radio, state) = mock_radio_for(board);
        let profile = LoRaProfile::Ghz24(GHZ24_BALANCED_PROFILE);
        assert_eq!(
            radio.validate_band_profile(profile),
            Err(RadioProfileCompatibilityError::UnsupportedBand)
        );
        assert_eq!(
            block_on(radio.initialize_band(profile)),
            Err(BandRadioError::Radio(Error::UnsupportedProfile))
        );
        assert!(state.borrow().commands.is_empty());
    }
    let mut board = board_for(Lr11xxPart::Lr1121);
    board.high_frequency = HighFrequencyPath::Regulated {
        maximum_power_dbm: 11,
    };
    let (mut radio, state) = mock_radio_for(board);
    let profile = LoRaProfile::Ghz24(
        GHZ24_BALANCED_PROFILE
            .with_tx_power(TxPower::new(12))
            .unwrap(),
    );
    assert_eq!(
        radio.validate_band_profile(profile),
        Err(
            RadioProfileCompatibilityError::TransmitPowerOutsideRadioRange {
                power_dbm: 12,
                minimum_dbm: -18,
                maximum_dbm: 11
            }
        )
    );
    assert_eq!(
        block_on(radio.initialize_band(profile)),
        Err(BandRadioError::Radio(Error::UnsupportedProfile))
    );
    assert!(state.borrow().commands.is_empty());
}

#[test]
fn modulation_encoding_covers_every_supported_sf_bandwidth_and_rate() {
    use prns_core::interfaces::lora::{
        CodingRate as Cr, LoraBandwidth as Bw, Modulation, SpreadingFactor as Sf,
    };
    for (sf, encoded_sf) in [
        (Sf::Sf5, 5),
        (Sf::Sf6, 6),
        (Sf::Sf7, 7),
        (Sf::Sf8, 8),
        (Sf::Sf9, 9),
        (Sf::Sf10, 10),
        (Sf::Sf11, 11),
        (Sf::Sf12, 12),
    ] {
        for (bw, encoded_bw) in [(Bw::Bw125kHz, 4), (Bw::Bw250kHz, 5), (Bw::Bw500kHz, 6)] {
            for (cr, encoded_cr) in [(Cr::Cr45, 1), (Cr::Cr46, 2), (Cr::Cr47, 3), (Cr::Cr48, 4)] {
                let profile = US915_AUTO_LORA_PROFILE
                    .with_modulation(Modulation::Lora {
                        spreading_factor: sf,
                        bandwidth: bw,
                        coding_rate: cr,
                    })
                    .unwrap();
                let config = radio_config(profile);
                assert_eq!(
                    [
                        config.modulation.spreading_factor as u8,
                        config.modulation.bandwidth as u8,
                        config.modulation.coding_rate as u8
                    ],
                    [encoded_sf, encoded_bw, encoded_cr]
                );
                let expected_ldro = u8::from(
                    (encoded_sf == 11 && encoded_bw == 4) || (encoded_sf == 12 && encoded_bw <= 5),
                );
                assert_eq!(
                    lora_ldro(
                        config.modulation.spreading_factor,
                        config.modulation.bandwidth
                    ),
                    expected_ldro
                );
            }
        }
    }
    assert_eq!(
        classify_receive_irq(irq::CRC_ERROR),
        Ok(IrqEventKind::CrcError)
    );
    let table = PowerAmplifierTable::new(20, &TEST_PA_CONFIGS);
    assert_eq!(table.configuration(19), None);
    assert_eq!(table.configuration(23), None);
}

#[test]
fn receive_and_transmit_statuses_preserve_driver_error_contracts() {
    let (mut radio, state) = mock_radio();
    block_on(crate::radios::LoRaRadio::initialize(
        &mut radio,
        US915_AUTO_LORA_PROFILE,
    ))
    .unwrap();
    block_on(crate::radios::LoRaRadio::idle(&mut radio)).unwrap();
    block_on(crate::radios::LoRaRadio::arm_rx(&mut radio)).unwrap();
    assert_eq!(
        block_on(crate::radios::LoRaRadio::channel_rssi_dbm(&mut radio)),
        Ok(-86)
    );
    let mut buffer = [0; 255];
    assert_eq!(
        block_on(crate::radios::LoRaRadio::poll_event(
            &mut radio,
            &mut buffer
        )),
        Ok(None)
    );
    for (flags, expected) in [
        (irq::PREAMBLE_DETECTED, RadioEvent::PreambleDetected),
        (irq::HEADER_VALID, RadioEvent::HeaderValid),
        (irq::HEADER_ERROR, RadioEvent::HeaderError),
        (irq::CRC_ERROR, RadioEvent::CrcError),
        (irq::TIMEOUT, RadioEvent::Timeout),
        (irq::TX_DONE, RadioEvent::SpuriousInterrupt),
    ] {
        state.borrow_mut().irq_statuses.push_back(flags);
        assert_eq!(
            block_on(crate::radios::LoRaRadio::read_event(
                &mut radio,
                &mut buffer
            )),
            Ok(expected)
        );
    }
    state.borrow_mut().rx_offset = 0;
    state.borrow_mut().irq_statuses.push_back(irq::RX_DONE);
    let frame = block_on(crate::radios::LoRaRadio::poll_event(
        &mut radio,
        &mut buffer,
    ))
    .unwrap()
    .unwrap();
    assert!(matches!(
        frame,
        RadioEvent::Frame(ReceivedAirFrame { len: 16, .. })
    ));
    state.borrow_mut().irq_statuses.push_back(irq::RX_DONE);
    assert_eq!(
        block_on(radio.read_event(&mut [])),
        Err(Error::BufferTooSmall)
    );
    for (flags, error) in [(irq::CRC_ERROR, Error::Crc), (irq::TIMEOUT, Error::Timeout)] {
        state.borrow_mut().irq_statuses.extend([
            irq::PREAMBLE_DETECTED,
            irq::HEADER_VALID,
            irq::HEADER_ERROR,
            irq::TX_DONE,
            flags,
        ]);
        assert_eq!(block_on(radio.read_frame(&mut buffer)), Err(error));
    }
    assert_eq!(
        block_on(crate::radios::LoRaRadio::transmit(&mut radio, &[0; 256])),
        Err(Error::BufferTooSmall)
    );
    for (flags, expected) in [
        (irq::HARDWARE_ERRORS, Err(Error::CommandRejected)),
        (irq::TIMEOUT, Err(Error::Timeout)),
        (irq::TX_DONE, Ok(())),
    ] {
        state.borrow_mut().irq_statuses.push_back(flags);
        assert_eq!(
            block_on(crate::radios::LoRaRadio::transmit(&mut radio, &[1, 2, 3])),
            expected
        );
    }
}

#[test]
fn chip_startup_handles_crystal_clock_pending_version_and_latched_irqs() {
    let mut config = board();
    config.reference_clock = ReferenceClock::Crystal;
    let (mut radio, state) = mock_radio_for(config);
    state
        .borrow_mut()
        .irq_statuses
        .push_back(irq::PREAMBLE_DETECTED);
    block_on(radio.initialize_profile(US915_AUTO_LORA_PROFILE.into())).unwrap();
    assert!(!state
        .borrow()
        .commands
        .iter()
        .any(|command| command.starts_with(&opcode(op::SET_TCXO_MODE))));
    state.borrow_mut().device_kind = DEVICE_KIND_NOT_READY;
    assert_eq!(
        block_on(radio.initialize_profile(US915_AUTO_LORA_PROFILE.into())),
        Err(Error::DeviceNotReady)
    );
}

#[test]
fn rf_switch_pin_sets_preserve_each_bit_and_idempotent_union() {
    let pins = [
        RfSwitchPins::RFSW0,
        RfSwitchPins::RFSW1,
        RfSwitchPins::RFSW2,
        RfSwitchPins::RFSW3,
        RfSwitchPins::RFSW4,
    ];
    for (pin, expected) in pins.into_iter().zip([1, 2, 4, 8, 16]) {
        assert_eq!(pin.bits(), expected);
        assert_eq!(pin.union(pin), pin);
        assert_eq!(pin.union(RfSwitchPins::NONE), pin);
    }
    for mask in 0..32u8 {
        let mut combined = RfSwitchPins::NONE;
        for (index, pin) in pins.into_iter().enumerate() {
            if mask & (1 << index) != 0 {
                combined = combined.union(pin);
            }
        }
        assert_eq!(combined.bits(), mask);
    }
}

#[test]
fn chip_deadlines_yield_until_the_pin_or_timer_settles() {
    struct YieldingDelay;
    impl DelayNs for YieldingDelay {
        async fn delay_ns(&mut self, _: u32) {
            embassy_futures::yield_now().await;
        }
    }
    let future = async {
        embassy_futures::yield_now().await;
        Ok::<(), MockError>(())
    };
    assert_eq!(
        block_on(deadline(
            future,
            &mut YieldingDelay,
            100,
            Error::Busy,
            Error::Timeout
        )),
        Ok(())
    );
    assert_eq!(
        block_on(deadline(
            core::future::ready(Err(MockError)),
            &mut YieldingDelay,
            100,
            Error::Busy,
            Error::Timeout
        )),
        Err(Error::Busy)
    );
}

#[test]
fn initialization_latched_hardware_errors_fail_closed_and_pa_lookup_is_checked() {
    let (mut radio, state) = mock_radio();
    state
        .borrow_mut()
        .irq_statuses
        .push_back(irq::COMMAND_ERROR);
    assert_eq!(
        block_on(radio.initialize_profile(US915_AUTO_LORA_PROFILE.into())),
        Err(Error::CommandRejected)
    );
    assert_eq!(block_on(radio.arm_rx()), Err(Error::NotInitialized));
    assert_eq!(
        block_on(radio.set_transmit_power(19)),
        Err(Error::UnsupportedTransmitPower(19))
    );
}

#[test]
#[should_panic]
fn pa_table_refuses_a_range_extending_above_the_signed_power_domain() {
    let _ = PowerAmplifierTable::new(std::hint::black_box(126), &TEST_PA_CONFIGS);
}

#[test]
#[should_panic]
fn pa_table_requires_at_least_one_calibrated_row() {
    let _ = PowerAmplifierTable::new(std::hint::black_box(0), &[]);
}

#[test]
fn pa_table_accepts_the_exact_signed_domain_boundary() {
    let table = PowerAmplifierTable::new(125, &TEST_PA_CONFIGS);
    assert_eq!(table.minimum_output_power_dbm(), 125);
    assert_eq!(table.maximum_output_power_dbm(), 127);
    assert_eq!(table.configuration(127), Some(TEST_PA_CONFIGS[2]));
}

#[test]
fn sync_word_command_switches_at_the_first_supported_lr1110_firmware() {
    for version in [0x0302, 0x0303, 0x0304] {
        assert_eq!(
            sync_word_command(Lr11xxPart::Lr1110, FirmwareVersion(version)),
            if version < 0x0303 {
                SyncWordCommand::SetLoraPublicNetwork
            } else {
                SyncWordCommand::SetLoraSyncWord
            }
        );
        assert_eq!(
            sync_word_command(Lr11xxPart::Lr1121, FirmwareVersion(version)),
            SyncWordCommand::SetLoraSyncWord
        );
    }
}

#[test]
fn pa_table_can_span_every_signed_power_value() {
    static ROWS: [PowerAmplifierConfig; 256] = [TEST_PA_CONFIGS[0]; 256];
    let table = PowerAmplifierTable::new(std::hint::black_box(i8::MIN), &ROWS);
    assert_eq!(table.maximum_output_power_dbm(), i8::MAX);
    for power in i8::MIN..=i8::MAX {
        assert_eq!(table.configuration(power), Some(TEST_PA_CONFIGS[0]));
    }
}

#[test]
#[should_panic]
fn pa_table_cannot_have_more_rows_than_the_power_domain() {
    static ROWS: [PowerAmplifierConfig; 257] = [TEST_PA_CONFIGS[0]; 257];
    let _ = PowerAmplifierTable::new(std::hint::black_box(i8::MIN), &ROWS);
}

#[test]
fn hard_reset_observes_pin_order_and_minimum_settling_delays() {
    #[derive(Debug, PartialEq, Eq)]
    enum ResetEvent {
        Low,
        High,
        Delay(u32),
    }
    struct ResetPin(Rc<RefCell<Vec<ResetEvent>>>);
    impl DigitalErrorType for ResetPin {
        type Error = MockError;
    }
    impl OutputPin for ResetPin {
        fn set_low(&mut self) -> Result<(), MockError> {
            self.0.borrow_mut().push(ResetEvent::Low);
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), MockError> {
            self.0.borrow_mut().push(ResetEvent::High);
            Ok(())
        }
    }
    struct ResetDelay(Rc<RefCell<Vec<ResetEvent>>>);
    impl DelayNs for ResetDelay {
        async fn delay_ns(&mut self, ns: u32) {
            self.0.borrow_mut().push(ResetEvent::Delay(ns));
        }
    }
    let state = Rc::new(RefCell::new(MockState::new()));
    let reset_events = Rc::new(RefCell::new(Vec::new()));
    let mut radio = Lr1110::new(
        MockSpi {
            state: state.clone(),
        },
        MockBusy {
            state: state.clone(),
        },
        MockDio1 {
            state: state.clone(),
        },
        ResetPin(reset_events.clone()),
        ResetDelay(reset_events.clone()),
        board(),
    );
    assert_eq!(block_on(radio.hard_reset()), Ok(()));
    assert_eq!(
        *reset_events.borrow(),
        [
            ResetEvent::Low,
            ResetEvent::Delay(RESET_ASSERT_MS * 1_000_000),
            ResetEvent::High,
            ResetEvent::Delay(RESET_BOOT_MS * 1_000_000)
        ]
    );
    assert_eq!(state.borrow().trace, [TraceEvent::BusyLow]);
}

#[test]
fn irq_command_status_and_initialization_acknowledgements_match_the_wire() {
    let (mut radio, state) = mock_radio();
    for command_status in 0..4 {
        state.borrow_mut().command_status = command_status;
        state.borrow_mut().irq_statuses.push_back(irq::HEADER_VALID);
        assert_eq!(
            block_on(radio.irq_status()),
            match command_status {
                COMMAND_STATUS_FAILED | COMMAND_STATUS_PARAMETER_ERROR =>
                    Err(Error::CommandRejected),
                _ => Ok(irq::HEADER_VALID),
            }
        );
    }
    state.borrow_mut().command_status = 2;
    for flags in [0, irq::HEADER_VALID, irq::COMMAND_ERROR] {
        state.borrow_mut().commands.clear();
        state.borrow_mut().irq_statuses.push_back(flags);
        assert_eq!(
            block_on(radio.validate_initialization()),
            if flags == irq::COMMAND_ERROR {
                Err(Error::CommandRejected)
            } else {
                Ok(())
            }
        );
        let expected = if flags == 0 {
            Vec::new()
        } else {
            vec![[
                opcode(op::CLEAR_IRQ).as_slice(),
                flags.to_be_bytes().as_slice(),
            ]
            .concat()]
        };
        assert_eq!(state.borrow().commands, expected);
    }
}

#[test]
fn maximum_payload_and_exact_receive_capacity_remain_usable() {
    let (mut radio, state) = mock_radio();
    block_on(radio.initialize_profile(US915_AUTO_LORA_PROFILE.into())).unwrap();
    state.borrow_mut().irq_statuses.push_back(irq::TX_DONE);
    assert_eq!(block_on(radio.transmit(&[0xa5; MAX_LORA_PAYLOAD])), Ok(()));
    for (offset, length) in [
        (0, 0),
        (0, 255),
        (1, 255),
        (2, 254),
        (2, 255),
        (100, 3),
        (255, 1),
        (255, 2),
    ] {
        {
            let mut state = state.borrow_mut();
            state.rx_length = length;
            state.rx_offset = offset;
            state.rx_memory = core::array::from_fn(|i| i as u8);
            state.commands.clear();
        }
        let mut received = vec![0; usize::from(length)];
        let result = block_on(radio.read_received_frame(&mut received)).unwrap();
        assert_eq!(result.len, usize::from(length));
        assert_eq!(
            received,
            (0..length)
                .map(|i| offset.wrapping_add(i))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn trait_idle_and_receive_commands_reach_the_chip() {
    let (mut radio, state) = mock_radio();
    block_on(radio.initialize_profile(US915_AUTO_LORA_PROFILE.into())).unwrap();
    state.borrow_mut().commands.clear();
    block_on(LoRaRadio::idle(&mut radio)).unwrap();
    assert!(state
        .borrow()
        .commands
        .iter()
        .any(|command| command.starts_with(&opcode(op::SET_STANDBY))));
    state.borrow_mut().commands.clear();
    block_on(LoRaRadio::arm_rx(&mut radio)).unwrap();
    assert!(state
        .borrow()
        .commands
        .iter()
        .any(|command| command.starts_with(&opcode(op::SET_RX))));
}

#[cfg(feature = "lora-2g4")]
#[test]
fn high_frequency_power_accepts_the_exact_board_ceiling() {
    use prns_core::interfaces::lora::GHZ24_BALANCED_PROFILE;
    let mut config = board_for(Lr11xxPart::Lr1121);
    config.high_frequency = HighFrequencyPath::Regulated {
        maximum_power_dbm: 11,
    };
    let (radio, _) = mock_radio_for(config);
    let profile = GHZ24_BALANCED_PROFILE
        .with_tx_power(TxPower::new(11))
        .unwrap();
    assert_eq!(
        LoRaRadio::validate_band_profile(&radio, LoRaProfile::Ghz24(profile)),
        Ok(())
    );
}
