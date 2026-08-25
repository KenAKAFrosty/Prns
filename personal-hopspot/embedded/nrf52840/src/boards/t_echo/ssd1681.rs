//! T-Echo SSD1681 e-ink panel driver with bounded asynchronous BUSY waits.

use core::sync::atomic::{AtomicU16, Ordering};

use embassy_time::{Duration, Instant, Timer};
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal_async::spi::SpiDevice;

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

static LAST_DISPLAY_ERROR: AtomicU16 = AtomicU16::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusyPhase {
    InitializationBeforeReset,
    InitializationAfterReset,
    InitializationReady,
    FullBeforeCurrentWrite,
    FullBeforePreviousWrite,
    FullBeforeRefresh,
    FullAfterRefresh,
    PartialBeforeCurrentWrite,
    PartialBeforeRefresh,
    PartialAfterRefresh,
    PartialBeforePreviousWrite,
    DeepSleep,
    RecoveryBeforeSoftwareReset,
    RecoveryAfterSoftwareReset,
    RecoveryReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferPhase {
    Initialization,
    FullCurrentFrame,
    FullPreviousFrame,
    FullRefresh,
    PartialCurrentFrame,
    PartialRefresh,
    PartialPreviousFrame,
    DeepSleep,
    Recovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetPhase {
    Initialization,
    Recovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ssd1681Error {
    ResetPin(ResetPhase),
    BusyPin(BusyPhase),
    BusyTimeout(BusyPhase),
    Transfer(TransferPhase),
}

/// Preserve the typed controller failure as a compact debugger-visible diagnostic code.
pub(crate) fn observe_error(error: Ssd1681Error) {
    let code = match error {
        Ssd1681Error::ResetPin(phase) => 0x0100 | reset_phase_code(phase),
        Ssd1681Error::BusyPin(phase) => 0x0200 | busy_phase_code(phase),
        Ssd1681Error::BusyTimeout(phase) => 0x0300 | busy_phase_code(phase),
        Ssd1681Error::Transfer(phase) => 0x0400 | transfer_phase_code(phase),
    };
    LAST_DISPLAY_ERROR.store(code, Ordering::Release);
}

const fn reset_phase_code(phase: ResetPhase) -> u16 {
    match phase {
        ResetPhase::Initialization => 1,
        ResetPhase::Recovery => 2,
    }
}

const fn busy_phase_code(phase: BusyPhase) -> u16 {
    match phase {
        BusyPhase::InitializationBeforeReset => 1,
        BusyPhase::InitializationAfterReset => 2,
        BusyPhase::InitializationReady => 3,
        BusyPhase::FullBeforeCurrentWrite => 4,
        BusyPhase::FullBeforePreviousWrite => 5,
        BusyPhase::FullBeforeRefresh => 6,
        BusyPhase::FullAfterRefresh => 7,
        BusyPhase::PartialBeforeCurrentWrite => 8,
        BusyPhase::PartialBeforeRefresh => 9,
        BusyPhase::PartialAfterRefresh => 10,
        BusyPhase::PartialBeforePreviousWrite => 11,
        BusyPhase::DeepSleep => 12,
        BusyPhase::RecoveryBeforeSoftwareReset => 13,
        BusyPhase::RecoveryAfterSoftwareReset => 14,
        BusyPhase::RecoveryReady => 15,
    }
}

const fn transfer_phase_code(phase: TransferPhase) -> u16 {
    match phase {
        TransferPhase::Initialization => 1,
        TransferPhase::FullCurrentFrame => 2,
        TransferPhase::FullPreviousFrame => 3,
        TransferPhase::FullRefresh => 4,
        TransferPhase::PartialCurrentFrame => 5,
        TransferPhase::PartialRefresh => 6,
        TransferPhase::PartialPreviousFrame => 7,
        TransferPhase::DeepSleep => 8,
        TransferPhase::Recovery => 9,
    }
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
    pub async fn new(spi: SPI, busy: BUSY, dc: DC, rst: RST) -> Result<Self, Ssd1681Error> {
        let mut driver = Self { spi, busy, dc, rst };
        driver.reset(ResetPhase::Initialization).await?;
        driver.initialize(false).await?;
        Ok(driver)
    }

    /// Reset and reinitialize the controller after a qualified retained deep sleep.
    pub async fn recover(&mut self) -> Result<(), Ssd1681Error> {
        self.reset(ResetPhase::Recovery).await?;
        self.initialize(true).await
    }

    pub async fn full_update(&mut self, frame: &[u8]) -> Result<(), Ssd1681Error> {
        self.write_ram(
            WRITE_RAM_BW,
            frame,
            BusyPhase::FullBeforeCurrentWrite,
            TransferPhase::FullCurrentFrame,
        )
        .await?;
        self.write_ram(
            WRITE_RAM_PREV,
            frame,
            BusyPhase::FullBeforePreviousWrite,
            TransferPhase::FullPreviousFrame,
        )
        .await?;
        self.run_sequence(
            SEQUENCE_FULL,
            BusyPhase::FullBeforeRefresh,
            BusyPhase::FullAfterRefresh,
            LONG_BUSY_TIMEOUT,
            TransferPhase::FullRefresh,
        )
        .await
    }

    pub async fn partial_update(&mut self, frame: &[u8]) -> Result<(), Ssd1681Error> {
        self.write_ram(
            WRITE_RAM_BW,
            frame,
            BusyPhase::PartialBeforeCurrentWrite,
            TransferPhase::PartialCurrentFrame,
        )
        .await?;
        self.run_sequence(
            SEQUENCE_PARTIAL,
            BusyPhase::PartialBeforeRefresh,
            BusyPhase::PartialAfterRefresh,
            PARTIAL_BUSY_TIMEOUT,
            TransferPhase::PartialRefresh,
        )
        .await?;
        self.write_ram(
            WRITE_RAM_PREV,
            frame,
            BusyPhase::PartialBeforePreviousWrite,
            TransferPhase::PartialPreviousFrame,
        )
        .await
    }

    pub async fn deep_sleep(&mut self) -> Result<(), Ssd1681Error> {
        self.wait_idle(BusyPhase::DeepSleep, LONG_BUSY_TIMEOUT)
            .await?;
        self.cmd_data(DEEP_SLEEP, &[0x01], TransferPhase::DeepSleep)
            .await
    }

    async fn initialize(&mut self, recovery: bool) -> Result<(), Ssd1681Error> {
        let (before_reset, after_reset, ready, transfer) = if recovery {
            (
                BusyPhase::RecoveryBeforeSoftwareReset,
                BusyPhase::RecoveryAfterSoftwareReset,
                BusyPhase::RecoveryReady,
                TransferPhase::Recovery,
            )
        } else {
            (
                BusyPhase::InitializationBeforeReset,
                BusyPhase::InitializationAfterReset,
                BusyPhase::InitializationReady,
                TransferPhase::Initialization,
            )
        };
        self.wait_idle(before_reset, LONG_BUSY_TIMEOUT).await?;
        self.cmd(SW_RESET, transfer).await?;
        self.wait_idle(after_reset, LONG_BUSY_TIMEOUT).await?;
        self.cmd_data(
            DRIVER_OUTPUT_CONTROL,
            &[(HEIGHT - 1) as u8, ((HEIGHT - 1) >> 8) as u8, 0x00],
            transfer,
        )
        .await?;
        self.cmd_data(BORDER_WAVEFORM_CONTROL, &[BORDER_FOLLOW_LUT], transfer)
            .await?;
        self.cmd_data(TEMP_SENSOR_SELECTION, &[0x80], transfer)
            .await?;
        self.set_ram_window(transfer).await?;
        self.wait_idle(ready, LONG_BUSY_TIMEOUT).await
    }

    async fn write_ram(
        &mut self,
        ram: u8,
        frame: &[u8],
        busy_phase: BusyPhase,
        transfer_phase: TransferPhase,
    ) -> Result<(), Ssd1681Error> {
        self.wait_idle(busy_phase, LONG_BUSY_TIMEOUT).await?;
        self.set_ram_window(transfer_phase).await?;
        self.cmd_data(ram, frame, transfer_phase).await
    }

    async fn run_sequence(
        &mut self,
        sequence: u8,
        before: BusyPhase,
        after: BusyPhase,
        timeout: Duration,
        transfer_phase: TransferPhase,
    ) -> Result<(), Ssd1681Error> {
        self.wait_idle(before, LONG_BUSY_TIMEOUT).await?;
        self.cmd_data(DISPLAY_UPDATE_CONTROL_2, &[sequence], transfer_phase)
            .await?;
        self.cmd(MASTER_ACTIVATION, transfer_phase).await?;
        self.wait_idle(after, timeout).await
    }

    async fn set_ram_window(&mut self, phase: TransferPhase) -> Result<(), Ssd1681Error> {
        self.cmd_data(DATA_ENTRY_MODE, &[0x03], phase).await?;
        self.cmd_data(
            SET_RAM_X_START_END,
            &[0x00, ((WIDTH - 1) >> 3) as u8],
            phase,
        )
        .await?;
        self.cmd_data(
            SET_RAM_Y_START_END,
            &[0x00, 0x00, (HEIGHT - 1) as u8, ((HEIGHT - 1) >> 8) as u8],
            phase,
        )
        .await?;
        self.cmd_data(SET_RAM_X_COUNTER, &[0x00], phase).await?;
        self.cmd_data(SET_RAM_Y_COUNTER, &[0x00, 0x00], phase).await
    }

    async fn cmd(&mut self, command: u8, phase: TransferPhase) -> Result<(), Ssd1681Error> {
        self.dc
            .set_low()
            .map_err(|_| Ssd1681Error::Transfer(phase))?;
        self.spi
            .write(&[command])
            .await
            .map_err(|_| Ssd1681Error::Transfer(phase))
    }

    async fn cmd_data(
        &mut self,
        command: u8,
        data: &[u8],
        phase: TransferPhase,
    ) -> Result<(), Ssd1681Error> {
        self.cmd(command, phase).await?;
        self.dc
            .set_high()
            .map_err(|_| Ssd1681Error::Transfer(phase))?;
        self.spi
            .write(data)
            .await
            .map_err(|_| Ssd1681Error::Transfer(phase))
    }

    async fn reset(&mut self, phase: ResetPhase) -> Result<(), Ssd1681Error> {
        self.rst
            .set_high()
            .map_err(|_| Ssd1681Error::ResetPin(phase))?;
        Timer::after(RESET_EDGE_DELAY).await;
        self.rst
            .set_low()
            .map_err(|_| Ssd1681Error::ResetPin(phase))?;
        Timer::after(RESET_EDGE_DELAY).await;
        self.rst
            .set_high()
            .map_err(|_| Ssd1681Error::ResetPin(phase))?;
        Timer::after(RESET_RECOVERY_DELAY).await;
        Ok(())
    }

    async fn wait_idle(&mut self, phase: BusyPhase, timeout: Duration) -> Result<(), Ssd1681Error> {
        let deadline = Instant::now() + timeout;
        loop {
            let busy = self
                .busy
                .is_high()
                .map_err(|_| Ssd1681Error::BusyPin(phase))?;
            if !busy {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Ssd1681Error::BusyTimeout(phase));
            }
            Timer::after(BUSY_SAMPLE_INTERVAL).await;
        }
    }
}
