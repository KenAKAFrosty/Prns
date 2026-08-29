use embassy_nrf::gpio::Output;
use embassy_time::Timer;
use embedded_hal::spi::SpiDevice;
use personal_hopspot_core::display::{
    BlankingCommand, BlankingOutcome, BlankingResult, BufferRetention, PresentationOutcome,
};
use personal_hopspot_core::face_64x128::{
    Frame, PanelScale, PanelSize, PanelTransform, PhysicalPoint, QuarterTurn,
};

use crate::boards::{tft, DisplayIoError};
use crate::immediate_display::ImmediateDisplayDevice;

const PANEL_WIDTH: u16 = 240;
const PANEL_HEIGHT: u16 = 135;
const PANEL: PanelSize = match PanelSize::new(PANEL_WIDTH as u32, PANEL_HEIGHT as u32) {
    Ok(panel) => panel,
    Err(_) => panic!("the T114 panel dimensions are nonzero"),
};
const TRANSFORM: PanelTransform = match PanelTransform::centered(
    PANEL,
    PanelScale::FifteenToEight,
    QuarterTurn::CounterClockwise,
) {
    Ok(transform) => transform,
    Err(_) => panic!("the canonical face fits the T114 panel"),
};
const VIEWPORT_WIDTH: usize = TRANSFORM.viewport().size().width() as usize;
const COLUMN_OFFSET: u16 = 40;
const ROW_OFFSET: u16 = 52;

const SWRESET: u8 = 0x01;
const SLPIN: u8 = 0x10;
const SLPOUT: u8 = 0x11;
const NORON: u8 = 0x13;
const INVON: u8 = 0x21;
const DISPOFF: u8 = 0x28;
const DISPON: u8 = 0x29;
const CASET: u8 = 0x2a;
const RASET: u8 = 0x2b;
const RAMWR: u8 = 0x2c;
const MADCTL: u8 = 0x36;
const COLMOD: u8 = 0x3a;
const LANDSCAPE_MADCTL: u8 = 0x80 | 0x20;

const _: () = {
    assert!(COLUMN_OFFSET + PANEL_WIDTH <= 320);
    assert!(ROW_OFFSET + PANEL_HEIGHT <= 240);
};

pub(crate) struct St7789Display<SPI> {
    spi: SPI,
    dc: Output<'static>,
    reset: Output<'static>,
    panel_power: Output<'static>,
    backlight: Output<'static>,
    displayed_frame: Frame,
    initialized: bool,
    has_displayed_frame: bool,
}

impl<SPI> St7789Display<SPI>
where
    SPI: SpiDevice<u8>,
{
    pub(crate) fn new(
        spi: SPI,
        dc: Output<'static>,
        reset: Output<'static>,
        panel_power: Output<'static>,
        backlight: Output<'static>,
    ) -> Self {
        Self {
            spi,
            dc,
            reset,
            panel_power,
            backlight,
            displayed_frame: Frame::new(),
            initialized: false,
            has_displayed_frame: false,
        }
    }

    pub(crate) async fn initialize(&mut self) -> Result<(), DisplayIoError> {
        self.backlight.set_high();
        self.panel_power.set_low();
        Timer::after_millis(10).await;
        self.reset.set_high();
        Timer::after_millis(1).await;
        self.reset.set_low();
        Timer::after_millis(10).await;
        self.reset.set_high();
        Timer::after_millis(120).await;

        self.command(SWRESET, &[])?;
        Timer::after_millis(150).await;
        self.command(SLPOUT, &[])?;
        Timer::after_millis(10).await;
        self.command(COLMOD, &[0x55])?;
        Timer::after_millis(10).await;
        self.command(MADCTL, &[LANDSCAPE_MADCTL])?;
        self.command(INVON, &[])?;
        Timer::after_millis(10).await;
        self.command(NORON, &[])?;
        Timer::after_millis(10).await;
        self.command(DISPON, &[])?;
        Timer::after_millis(100).await;

        self.clear_panel()?;
        self.initialized = true;
        self.has_displayed_frame = false;
        self.backlight.set_low();
        Ok(())
    }

    pub(crate) async fn wake(&mut self) -> Result<(), DisplayIoError> {
        if self.initialized {
            self.backlight.set_low();
            return Ok(());
        }
        self.initialize().await
    }

    pub(crate) async fn darken(&mut self) -> Result<(), DisplayIoError> {
        self.backlight.set_high();
        let result = if self.initialized {
            self.command(DISPOFF, &[])
                .and_then(|()| self.command(SLPIN, &[]))
        } else {
            Ok(())
        };
        Timer::after_millis(120).await;
        self.panel_power.set_high();
        self.initialized = false;
        self.has_displayed_frame = false;
        result
    }

    pub(crate) fn force_dark(&mut self) {
        self.backlight.set_high();
        self.panel_power.set_high();
        self.initialized = false;
        self.has_displayed_frame = false;
    }

    fn present_frame(&mut self, frame: &Frame) -> Result<(), DisplayIoError> {
        if !self.initialized {
            return Err(DisplayIoError::NotInitialized);
        }
        if self.has_displayed_frame && frame == &self.displayed_frame {
            return Ok(());
        }

        let viewport = TRANSFORM.viewport();
        let origin = viewport.origin();
        let size = viewport.size();
        self.set_window(
            origin.x() as u16,
            origin.y() as u16,
            (origin.x() + size.width() - 1) as u16,
            (origin.y() + size.height() - 1) as u16,
        )?;
        self.write_command(RAMWR)?;
        let mut row = [0u8; VIEWPORT_WIDTH * 2];
        for physical_y in origin.y()..origin.y() + size.height() {
            for physical_x in origin.x()..origin.x() + size.width() {
                let color = tft::rgb565_pixel(
                    frame,
                    &TRANSFORM,
                    PhysicalPoint::new(physical_x, physical_y),
                );
                let offset = (physical_x - origin.x()) as usize * 2;
                row[offset..offset + 2].copy_from_slice(&color);
            }
            self.write_data(&row)?;
        }
        self.displayed_frame.clone_from(frame);
        self.has_displayed_frame = true;
        Ok(())
    }

    fn clear_panel(&mut self) -> Result<(), DisplayIoError> {
        self.set_window(0, 0, PANEL_WIDTH - 1, PANEL_HEIGHT - 1)?;
        self.write_command(RAMWR)?;
        let black_row = [0u8; PANEL_WIDTH as usize * 2];
        for _ in 0..PANEL_HEIGHT {
            self.write_data(&black_row)?;
        }
        Ok(())
    }

    fn set_window(&mut self, x0: u16, y0: u16, x1: u16, y1: u16) -> Result<(), DisplayIoError> {
        let x0 = x0 + COLUMN_OFFSET;
        let x1 = x1 + COLUMN_OFFSET;
        let y0 = y0 + ROW_OFFSET;
        let y1 = y1 + ROW_OFFSET;
        self.command(
            CASET,
            &[(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8],
        )?;
        self.command(
            RASET,
            &[(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8],
        )
    }

    fn command(&mut self, command: u8, data: &[u8]) -> Result<(), DisplayIoError> {
        self.write_command(command)?;
        if !data.is_empty() {
            self.write_data(data)?;
        }
        Ok(())
    }

    fn write_command(&mut self, command: u8) -> Result<(), DisplayIoError> {
        self.dc.set_low();
        self.spi.write(&[command]).map_err(|_| DisplayIoError::Spi)
    }

    fn write_data(&mut self, data: &[u8]) -> Result<(), DisplayIoError> {
        self.dc.set_high();
        self.spi.write(data).map_err(|_| DisplayIoError::Spi)
    }
}

impl<SPI: SpiDevice<u8>> ImmediateDisplayDevice for St7789Display<SPI> {
    fn present(&mut self, frame: &Frame) -> PresentationOutcome {
        match self.present_frame(frame) {
            Ok(()) => PresentationOutcome::Succeeded,
            Err(DisplayIoError::Spi | DisplayIoError::NotInitialized) => {
                PresentationOutcome::Failed
            }
        }
    }

    async fn apply_blanking(&mut self, command: BlankingCommand) -> BlankingOutcome {
        let result = match command {
            BlankingCommand::Blank => self.darken().await,
            BlankingCommand::Restore => self.wake().await,
        };
        BlankingOutcome {
            result: match result {
                Ok(()) => BlankingResult::Succeeded,
                Err(DisplayIoError::Spi | DisplayIoError::NotInitialized) => BlankingResult::Failed,
            },
            buffer_retention: BufferRetention::Lost,
        }
    }
}
