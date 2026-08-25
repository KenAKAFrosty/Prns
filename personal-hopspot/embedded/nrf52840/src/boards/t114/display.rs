use core::convert::Infallible;

use embassy_nrf::gpio::Output;
use embassy_time::Timer;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, OriginDimensions, Pixel, Point, Size};
use embedded_hal::spi::SpiDevice;
use personal_hopspot_core::{CanvasDimensions, QuarterTurn, RotatedCanvasMapping};

use crate::boards::DisplayIoError;

const LOGICAL_WIDTH: u32 = 64;
const LOGICAL_HEIGHT: u32 = 128;
const FRAME_BYTES: usize = (LOGICAL_WIDTH * LOGICAL_HEIGHT / 8) as usize;

const PANEL_WIDTH: u16 = 240;
const PANEL_HEIGHT: u16 = 135;
const CONTENT_WIDTH: u16 = PANEL_WIDTH;
const CONTENT_HEIGHT: u16 = CONTENT_WIDTH * LOGICAL_WIDTH as u16 / LOGICAL_HEIGHT as u16;
const CONTENT_X: u16 = (PANEL_WIDTH - CONTENT_WIDTH) / 2;
const CONTENT_Y: u16 = (PANEL_HEIGHT - CONTENT_HEIGHT) / 2;
const COLUMN_OFFSET: u16 = 40;
const ROW_OFFSET: u16 = 52;
const CANVAS_MAPPING: RotatedCanvasMapping = RotatedCanvasMapping::new(
    CanvasDimensions::new(LOGICAL_WIDTH, LOGICAL_HEIGHT),
    CanvasDimensions::new(CONTENT_WIDTH as u32, CONTENT_HEIGHT as u32),
    QuarterTurn::CounterClockwise,
);

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
    assert!(CONTENT_X + CONTENT_WIDTH <= PANEL_WIDTH);
    assert!(CONTENT_Y + CONTENT_HEIGHT <= PANEL_HEIGHT);
};

pub(crate) struct St7789Display<SPI> {
    spi: SPI,
    dc: Output<'static>,
    reset: Output<'static>,
    panel_power: Output<'static>,
    backlight: Output<'static>,
    frame: [u8; FRAME_BYTES],
    displayed_frame: [u8; FRAME_BYTES],
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
            frame: [0; FRAME_BYTES],
            displayed_frame: [0; FRAME_BYTES],
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

    pub(crate) fn flush(&mut self) -> Result<(), DisplayIoError> {
        if !self.initialized || (self.has_displayed_frame && self.frame == self.displayed_frame) {
            return Ok(());
        }

        self.set_window(
            CONTENT_X,
            CONTENT_Y,
            CONTENT_X + CONTENT_WIDTH - 1,
            CONTENT_Y + CONTENT_HEIGHT - 1,
        )?;
        self.write_command(RAMWR)?;
        let mut row = [0u8; CONTENT_WIDTH as usize * 2];
        for physical_y in 0..CONTENT_HEIGHT {
            for physical_x in 0..CONTENT_WIDTH {
                let logical =
                    CANVAS_MAPPING.logical_point(u32::from(physical_x), u32::from(physical_y));
                let color = if self.pixel_is_on(logical.x, logical.y) {
                    [0xff, 0xff]
                } else {
                    [0x00, 0x00]
                };
                let offset = usize::from(physical_x) * 2;
                row[offset..offset + 2].copy_from_slice(&color);
            }
            self.write_data(&row)?;
        }
        self.displayed_frame.copy_from_slice(&self.frame);
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

    fn pixel_is_on(&self, x: u32, y: u32) -> bool {
        let bit_index = y * LOGICAL_WIDTH + x;
        let byte = self.frame[(bit_index / 8) as usize];
        byte & (0x80 >> (bit_index % 8)) != 0
    }
}

impl<SPI> OriginDimensions for St7789Display<SPI> {
    fn size(&self) -> Size {
        Size::new(LOGICAL_WIDTH, LOGICAL_HEIGHT)
    }
}

impl<SPI> DrawTarget for St7789Display<SPI> {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(Point { x, y }, color) in pixels {
            if x < 0 || y < 0 || x >= LOGICAL_WIDTH as i32 || y >= LOGICAL_HEIGHT as i32 {
                continue;
            }
            let bit_index = y as u32 * LOGICAL_WIDTH + x as u32;
            let byte = &mut self.frame[(bit_index / 8) as usize];
            let mask = 0x80 >> (bit_index % 8);
            match color {
                BinaryColor::On => *byte |= mask,
                BinaryColor::Off => *byte &= !mask,
            }
        }
        Ok(())
    }
}
