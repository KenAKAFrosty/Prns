use embassy_time::{with_timeout, Delay, Duration, Instant, Timer};
use embedded_hal_async::spi::SpiDevice;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    gpio::{Input, Output},
    spi::master::Spi,
    Async,
};
use personal_hopspot_core as screen;

use crate::heltec_e290_ssd1680::{ControllerPacking, FRAME_BYTES};

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

const POWER_SETTLE_MS: u64 = 10;
const RESET_PULSE_US: u64 = 200;
const CONTROL_BUSY_TIMEOUT_MS: u64 = 2_000;
const FULL_REFRESH_BUSY_TIMEOUT_MS: u64 = 10_000;
const TRANSFER_CHUNK_BYTES: usize = 64;

pub(crate) type DisplaySpi = ExclusiveDevice<Spi<'static, Async>, Output<'static>, Delay>;

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
pub(crate) enum E290DisplayError {
    Unavailable,
    UnexpectedRefreshKind,
    Spi(DisplayPhase),
    BusyTimeout(DisplayPhase),
}

struct Controller {
    spi: DisplaySpi,
    data_command: Output<'static>,
    reset: Output<'static>,
    busy: Input<'static>,
    packing: ControllerPacking,
}

impl Controller {
    fn new(
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
    ) -> Result<(), E290DisplayError> {
        self.data_command.set_low();
        self.spi
            .write(&[command])
            .await
            .map_err(|_| E290DisplayError::Spi(phase))
    }

    async fn write_data(
        &mut self,
        phase: DisplayPhase,
        data: &[u8],
    ) -> Result<(), E290DisplayError> {
        self.data_command.set_high();
        self.spi
            .write(data)
            .await
            .map_err(|_| E290DisplayError::Spi(phase))
    }

    async fn write_command_data(
        &mut self,
        phase: DisplayPhase,
        command: u8,
        data: &[u8],
    ) -> Result<(), E290DisplayError> {
        self.write_command(phase, command).await?;
        self.write_data(phase, data).await
    }

    async fn wait_until_idle(
        &mut self,
        phase: DisplayPhase,
        timeout_ms: u64,
    ) -> Result<(), E290DisplayError> {
        match with_timeout(Duration::from_millis(timeout_ms), self.busy.wait_for_low()).await {
            Ok(()) => Ok(()),
            Err(_) => Err(E290DisplayError::BusyTimeout(phase)),
        }
    }

    async fn initialize(&mut self) -> Result<(), E290DisplayError> {
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

        // The panel is native portrait (128x296). X is its sixteen-byte row and Y is the
        // landscape column, so X-then-Y increment produces `landscape_x * 16 + y / 8`.
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

    async fn stream_frame(
        &mut self,
        frame: &screen::face_64x128::Frame,
    ) -> Result<(), E290DisplayError> {
        self.wait_until_idle(DisplayPhase::RamWriteReady, CONTROL_BUSY_TIMEOUT_MS)
            .await?;
        self.write_command(DisplayPhase::RamWrite, WRITE_BLACK_WHITE_RAM)
            .await?;
        let mut staging = [0u8; TRANSFER_CHUNK_BYTES];
        for offset in (0..FRAME_BYTES).step_by(TRANSFER_CHUNK_BYTES) {
            self.packing.fill(frame, offset, &mut staging);
            self.write_data(DisplayPhase::RamWrite, &staging).await?;
        }
        Ok(())
    }

    async fn activate(&mut self) -> Result<(), E290DisplayError> {
        self.write_command_data(DisplayPhase::FullRefresh, DISPLAY_UPDATE_CONTROL_2, &[0xf7])
            .await?;
        self.write_command(DisplayPhase::FullRefresh, MASTER_ACTIVATION)
            .await?;
        self.wait_until_idle(DisplayPhase::FullRefresh, FULL_REFRESH_BUSY_TIMEOUT_MS)
            .await
    }

    async fn deep_sleep(&mut self) -> Result<(), E290DisplayError> {
        self.wait_until_idle(DisplayPhase::DeepSleep, CONTROL_BUSY_TIMEOUT_MS)
            .await?;
        self.write_command_data(DisplayPhase::DeepSleep, DEEP_SLEEP, &[0x01])
            .await
    }

    fn assert_reset(&mut self) {
        self.reset.set_low();
    }
}

/// Sole E290 panel owner. It retains no physical framebuffer; each operation streams bytes from
/// the exact logical candidate and leaves the controller reset and rail off afterward.
pub(crate) struct E290Display {
    controller: Option<Controller>,
    power: Output<'static>,
}

impl E290Display {
    pub(crate) fn new(
        spi: Option<DisplaySpi>,
        data_command: Output<'static>,
        reset: Output<'static>,
        busy: Input<'static>,
        power: Output<'static>,
    ) -> Self {
        Self {
            controller: spi.map(|spi| Controller::new(spi, data_command, reset, busy)),
            power,
        }
    }

    pub(crate) const fn is_available(&self) -> bool {
        self.controller.is_some()
    }

    pub(crate) async fn present(
        &mut self,
        frame: &screen::face_64x128::Frame,
        kind: screen::presentation::RefreshKind,
    ) -> Result<(), E290DisplayError> {
        if kind != screen::presentation::RefreshKind::RetainedFullWaveform {
            return Err(E290DisplayError::UnexpectedRefreshKind);
        }
        let controller = self
            .controller
            .as_mut()
            .ok_or(E290DisplayError::Unavailable)?;
        let started_at_ms = Instant::now().as_millis();
        log::info!("E290 display refresh begin t_ms={started_at_ms} kind={kind:?}");
        self.power.set_high();
        Timer::after(Duration::from_millis(POWER_SETTLE_MS)).await;

        let result = async {
            controller.initialize().await?;
            controller.stream_frame(frame).await?;
            controller.activate().await?;
            controller.deep_sleep().await
        }
        .await;
        controller.assert_reset();
        self.power.set_low();
        let completed_at_ms = Instant::now().as_millis();
        log::info!(
            "E290 display refresh complete t_ms={completed_at_ms} elapsed_ms={} result={result:?}",
            completed_at_ms.saturating_sub(started_at_ms)
        );
        result
    }
}
