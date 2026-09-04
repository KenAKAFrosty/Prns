use embassy_time::{Duration, Instant, Timer};
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal_async::spi::SpiDevice;

use super::raster::ROW_BYTES;

const HEIGHT: u32 = 250;
const RAM_X_OFFSET: u8 = 1;
const SW_RESET: u8 = 0x12;
const DRIVER_OUTPUT_CONTROL: u8 = 0x01;
const DATA_ENTRY_MODE: u8 = 0x11;
const DEEP_SLEEP: u8 = 0x10;
const MASTER_ACTIVATION: u8 = 0x20;
const DISPLAY_UPDATE_CONTROL_2: u8 = 0x22;
const WRITE_RAM_BW: u8 = 0x24;
const WRITE_RAM_PREV: u8 = 0x26;
const BORDER_WAVEFORM_CONTROL: u8 = 0x3c;
const SET_RAM_X_START_END: u8 = 0x44;
const SET_RAM_Y_START_END: u8 = 0x45;
const SET_RAM_X_COUNTER: u8 = 0x4e;
const SET_RAM_Y_COUNTER: u8 = 0x4f;
const TERMINATE_FRAME_WRITE: u8 = 0x7f;
const SEQUENCE_FULL: u8 = 0xf7;
const SEQUENCE_FAST: u8 = 0xff;
const FAST_BORDER: u8 = 0x85;
const RESET_EDGE_DELAY: Duration = Duration::from_millis(10);
const BUSY_SAMPLE_INTERVAL: Duration = Duration::from_millis(5);
const LONG_BUSY_TIMEOUT: Duration = Duration::from_secs(10);
const FAST_BUSY_TIMEOUT: Duration = Duration::from_millis(2_500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerOperation {
    Initialize,
    FullRefresh,
    FastRefresh,
    DeepSleep,
    Recover,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ssd1680Error {
    ResetPin(ControllerOperation),
    BusyPin(ControllerOperation),
    BusyTimeout(ControllerOperation),
    ControlPin(ControllerOperation),
    Transfer(ControllerOperation),
}

enum RefreshMode {
    Full,
    Fast,
}

pub struct Ssd1680<SPI, BUSY, DC, RST> {
    spi: SPI,
    busy: BUSY,
    dc: DC,
    reset: RST,
}

impl<SPI, BUSY, DC, RST> Ssd1680<SPI, BUSY, DC, RST>
where
    SPI: SpiDevice,
    BUSY: InputPin,
    DC: OutputPin,
    RST: OutputPin,
{
    pub const fn new(spi: SPI, busy: BUSY, dc: DC, reset: RST) -> Self {
        Self {
            spi,
            busy,
            dc,
            reset,
        }
    }

    pub async fn initialize(&mut self) -> Result<(), Ssd1680Error> {
        self.prepare(ControllerOperation::Initialize, RefreshMode::Full)
            .await
    }

    pub async fn recover(&mut self) -> Result<(), Ssd1680Error> {
        self.prepare(ControllerOperation::Recover, RefreshMode::Full)
            .await
    }

    pub async fn full_update<F>(&mut self, rows: &mut F) -> Result<(), Ssd1680Error>
    where
        F: FnMut(u32) -> [u8; ROW_BYTES],
    {
        let operation = ControllerOperation::FullRefresh;
        self.prepare(operation, RefreshMode::Full).await?;
        self.write_ram(WRITE_RAM_BW, rows, operation).await?;
        self.write_ram(WRITE_RAM_PREV, rows, operation).await?;
        self.run_sequence(SEQUENCE_FULL, operation, LONG_BUSY_TIMEOUT)
            .await
    }

    pub async fn partial_update<F>(&mut self, rows: &mut F) -> Result<(), Ssd1680Error>
    where
        F: FnMut(u32) -> [u8; ROW_BYTES],
    {
        let operation = ControllerOperation::FastRefresh;
        self.prepare(operation, RefreshMode::Fast).await?;
        self.write_ram(WRITE_RAM_BW, rows, operation).await?;
        self.run_sequence(SEQUENCE_FAST, operation, FAST_BUSY_TIMEOUT)
            .await?;
        self.write_ram(WRITE_RAM_BW, rows, operation).await?;
        self.write_ram(WRITE_RAM_PREV, rows, operation).await?;
        self.command(TERMINATE_FRAME_WRITE, operation).await?;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await
    }

    pub async fn deep_sleep(&mut self) -> Result<(), Ssd1680Error> {
        let operation = ControllerOperation::DeepSleep;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.command_data(DEEP_SLEEP, &[0x01], operation).await
    }

    async fn prepare(
        &mut self,
        operation: ControllerOperation,
        refresh: RefreshMode,
    ) -> Result<(), Ssd1680Error> {
        self.reset(operation).await?;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.command(SW_RESET, operation).await?;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.command_data(DRIVER_OUTPUT_CONTROL, &[0xf9, 0x00, 0x00], operation)
            .await?;
        if matches!(refresh, RefreshMode::Fast) {
            self.command_data(BORDER_WAVEFORM_CONTROL, &[FAST_BORDER], operation)
                .await?;
        }
        self.set_ram_window(operation).await?;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await
    }

    async fn write_ram<F>(
        &mut self,
        ram: u8,
        rows: &mut F,
        operation: ControllerOperation,
    ) -> Result<(), Ssd1680Error>
    where
        F: FnMut(u32) -> [u8; ROW_BYTES],
    {
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.set_ram_window(operation).await?;
        self.command(ram, operation).await?;
        self.dc
            .set_high()
            .map_err(|_| Ssd1680Error::ControlPin(operation))?;
        for y in 0..HEIGHT {
            self.spi
                .write(&rows(y))
                .await
                .map_err(|_| Ssd1680Error::Transfer(operation))?;
        }
        Ok(())
    }

    async fn run_sequence(
        &mut self,
        sequence: u8,
        operation: ControllerOperation,
        timeout: Duration,
    ) -> Result<(), Ssd1680Error> {
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.command_data(DISPLAY_UPDATE_CONTROL_2, &[sequence], operation)
            .await?;
        self.command(MASTER_ACTIVATION, operation).await?;
        self.wait_idle(operation, timeout).await
    }

    async fn set_ram_window(&mut self, operation: ControllerOperation) -> Result<(), Ssd1680Error> {
        let x_end = RAM_X_OFFSET + ROW_BYTES as u8 - 1;
        self.command_data(DATA_ENTRY_MODE, &[0x03], operation)
            .await?;
        self.command_data(SET_RAM_X_START_END, &[RAM_X_OFFSET, x_end], operation)
            .await?;
        self.command_data(
            SET_RAM_Y_START_END,
            &[0x00, 0x00, (HEIGHT - 1) as u8, ((HEIGHT - 1) >> 8) as u8],
            operation,
        )
        .await?;
        self.command_data(SET_RAM_X_COUNTER, &[RAM_X_OFFSET], operation)
            .await?;
        self.command_data(SET_RAM_Y_COUNTER, &[0x00, 0x00], operation)
            .await
    }

    async fn command(
        &mut self,
        command: u8,
        operation: ControllerOperation,
    ) -> Result<(), Ssd1680Error> {
        self.dc
            .set_low()
            .map_err(|_| Ssd1680Error::ControlPin(operation))?;
        self.spi
            .write(&[command])
            .await
            .map_err(|_| Ssd1680Error::Transfer(operation))
    }

    async fn command_data(
        &mut self,
        command: u8,
        data: &[u8],
        operation: ControllerOperation,
    ) -> Result<(), Ssd1680Error> {
        self.command(command, operation).await?;
        self.dc
            .set_high()
            .map_err(|_| Ssd1680Error::ControlPin(operation))?;
        self.spi
            .write(data)
            .await
            .map_err(|_| Ssd1680Error::Transfer(operation))
    }

    async fn reset(&mut self, operation: ControllerOperation) -> Result<(), Ssd1680Error> {
        self.reset
            .set_low()
            .map_err(|_| Ssd1680Error::ResetPin(operation))?;
        Timer::after(RESET_EDGE_DELAY).await;
        self.reset
            .set_high()
            .map_err(|_| Ssd1680Error::ResetPin(operation))?;
        Timer::after(RESET_EDGE_DELAY).await;
        Ok(())
    }

    async fn wait_idle(
        &mut self,
        operation: ControllerOperation,
        timeout: Duration,
    ) -> Result<(), Ssd1680Error> {
        let deadline = Instant::now() + timeout;
        loop {
            let busy = self
                .busy
                .is_high()
                .map_err(|_| Ssd1680Error::BusyPin(operation))?;
            if !busy {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Ssd1680Error::BusyTimeout(operation));
            }
            Timer::after(BUSY_SAMPLE_INTERVAL).await;
        }
    }
}
