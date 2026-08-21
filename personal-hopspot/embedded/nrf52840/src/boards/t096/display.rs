use core::convert::Infallible;

use embassy_nrf::gpio::Output;
use embassy_time::{Duration, Timer};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, OriginDimensions, Pixel, Point, Size};
use embedded_hal::spi::SpiDevice;

const LOGICAL_WIDTH: u32 = 64;
const LOGICAL_HEIGHT: u32 = 128;
const FRAME_BYTES: usize = (LOGICAL_WIDTH * LOGICAL_HEIGHT / 8) as usize;

const PANEL_WIDTH: u16 = 160;
const PANEL_HEIGHT: u16 = 80;
const CONTENT_WIDTH: u16 = LOGICAL_HEIGHT as u16;
const CONTENT_HEIGHT: u16 = LOGICAL_WIDTH as u16;
const CONTENT_X: u16 = (PANEL_WIDTH - CONTENT_WIDTH) / 2;
const CONTENT_Y: u16 = (PANEL_HEIGHT - CONTENT_HEIGHT) / 2;

// The 80x160 red-tab glass occupies columns 24..103 in the ST7735S's 132x162 RAM. Rotation 1
// makes that a 160x80 landscape surface and moves the 24-pixel offset onto the row address.
const ROTATION_ONE_COLUMN_OFFSET: u16 = 0;
const ROTATION_ONE_ROW_OFFSET: u16 = 24;

const SWRESET: u8 = 0x01;
const SLPOUT: u8 = 0x11;
const NORON: u8 = 0x13;
const INVOFF: u8 = 0x20;
const DISPON: u8 = 0x29;
const CASET: u8 = 0x2a;
const RASET: u8 = 0x2b;
const RAMWR: u8 = 0x2c;
const MADCTL: u8 = 0x36;
const COLMOD: u8 = 0x3a;
const FRMCTR1: u8 = 0xb1;
const FRMCTR2: u8 = 0xb2;
const FRMCTR3: u8 = 0xb3;
const INVCTR: u8 = 0xb4;
const PWCTR1: u8 = 0xc0;
const PWCTR2: u8 = 0xc1;
const PWCTR3: u8 = 0xc2;
const PWCTR4: u8 = 0xc3;
const PWCTR5: u8 = 0xc4;
const VMCTR1: u8 = 0xc5;
const GMCTRP1: u8 = 0xe0;
const GMCTRN1: u8 = 0xe1;

// MY | MV | BGR: TFT_eSPI's rotation 1 for ST7735_REDTAB160x80, the configuration used by
// Meshtastic's hardware-validated T096 support.
const LANDSCAPE_MADCTL: u8 = 0x80 | 0x20 | 0x08;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplayIoError {
    Spi,
}

pub(crate) struct St7735Display<SPI> {
    spi: SPI,
    dc: Output<'static>,
    reset: Output<'static>,
    backlight: Output<'static>,
    frame: [u8; FRAME_BYTES],
    displayed_frame: [u8; FRAME_BYTES],
    initialized: bool,
    has_displayed_frame: bool,
}

impl<SPI> St7735Display<SPI>
where
    SPI: SpiDevice<u8>,
{
    pub(crate) fn new(
        spi: SPI,
        dc: Output<'static>,
        reset: Output<'static>,
        backlight: Output<'static>,
    ) -> Self {
        Self {
            spi,
            dc,
            reset,
            backlight,
            frame: [0; FRAME_BYTES],
            displayed_frame: [0; FRAME_BYTES],
            initialized: false,
            has_displayed_frame: false,
        }
    }

    pub(crate) async fn initialize(&mut self) -> Result<(), DisplayIoError> {
        // Backlight is active-low. Keep it dark until controller setup and the initial black clear
        // both succeed, avoiding a bright panel-RAM flash during boot.
        self.backlight.set_high();
        self.reset.set_low();
        Timer::after(Duration::from_millis(20)).await;
        self.reset.set_high();
        Timer::after(Duration::from_millis(120)).await;

        self.command(SWRESET, &[])?;
        Timer::after(Duration::from_millis(150)).await;
        self.command(SLPOUT, &[])?;
        Timer::after(Duration::from_millis(500)).await;
        self.command(FRMCTR1, &[0x01, 0x2c, 0x2d])?;
        self.command(FRMCTR2, &[0x01, 0x2c, 0x2d])?;
        self.command(FRMCTR3, &[0x01, 0x2c, 0x2d, 0x01, 0x2c, 0x2d])?;
        self.command(INVCTR, &[0x07])?;
        self.command(PWCTR1, &[0xa2, 0x02, 0x84])?;
        self.command(PWCTR2, &[0xc5])?;
        self.command(PWCTR3, &[0x0a, 0x00])?;
        self.command(PWCTR4, &[0x8a, 0x2a])?;
        self.command(PWCTR5, &[0x8a, 0xee])?;
        self.command(VMCTR1, &[0x0e])?;
        self.command(INVOFF, &[])?;
        self.command(MADCTL, &[0xc8])?;
        self.command(COLMOD, &[0x05])?;

        // Red-tab 160x80 initialization uses the green-tab address bounds, then applies the
        // panel-specific 24-pixel offset when selecting rotation 1.
        self.command(CASET, &[0x00, 0x02, 0x00, 0x81])?;
        self.command(RASET, &[0x00, 0x01, 0x00, 0xa0])?;
        self.command(
            GMCTRP1,
            &[
                0x02, 0x1c, 0x07, 0x12, 0x37, 0x32, 0x29, 0x2d, 0x29, 0x25, 0x2b, 0x39, 0x00, 0x01,
                0x03, 0x10,
            ],
        )?;
        self.command(
            GMCTRN1,
            &[
                0x03, 0x1d, 0x07, 0x06, 0x2e, 0x2c, 0x29, 0x2d, 0x2e, 0x2e, 0x37, 0x3f, 0x00, 0x00,
                0x02, 0x10,
            ],
        )?;
        self.command(NORON, &[])?;
        Timer::after(Duration::from_millis(10)).await;
        self.command(DISPON, &[])?;
        Timer::after(Duration::from_millis(100)).await;
        self.command(MADCTL, &[LANDSCAPE_MADCTL])?;

        self.clear_panel()?;
        self.initialized = true;
        self.backlight.set_low();
        Ok(())
    }

    pub(crate) const fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Flush the 64x128 shared portrait canvas, rotated clockwise and centered on the 160x80 TFT.
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
            let logical_x = u32::from(physical_y);
            for physical_x in 0..CONTENT_WIDTH {
                let logical_y = u32::from(CONTENT_WIDTH - 1 - physical_x);
                let color = if self.pixel_is_on(logical_x, logical_y) {
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
        let x0 = x0 + ROTATION_ONE_COLUMN_OFFSET;
        let x1 = x1 + ROTATION_ONE_COLUMN_OFFSET;
        let y0 = y0 + ROTATION_ONE_ROW_OFFSET;
        let y1 = y1 + ROTATION_ONE_ROW_OFFSET;
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

impl<SPI> OriginDimensions for St7735Display<SPI> {
    fn size(&self) -> Size {
        Size::new(LOGICAL_WIDTH, LOGICAL_HEIGHT)
    }
}

impl<SPI> DrawTarget for St7735Display<SPI> {
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
