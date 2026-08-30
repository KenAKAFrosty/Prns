use embassy_time::{with_timeout, Duration, Timer};
use embedded_hal_async::spi::SpiDevice;
use esp_hal::gpio::{Input, Output};
use personal_hopspot_core::face_64x128::Frame;

use crate::heltec_e290_ssd1680::{ControllerPacking, PackingError, FRAME_BYTES};

use super::DisplaySpi;

const SOFTWARE_RESET: u8 = 0x12;
const DRIVER_OUTPUT_CONTROL: u8 = 0x01;
const DATA_ENTRY_MODE: u8 = 0x11;
const RAM_X_WINDOW: u8 = 0x44;
const RAM_Y_WINDOW: u8 = 0x45;
const BORDER_WAVEFORM: u8 = 0x3c;
const DISPLAY_UPDATE_CONTROL_1: u8 = 0x21;
const RAM_X_COUNTER: u8 = 0x4e;
const RAM_Y_COUNTER: u8 = 0x4f;
const WRITE_BLACK_WHITE_RAM: u8 = 0x24;
const DISPLAY_UPDATE_CONTROL_2: u8 = 0x22;
const MASTER_ACTIVATION: u8 = 0x20;
const DEEP_SLEEP: u8 = 0x10;

const RESET_PULSE_US: u64 = 200;
const CONTROL_BUSY_TIMEOUT_MS: u64 = 2_000;
const FULL_REFRESH_BUSY_TIMEOUT_MS: u64 = 10_000;
const TRANSFER_CHUNK_BYTES: usize = 64;
const _: () = assert!(FRAME_BYTES.is_multiple_of(TRANSFER_CHUNK_BYTES));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplayPhase {
    ResetRelease,
    SoftwareReset,
    RamWriteReady,
    RamWrite,
    FullRefresh,
    DeepSleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ControllerError {
    Spi(DisplayPhase),
    BusyTimeout(DisplayPhase),
    Packing(PackingError),
}

pub(super) struct Controller {
    spi: DisplaySpi,
    data_command: Output<'static>,
    reset: Output<'static>,
    busy: Input<'static>,
    packing: ControllerPacking,
}

impl Controller {
    pub(super) const fn new(
        spi: DisplaySpi,
        data_command: Output<'static>,
        reset: Output<'static>,
        busy: Input<'static>,
    ) -> Self {
        Self {
            spi,
            data_command,
            reset,
            busy,
            packing: ControllerPacking::front_facing(),
        }
    }

    async fn write_command(
        &mut self,
        phase: DisplayPhase,
        command: u8,
    ) -> Result<(), ControllerError> {
        self.data_command.set_low();
        self.spi
            .write(&[command])
            .await
            .map_err(|_| ControllerError::Spi(phase))
    }

    async fn write_data(
        &mut self,
        phase: DisplayPhase,
        data: &[u8],
    ) -> Result<(), ControllerError> {
        self.data_command.set_high();
        self.spi
            .write(data)
            .await
            .map_err(|_| ControllerError::Spi(phase))
    }

    async fn write_command_data(
        &mut self,
        phase: DisplayPhase,
        command: u8,
        data: &[u8],
    ) -> Result<(), ControllerError> {
        self.write_command(phase, command).await?;
        self.write_data(phase, data).await
    }

    async fn wait_until_idle(
        &mut self,
        phase: DisplayPhase,
        timeout_ms: u64,
    ) -> Result<(), ControllerError> {
        with_timeout(Duration::from_millis(timeout_ms), self.busy.wait_for_low())
            .await
            .map_err(|_| ControllerError::BusyTimeout(phase))
    }

    pub(super) async fn initialize(&mut self) -> Result<(), ControllerError> {
        self.reset.set_low();
        Timer::after(Duration::from_micros(RESET_PULSE_US)).await;
        self.reset.set_high();
        Timer::after(Duration::from_micros(RESET_PULSE_US)).await;
        self.wait_until_idle(DisplayPhase::ResetRelease, CONTROL_BUSY_TIMEOUT_MS)
            .await?;

        self.write_command(DisplayPhase::SoftwareReset, SOFTWARE_RESET)
            .await?;
        self.wait_until_idle(DisplayPhase::SoftwareReset, CONTROL_BUSY_TIMEOUT_MS)
            .await?;
        // Native portrait RAM stores sixteen vertical bytes for each landscape column.
        self.write_command_data(
            DisplayPhase::SoftwareReset,
            DRIVER_OUTPUT_CONTROL,
            &[0x27, 0x01, 0x00],
        )
        .await?;
        self.write_command_data(DisplayPhase::SoftwareReset, DATA_ENTRY_MODE, &[0x03])
            .await?;
        self.write_command_data(DisplayPhase::SoftwareReset, RAM_X_WINDOW, &[0x00, 0x0f])
            .await?;
        self.write_command_data(
            DisplayPhase::SoftwareReset,
            RAM_Y_WINDOW,
            &[0x00, 0x00, 0x27, 0x01],
        )
        .await?;
        self.write_command_data(DisplayPhase::SoftwareReset, BORDER_WAVEFORM, &[0x05])
            .await?;
        self.write_command_data(
            DisplayPhase::SoftwareReset,
            DISPLAY_UPDATE_CONTROL_1,
            &[0x00, 0x80],
        )
        .await?;
        self.write_command_data(DisplayPhase::SoftwareReset, RAM_X_COUNTER, &[0x00])
            .await?;
        self.write_command_data(DisplayPhase::SoftwareReset, RAM_Y_COUNTER, &[0x00, 0x00])
            .await
    }

    pub(super) async fn stream_frame(&mut self, frame: &Frame) -> Result<(), ControllerError> {
        self.wait_until_idle(DisplayPhase::RamWriteReady, CONTROL_BUSY_TIMEOUT_MS)
            .await?;
        self.write_command(DisplayPhase::RamWrite, WRITE_BLACK_WHITE_RAM)
            .await?;
        let mut staging = [0u8; TRANSFER_CHUNK_BYTES];
        for offset in (0..FRAME_BYTES).step_by(TRANSFER_CHUNK_BYTES) {
            self.packing
                .fill(frame, offset, &mut staging)
                .map_err(ControllerError::Packing)?;
            self.write_data(DisplayPhase::RamWrite, &staging).await?;
        }
        Ok(())
    }

    pub(super) async fn activate(&mut self) -> Result<(), ControllerError> {
        self.write_command_data(DisplayPhase::FullRefresh, DISPLAY_UPDATE_CONTROL_2, &[0xf7])
            .await?;
        self.write_command(DisplayPhase::FullRefresh, MASTER_ACTIVATION)
            .await?;
        self.wait_until_idle(DisplayPhase::FullRefresh, FULL_REFRESH_BUSY_TIMEOUT_MS)
            .await
    }

    pub(super) async fn deep_sleep(&mut self) -> Result<(), ControllerError> {
        self.wait_until_idle(DisplayPhase::DeepSleep, CONTROL_BUSY_TIMEOUT_MS)
            .await?;
        self.write_command_data(DisplayPhase::DeepSleep, DEEP_SLEEP, &[0x01])
            .await
    }

    pub(super) fn assert_reset(&mut self) {
        self.reset.set_low();
    }
}
