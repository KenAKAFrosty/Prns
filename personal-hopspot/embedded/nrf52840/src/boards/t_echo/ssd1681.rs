use embassy_time::{Duration, Instant, Timer};
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal_async::spi::SpiDevice;

use super::raster::ROW_BYTES;

pub const WIDTH: u32 = 200;
pub const HEIGHT: u32 = 200;

const SW_RESET: u8 = 0x12;
const DRIVER_OUTPUT_CONTROL: u8 = 0x01;
const DATA_ENTRY_MODE: u8 = 0x11;
const DEEP_SLEEP: u8 = 0x10;
const TEMP_SENSOR_SELECTION: u8 = 0x18;
const MASTER_ACTIVATION: u8 = 0x20;
const DISPLAY_UPDATE_CONTROL_2: u8 = 0x22;
const WRITE_RAM_BW: u8 = 0x24;
const WRITE_RAM_PREV: u8 = 0x26;
const BORDER_WAVEFORM_CONTROL: u8 = 0x3c;
const SET_RAM_X_START_END: u8 = 0x44;
const SET_RAM_Y_START_END: u8 = 0x45;
const SET_RAM_X_COUNTER: u8 = 0x4e;
const SET_RAM_Y_COUNTER: u8 = 0x4f;

const SEQUENCE_FULL: u8 = 0xf7;
const SEQUENCE_PARTIAL: u8 = 0xfc;
const BORDER_FOLLOW_LUT: u8 = 0x05;

const RESET_EDGE_DELAY: Duration = Duration::from_millis(10);
const RESET_RECOVERY_DELAY: Duration = Duration::from_millis(200);
const BUSY_SAMPLE_INTERVAL: Duration = Duration::from_millis(5);
const LONG_BUSY_TIMEOUT: Duration = Duration::from_secs(10);
const PARTIAL_BUSY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerOperation {
    Initialize,
    FullRefresh,
    PartialRefresh,
    DeepSleep,
    Recover,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ssd1681Error {
    ResetPin(ControllerOperation),
    BusyPin(ControllerOperation),
    BusyTimeout(ControllerOperation),
    ControlPin(ControllerOperation),
    Transfer(ControllerOperation),
}

pub struct Ssd1681<SPI, BUSY, DC, RST> {
    spi: SPI,
    busy: BUSY,
    dc: DC,
    rst: RST,
}

impl<SPI, BUSY, DC, RST> Ssd1681<SPI, BUSY, DC, RST>
where
    SPI: SpiDevice,
    BUSY: InputPin,
    DC: OutputPin,
    RST: OutputPin,
{
    pub const fn new(spi: SPI, busy: BUSY, dc: DC, rst: RST) -> Self {
        Self { spi, busy, dc, rst }
    }

    pub async fn initialize(&mut self) -> Result<(), Ssd1681Error> {
        self.reset_and_configure(ControllerOperation::Initialize)
            .await
    }

    pub async fn recover(&mut self) -> Result<(), Ssd1681Error> {
        self.reset_and_configure(ControllerOperation::Recover).await
    }

    pub async fn full_update<F>(&mut self, rows: &mut F) -> Result<(), Ssd1681Error>
    where
        F: FnMut(u32) -> [u8; ROW_BYTES],
    {
        let operation = ControllerOperation::FullRefresh;
        self.write_ram(WRITE_RAM_BW, rows, operation).await?;
        self.write_ram(WRITE_RAM_PREV, rows, operation).await?;
        self.run_sequence(SEQUENCE_FULL, operation, LONG_BUSY_TIMEOUT)
            .await
    }

    pub async fn partial_update<F>(&mut self, rows: &mut F) -> Result<(), Ssd1681Error>
    where
        F: FnMut(u32) -> [u8; ROW_BYTES],
    {
        let operation = ControllerOperation::PartialRefresh;
        self.write_ram(WRITE_RAM_BW, rows, operation).await?;
        self.run_sequence(SEQUENCE_PARTIAL, operation, PARTIAL_BUSY_TIMEOUT)
            .await?;
        self.write_ram(WRITE_RAM_PREV, rows, operation).await
    }

    pub async fn deep_sleep(&mut self) -> Result<(), Ssd1681Error> {
        let operation = ControllerOperation::DeepSleep;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.cmd_data(DEEP_SLEEP, &[0x01], operation).await
    }

    async fn reset_and_configure(
        &mut self,
        operation: ControllerOperation,
    ) -> Result<(), Ssd1681Error> {
        self.reset(operation).await?;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.cmd(SW_RESET, operation).await?;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.cmd_data(
            DRIVER_OUTPUT_CONTROL,
            &[(HEIGHT - 1) as u8, ((HEIGHT - 1) >> 8) as u8, 0x00],
            operation,
        )
        .await?;
        self.cmd_data(BORDER_WAVEFORM_CONTROL, &[BORDER_FOLLOW_LUT], operation)
            .await?;
        self.cmd_data(TEMP_SENSOR_SELECTION, &[0x80], operation)
            .await?;
        self.set_ram_window(operation).await?;
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await
    }

    async fn write_ram<F>(
        &mut self,
        ram: u8,
        rows: &mut F,
        operation: ControllerOperation,
    ) -> Result<(), Ssd1681Error>
    where
        F: FnMut(u32) -> [u8; ROW_BYTES],
    {
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.set_ram_window(operation).await?;
        self.cmd(ram, operation).await?;
        self.dc
            .set_high()
            .map_err(|_| Ssd1681Error::ControlPin(operation))?;
        for y in 0..HEIGHT {
            let row = rows(y);
            self.spi
                .write(&row)
                .await
                .map_err(|_| Ssd1681Error::Transfer(operation))?;
        }
        Ok(())
    }

    async fn run_sequence(
        &mut self,
        sequence: u8,
        operation: ControllerOperation,
        completion_timeout: Duration,
    ) -> Result<(), Ssd1681Error> {
        self.wait_idle(operation, LONG_BUSY_TIMEOUT).await?;
        self.cmd_data(DISPLAY_UPDATE_CONTROL_2, &[sequence], operation)
            .await?;
        self.cmd(MASTER_ACTIVATION, operation).await?;
        self.wait_idle(operation, completion_timeout).await
    }

    async fn set_ram_window(&mut self, operation: ControllerOperation) -> Result<(), Ssd1681Error> {
        self.cmd_data(DATA_ENTRY_MODE, &[0x03], operation).await?;
        self.cmd_data(
            SET_RAM_X_START_END,
            &[0x00, ((WIDTH - 1) >> 3) as u8],
            operation,
        )
        .await?;
        self.cmd_data(
            SET_RAM_Y_START_END,
            &[0x00, 0x00, (HEIGHT - 1) as u8, ((HEIGHT - 1) >> 8) as u8],
            operation,
        )
        .await?;
        self.cmd_data(SET_RAM_X_COUNTER, &[0x00], operation).await?;
        self.cmd_data(SET_RAM_Y_COUNTER, &[0x00, 0x00], operation)
            .await
    }

    async fn cmd(
        &mut self,
        command: u8,
        operation: ControllerOperation,
    ) -> Result<(), Ssd1681Error> {
        self.dc
            .set_low()
            .map_err(|_| Ssd1681Error::ControlPin(operation))?;
        self.spi
            .write(&[command])
            .await
            .map_err(|_| Ssd1681Error::Transfer(operation))
    }

    async fn cmd_data(
        &mut self,
        command: u8,
        data: &[u8],
        operation: ControllerOperation,
    ) -> Result<(), Ssd1681Error> {
        self.cmd(command, operation).await?;
        self.dc
            .set_high()
            .map_err(|_| Ssd1681Error::ControlPin(operation))?;
        self.spi
            .write(data)
            .await
            .map_err(|_| Ssd1681Error::Transfer(operation))
    }

    async fn reset(&mut self, operation: ControllerOperation) -> Result<(), Ssd1681Error> {
        self.rst
            .set_high()
            .map_err(|_| Ssd1681Error::ResetPin(operation))?;
        Timer::after(RESET_EDGE_DELAY).await;
        self.rst
            .set_low()
            .map_err(|_| Ssd1681Error::ResetPin(operation))?;
        Timer::after(RESET_EDGE_DELAY).await;
        self.rst
            .set_high()
            .map_err(|_| Ssd1681Error::ResetPin(operation))?;
        Timer::after(RESET_RECOVERY_DELAY).await;
        Ok(())
    }

    async fn wait_idle(
        &mut self,
        operation: ControllerOperation,
        timeout: Duration,
    ) -> Result<(), Ssd1681Error> {
        let deadline = Instant::now() + timeout;
        loop {
            let busy = self
                .busy
                .is_high()
                .map_err(|_| Ssd1681Error::BusyPin(operation))?;
            if !busy {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Ssd1681Error::BusyTimeout(operation));
            }
            Timer::after(BUSY_SAMPLE_INTERVAL).await;
        }
    }
}
