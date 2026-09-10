use core::cell::{Cell, RefCell};
use core::future::Future;
use core::task::{Context, Poll, Waker};

use embassy_futures::block_on;
use embedded_hal::spi::{ErrorType as SpiErrorType, Operation};
use embedded_hal_async::spi::SpiDevice;
use prns_core::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;
use prns_interfaces_embassy::radios::sx126x::{
    BoardConfig, Error, FrontendControl, Sx126x, TcxoVoltage,
};
use prns_interfaces_embassy::radios::{LoRaRadio, RadioEvent, RadioRecovery};

use crate::test_support::{ControlledWait, MockDelay, MockError, MockOutput, ReadyWait, Trace};
use crate::transcript::EventKind;
use crate::{ScenarioError, Transcript};

const GET_IRQ_STATUS: u8 = 0x12;
const GET_RX_BUFFER_STATUS: u8 = 0x13;
const GET_PACKET_STATUS: u8 = 0x14;
const GET_RSSI_INSTANTANEOUS: u8 = 0x15;
const READ_BUFFER: u8 = 0x1e;
const TX_DONE: u16 = 1 << 0;
const RX_DONE: u16 = 1 << 1;
const FRAME: &[u8] = b"PRNS-HELTEC-SMOK";
const MAX_TRANSACTION_BYTES: usize = 260;

struct Device {
    irqs: [u16; 2],
    irq_index: usize,
}

impl Device {
    const fn new() -> Self {
        Self {
            irqs: [TX_DONE, RX_DONE],
            irq_index: 0,
        }
    }

    fn next_irq(&mut self) -> u16 {
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
                Operation::Read(_) => {}
                Operation::Transfer(_, _)
                | Operation::TransferInPlace(_)
                | Operation::DelayNs(_) => return Err(MockError::UnsupportedOperation),
            }
        }
        if command_len != 0 {
            self.trace
                .record(EventKind::SpiWrite, &command[..command_len])?;
        }
        for operation in operations.iter_mut() {
            match operation {
                Operation::Read(buffer) => {
                    if command_len == 0 {
                        return Err(MockError::UnsupportedOperation);
                    }
                    fill_read(
                        &command[..command_len],
                        buffer,
                        &mut self.device.borrow_mut(),
                    );
                    self.trace.record(EventKind::SpiRead, buffer)?;
                }
                Operation::Write(_) => {}
                Operation::Transfer(_, _)
                | Operation::TransferInPlace(_)
                | Operation::DelayNs(_) => return Err(MockError::UnsupportedOperation),
            }
        }
        Ok(())
    }
}

fn fill_read(command: &[u8], buffer: &mut [u8], device: &mut Device) {
    buffer.fill(0);
    match command.first().copied().unwrap_or(0) {
        GET_IRQ_STATUS if buffer.len() >= 3 => {
            let status = device.next_irq().to_be_bytes();
            buffer[1] = status[0];
            buffer[2] = status[1];
        }
        GET_RX_BUFFER_STATUS if buffer.len() >= 3 => {
            buffer[1] = FRAME.len() as u8;
        }
        GET_PACKET_STATUS if buffer.len() >= 4 => {
            buffer[1..4].copy_from_slice(&[181, 0xf7, 184]);
        }
        GET_RSSI_INSTANTANEOUS if buffer.len() >= 2 => {
            buffer[1] = 172;
        }
        READ_BUFFER => {
            for (destination, source) in buffer.iter_mut().zip(FRAME.iter().copied()) {
                *destination = source;
            }
        }
        _ => {}
    }
}

type Radio<'a> =
    Sx126x<MockSpi<'a>, ReadyWait<'a>, ControlledWait<'a>, MockOutput<'a>, MockDelay<'a>>;

fn board() -> BoardConfig {
    BoardConfig {
        tcxo_voltage: Some(TcxoVoltage::V1_8),
        use_dcdc: true,
        rx_boost: true,
        dio2_as_rf_switch: true,
        external_rx_gain_db: 0,
        external_power_amplifier: None,
        frontend_control: FrontendControl::NoDynamicControl,
    }
}

pub fn run(transcript: &RefCell<Transcript>) -> Result<(), ScenarioError> {
    let trace = Trace::new(transcript);
    trace
        .record(EventKind::Scenario, b"sx126x")
        .map_err(|_| ScenarioError::TranscriptFull)?;
    let device = RefCell::new(Device::new());
    let high_ready = Cell::new(true);
    let mut radio: Radio<'_> = Sx126x::new(
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

    block_on(radio.initialize(US915_AUTO_LORA_PROFILE)).map_err(ScenarioError::Sx126x)?;
    trace
        .record(EventKind::Result, &[1])
        .map_err(|_| ScenarioError::TranscriptFull)?;
    block_on(radio.transmit(b"sx126x-target")).map_err(ScenarioError::Sx126x)?;
    trace
        .record(EventKind::Result, &[2])
        .map_err(|_| ScenarioError::TranscriptFull)?;

    high_ready.set(false);
    let timeout = match block_on(radio.transmit(b"sx126x-timeout")) {
        Err(error @ Error::Timeout) => error,
        _ => return Err(ScenarioError::UnexpectedResult),
    };
    if <Radio<'_> as LoRaRadio>::recovery(&timeout) != RadioRecovery::Reinitialize {
        return Err(ScenarioError::UnexpectedResult);
    }
    trace
        .record(EventKind::Recovery, &[1])
        .map_err(|_| ScenarioError::TranscriptFull)?;

    block_on(radio.arm_rx()).map_err(ScenarioError::Sx126x)?;
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

    let event = block_on(radio.poll_event(&mut buffer)).map_err(ScenarioError::Sx126x)?;
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
        .record(EventKind::Complete, b"sx126x")
        .map_err(|_| ScenarioError::TranscriptFull)
}
