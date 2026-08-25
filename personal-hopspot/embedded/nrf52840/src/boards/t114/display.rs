use embassy_nrf::gpio::Output;
use embassy_time::Timer;
use embedded_graphics::geometry::Point;
use embedded_hal::spi::SpiDevice;
use personal_hopspot_core::{
    face_64x128, LogicalSize, MappedPoint, PanelScale, PanelSize, PanelTransform, PhysicalPoint,
};

use crate::boards::DisplayIoError;

const PANEL_WIDTH: u16 = 240;
const PANEL_HEIGHT: u16 = 135;
const COLUMN_OFFSET: u16 = 40;
const ROW_OFFSET: u16 = 52;

const SWRESET: u8 = 0x01;
const SLPOUT: u8 = 0x11;
const NORON: u8 = 0x13;
const INVON: u8 = 0x21;
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
    transform: PanelTransform,
    initialized: bool,
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
            transform: PanelTransform::centered_counterclockwise_quarter_turn(
                LogicalSize::new(face_64x128::LOGICAL_WIDTH, face_64x128::LOGICAL_HEIGHT),
                PanelSize::new(u32::from(PANEL_WIDTH), u32::from(PANEL_HEIGHT)),
                PanelScale::FifteenToEight,
            )
            .expect("the T114 face viewport fits its panel"),
            initialized: false,
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
        self.backlight.set_low();
        Ok(())
    }

    pub(crate) fn force_dark(&mut self) {
        self.backlight.set_high();
        self.panel_power.set_high();
        self.initialized = false;
    }

    /// Flush the shared face through the board-qualified viewport and RGB565 packing.
    pub(crate) fn flush(&mut self, frame: &face_64x128::Frame) -> Result<(), DisplayIoError> {
        if !self.initialized {
            return Ok(());
        }

        let viewport = self.transform.viewport();
        let origin = viewport.origin();
        let size = viewport.size();
        self.set_window(
            origin.x() as u16,
            origin.y() as u16,
            (origin.x() + size.width() - 1) as u16,
            (origin.y() + size.height() - 1) as u16,
        )?;
        self.write_command(RAMWR)?;
        let mut row = [0u8; PANEL_WIDTH as usize * 2];
        for physical_y in origin.y()..origin.y() + size.height() {
            for physical_x in origin.x()..origin.x() + size.width() {
                let mapped = self
                    .transform
                    .map_panel_point(PhysicalPoint::new(physical_x, physical_y))
                    .expect("the viewport is inside the T114 panel");
                let color = if matches!(
                    mapped,
                    MappedPoint::Source(point)
                        if frame.pixel_is_on(Point::new(point.x() as i32, point.y() as i32))
                ) {
                    [0xff, 0xff]
                } else {
                    [0x00, 0x00]
                };
                let offset = (physical_x - origin.x()) as usize * 2;
                row[offset..offset + 2].copy_from_slice(&color);
            }
            self.write_data(&row)?;
        }
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
