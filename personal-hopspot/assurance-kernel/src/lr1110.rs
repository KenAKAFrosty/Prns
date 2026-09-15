use core::cell::{Cell, RefCell};
use core::future::Future;
use core::task::{Context, Poll, Waker};

use embedded_hal::spi::{ErrorType as SpiErrorType, Operation};
use embedded_hal_async::spi::SpiDevice;
use prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;
use prns_interfaces_embassy::radios::lr1110::{
    BoardConfig, Error, HighPowerSelection, Lr1110, PowerAmplifierConfig, PowerAmplifierDutyCycle,
    PowerAmplifierSelection, PowerAmplifierSupply, PowerAmplifierTable, ReceiveGain,
    ReferenceClock, RegulatorMode, RfSwitchConfig, RfSwitchPins, TcxoStartupTime, TcxoVoltage,
    TransmitRampTime,
};
use prns_interfaces_embassy::radios::{LoRaRadio, RadioEvent, RadioRecovery};

use crate::test_support::{ControlledWait, MockDelay, MockError, MockOutput, ReadyWait, Trace};
use crate::transcript::EventKind;
use crate::{ScenarioError, Transcript};

const GET_VERSION: u16 = 0x0101;
const READ_BUFFER: u16 = 0x010a;
const GET_RX_BUFFER_STATUS: u16 = 0x0203;
const GET_PACKET_STATUS: u16 = 0x0204;
const GET_RSSI_INSTANTANEOUS: u16 = 0x0205;
const TX_DONE: u32 = 1 << 2;
const RX_DONE: u32 = 1 << 3;
const FRAME: &[u8] = b"PRNS-LR1110-SMOK";
const MAX_TRANSACTION_BYTES: usize = 260;
const MAX_COMMAND_BYTES: usize = 4;

const POWER_AMPLIFIER_CONFIGS: [PowerAmplifierConfig; 3] = [
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

struct PendingRead {
    command: [u8; MAX_COMMAND_BYTES],
    len: usize,
}

struct Device {
    irqs: [u32; 3],
    irq_index: usize,
    pending: Option<PendingRead>,
}

impl Device {
    const fn new() -> Self {
        Self {
            irqs: [0, TX_DONE, RX_DONE],
            irq_index: 0,
            pending: None,
        }
    }

    fn next_irq(&mut self) -> u32 {
        let status = self.irqs.get(self.irq_index).copied().unwrap_or(0);
        self.irq_index = self.irq_index.saturating_add(1);
        status
    }
}

struct MockSpi<'a> {
    trace: Trace<'a>,
    device: &'a RefCell<Device>,
}

impl SpiErrorType for MockSpi<'_> {
    type Error = MockError;
}

impl SpiDevice<u8> for MockSpi<'_> {
    async fn transaction(
        &mut self,
        operations: &mut [Operation<'_, u8>],
    ) -> Result<(), Self::Error> {
        let mut command = [0; MAX_TRANSACTION_BYTES];
        let mut command_len: usize = 0;
        for operation in operations.iter() {
            match operation {
                Operation::Write(bytes) => {
                    let Some(end) = command_len.checked_add(bytes.len()) else {
                        return Err(MockError::TransactionTooLong);
                    };
                    if end > command.len() {
                        return Err(MockError::TransactionTooLong);
                    }
                    command[command_len..end].copy_from_slice(bytes);
                    command_len = end;
                }
                Operation::TransferInPlace(_) => {}
                Operation::Read(_) | Operation::Transfer(_, _) | Operation::DelayNs(_) => {
                    return Err(MockError::UnsupportedOperation);
                }
            }
        }
        if command_len != 0 {
            self.trace
                .record(EventKind::SpiWrite, &command[..command_len])?;
        }

        let mut device = self.device.borrow_mut();
        for operation in operations.iter_mut() {
            match operation {
                Operation::TransferInPlace(buffer) => {
                    if buffer.iter().any(|byte| *byte != 0) {
                        return Err(MockError::InvalidReadBuffer);
                    }
                    if let Some(pending) = device.pending.take() {
                        fill_read(&pending.command[..pending.len], buffer);
                    } else {
                        fill_status(buffer, device.next_irq());
                    }
                    self.trace.record(EventKind::SpiRead, buffer)?;
                }
                Operation::Write(_) => {}
                Operation::Read(_) | Operation::Transfer(_, _) | Operation::DelayNs(_) => {
                    return Err(MockError::UnsupportedOperation);
                }
            }
        }

        if command_len >= 2 {
            let operation = u16::from_be_bytes([command[0], command[1]]);
            if matches!(
                operation,
                GET_VERSION
                    | GET_RX_BUFFER_STATUS
                    | GET_PACKET_STATUS
                    | GET_RSSI_INSTANTANEOUS
                    | READ_BUFFER
            ) {
                let len = command_len.min(MAX_COMMAND_BYTES);
                let mut pending = PendingRead {
                    command: [0; MAX_COMMAND_BYTES],
                    len,
                };
                pending.command[..len].copy_from_slice(&command[..len]);
                device.pending = Some(pending);
            }
        }
        Ok(())
    }
}

fn fill_status(buffer: &mut [u8], status: u32) {
    buffer.fill(0);
    if buffer.len() >= 6 {
        let flags = status.to_be_bytes();
        buffer[0] = 0x04;
        buffer[2..6].copy_from_slice(&flags);
    }
}

fn fill_read(command: &[u8], buffer: &mut [u8]) {
    buffer.fill(0);
    if command.len() < 2 {
        return;
    }
    match u16::from_be_bytes([command[0], command[1]]) {
        GET_VERSION if buffer.len() >= 4 => {
            buffer[..4].copy_from_slice(&[0x01, 0x01, 0x03, 0x08]);
        }
        GET_RX_BUFFER_STATUS if buffer.len() >= 2 => {
            buffer[..2].copy_from_slice(&[FRAME.len() as u8, 250]);
        }
        GET_PACKET_STATUS if buffer.len() >= 3 => {
            buffer[..3].copy_from_slice(&[181, 0xf7, 184]);
        }
        GET_RSSI_INSTANTANEOUS if !buffer.is_empty() => {
            buffer[0] = 172;
        }
        READ_BUFFER => {
            let offset = command.get(2).copied().unwrap_or(0) as usize;
            for (index, destination) in buffer.iter_mut().enumerate() {
                let address = (offset + index) % 256;
                let frame_index = (address + 6) % 256;
                *destination = FRAME.get(frame_index).copied().unwrap_or(0);
            }
        }
        _ => {}
    }
}

type Radio<'a> =
    Lr1110<MockSpi<'a>, ReadyWait<'a>, ControlledWait<'a>, MockOutput<'a>, MockDelay<'a>>;

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
        power_amplifier: PowerAmplifierTable::new(20, &POWER_AMPLIFIER_CONFIGS),
        transmit_ramp_time: TransmitRampTime::Us48,
        external_receive_gain_db: 0,
    }
}

pub async fn run(transcript: &RefCell<Transcript>) -> Result<(), ScenarioError> {
    let trace = Trace::new(transcript);
    trace
        .record(EventKind::Scenario, b"lr1110")
        .map_err(|_| ScenarioError::TranscriptFull)?;
    let device = RefCell::new(Device::new());
    let high_ready = Cell::new(true);
    let mut radio: Radio<'_> = Lr1110::new(
        MockSpi {
            trace,
            device: &device,
        },
        ReadyWait::new(trace, 1),
        ControlledWait::new(trace, &high_ready),
        MockOutput::new(trace),
        MockDelay::new(trace),
        board(),
    );

    radio
        .initialize(US915_AUTO_LORA_PROFILE)
        .await
        .map_err(ScenarioError::Lr1110)?;
    trace
        .record(EventKind::Result, &[1])
        .map_err(|_| ScenarioError::TranscriptFull)?;
    radio
        .transmit(b"lr1110-target")
        .await
        .map_err(ScenarioError::Lr1110)?;
    trace
        .record(EventKind::Result, &[2])
        .map_err(|_| ScenarioError::TranscriptFull)?;

    high_ready.set(false);
    let timeout = match radio.transmit(b"lr1110-timeout").await {
        Err(error @ Error::Timeout) => error,
        _ => return Err(ScenarioError::UnexpectedResult),
    };
    if <Radio<'_> as LoRaRadio>::recovery(&timeout) != RadioRecovery::Reinitialize {
        return Err(ScenarioError::UnexpectedResult);
    }
    trace
        .record(EventKind::Recovery, &[1])
        .map_err(|_| ScenarioError::TranscriptFull)?;

    radio.arm_rx().await.map_err(ScenarioError::Lr1110)?;
    let mut buffer = [0; 255];
    {
        let mut receive = core::pin::pin!(radio.read_event(&mut buffer));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        if !matches!(receive.as_mut().poll(&mut context), Poll::Pending) {
            return Err(ScenarioError::UnexpectedResult);
        }
    }
    trace
        .record(EventKind::Poll, &[0])
        .map_err(|_| ScenarioError::TranscriptFull)?;

    let event = radio
        .poll_event(&mut buffer)
        .await
        .map_err(ScenarioError::Lr1110)?;
    let Some(RadioEvent::Frame(frame)) = event else {
        return Err(ScenarioError::UnexpectedResult);
    };
    if frame.len != FRAME.len() || &buffer[..frame.len] != FRAME {
        return Err(ScenarioError::UnexpectedResult);
    }
    trace
        .record(EventKind::Result, &buffer[..frame.len])
        .map_err(|_| ScenarioError::TranscriptFull)?;
    trace
        .record(EventKind::Complete, b"lr1110")
        .map_err(|_| ScenarioError::TranscriptFull)
}
