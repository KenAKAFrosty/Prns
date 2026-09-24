use embassy_nrf::twim::Twim;
use embassy_time::{Duration, Timer};
use embedded_graphics::prelude::Point;
use personal_hopspot_core::display::{
    BlankingCommand, BlankingOutcome, BlankingResult, BufferRetention, MonochromePageCache,
    PresentationOutcome,
};
use personal_hopspot_core::face_64x128::{
    Frame, MappedPoint, PanelScale, PanelScaling, PanelSize, PanelTransform, PhysicalPoint,
    QuarterTurn,
};

use crate::boards::DisplayIoError;
use crate::immediate_display::ImmediateDisplayDevice;

/// Seeed has fitted panels strapped to either address; Meshtastic's L1 configuration names 0x3D
/// while common SSD1306 modules answer at 0x3C, so the first one that ACKs is used.
const ADDRESSES: [u8; 2] = [0x3d, 0x3c];
const PANEL_WIDTH: u32 = 128;
const PANEL_HEIGHT: u32 = 64;
const PAGES: usize = (PANEL_HEIGHT / 8) as usize;
const PANEL: PanelSize = match PanelSize::new(PANEL_WIDTH, PANEL_HEIGHT) {
    Ok(panel) => panel,
    Err(_) => panic!("the Wio Tracker L1 panel dimensions are nonzero"),
};
const TRANSFORM: PanelTransform = match PanelTransform::centered(
    PANEL,
    PanelScaling::SampledDestinationPixels(PanelScale::OneToOne),
    QuarterTurn::Clockwise,
) {
    Ok(transform) => transform,
    Err(_) => panic!("the canonical face fits the Wio Tracker L1 panel"),
};
pub(super) type PageCache = MonochromePageCache<{ PANEL_WIDTH as usize }, PAGES>;

const IO_TIMEOUT: embassy_time::Duration = embassy_time::Duration::from_millis(50);

const CONTROL_COMMAND: u8 = 0x00;
const CONTROL_DATA: u8 = 0x40;
const DISPLAY_OFF: u8 = 0xae;
const DISPLAY_ON: u8 = 0xaf;

// Shared SSD1306/SH1106 setup in page-addressing mode, which both controllers support. Segment
// remap (0xA1) and reverse COM scan (0xC8) are the orientation Meshtastic uses on this board.
const SSD1306_INIT: [u8; 23] = [
    DISPLAY_OFF,
    0xd5,
    0x80,
    0xa8,
    0x3f,
    0xd3,
    0x00,
    0x40,
    0x8d,
    0x14,
    0x20,
    0x02,
    0xa1,
    0xc8,
    0xda,
    0x12,
    0x81,
    0xcf,
    0xd9,
    0xf1,
    0xdb,
    0x40,
    0xa4,
];
const SH1106_INIT: [u8; 21] = [
    DISPLAY_OFF,
    0xd5,
    0x80,
    0xa8,
    0x3f,
    0xd3,
    0x00,
    0x40,
    0xad,
    0x8b,
    0xa1,
    0xc8,
    0xda,
    0x12,
    0x81,
    0xcf,
    0xd9,
    0x1f,
    0xdb,
    0x40,
    0xa4,
];
const NORMAL_DISPLAY: u8 = 0xa6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Controller {
    Ssd1306,
    /// 132-column RAM whose glass starts at column 2.
    Sh1106,
}

impl Controller {
    const fn column_offset(self) -> u8 {
        match self {
            Self::Ssd1306 => 0,
            Self::Sh1106 => 2,
        }
    }
}

/// The Wio Tracker L1's 128x64 monochrome OLED on TWIM (SDA P0.06, SCL P0.05).
pub(crate) struct OledDisplay {
    i2c: Twim<'static>,
    address: u8,
    controller: Controller,
    page: &'static mut PageCache,
    initialized: bool,
}

impl OledDisplay {
    pub(crate) fn new(i2c: Twim<'static>, page: &'static mut PageCache) -> Self {
        Self {
            i2c,
            address: ADDRESSES[0],
            controller: Controller::Ssd1306,
            page,
            initialized: false,
        }
    }

    pub(crate) async fn initialize(&mut self) -> Result<(), DisplayIoError> {
        // The controller needs a moment after the 3V3 rail settles before it ACKs.
        Timer::after(Duration::from_millis(50)).await;
        self.controller = self.probe()?;
        let init: &[u8] = match self.controller {
            Controller::Ssd1306 => &SSD1306_INIT,
            Controller::Sh1106 => &SH1106_INIT,
        };
        for &command in init {
            self.command(command)?;
        }
        self.command(NORMAL_DISPLAY)?;
        self.page.invalidate();
        let address = self.address;
        let column = self.controller.column_offset();
        let i2c = &mut self.i2c;
        self.page.present(
            |_| [0; PANEL_WIDTH as usize],
            |page, bytes| Self::write_page(i2c, address, column, page, bytes),
        )?;
        self.command(DISPLAY_ON)?;
        Timer::after(Duration::from_millis(100)).await;
        self.initialized = true;
        Ok(())
    }

    pub(crate) fn force_dark(&mut self) {
        let _ = self.command(DISPLAY_OFF);
        self.initialized = false;
        self.page.invalidate();
    }

    /// Meshtastic's controller probe: the low nibble of the status byte is 0x0 or 0x8 on SH1106
    /// and 0x3..0x7 on SSD1306. An unrecognised value falls back to SSD1306 framing.
    fn probe(&mut self) -> Result<Controller, DisplayIoError> {
        let mut status = [0u8];
        self.address = ADDRESSES
            .into_iter()
            .find(|&address| {
                self.i2c
                    .blocking_write_read_timeout(
                        address,
                        &[CONTROL_COMMAND],
                        &mut status,
                        IO_TIMEOUT,
                    )
                    .is_ok()
            })
            .ok_or(DisplayIoError::I2c)?;
        let mut previous = None;
        for _ in 0..4 {
            self.i2c
                .blocking_write_read_timeout(
                    self.address,
                    &[CONTROL_COMMAND],
                    &mut status,
                    IO_TIMEOUT,
                )
                .map_err(|_| DisplayIoError::I2c)?;
            let nibble = status[0] & 0x0f;
            if previous == Some(nibble) {
                break;
            }
            previous = Some(nibble);
        }
        Ok(match previous {
            Some(0x00 | 0x08) => Controller::Sh1106,
            _ => Controller::Ssd1306,
        })
    }

    fn present_frame(&mut self, frame: &Frame) -> Result<(), DisplayIoError> {
        if !self.initialized {
            return Err(DisplayIoError::NotInitialized);
        }
        let address = self.address;
        let column = self.controller.column_offset();
        let i2c = &mut self.i2c;
        self.page.present(
            |page| {
                let mut next = [0u8; PANEL_WIDTH as usize];
                for bit in 0..8u32 {
                    let y = page as u32 * 8 + bit;
                    for x in 0..PANEL_WIDTH {
                        let Ok(MappedPoint::Source(source)) =
                            TRANSFORM.map_panel_point(PhysicalPoint::new(x, y))
                        else {
                            continue;
                        };
                        if frame.pixel_is_on(Point::new(source.x() as i32, source.y() as i32)) {
                            next[x as usize] |= 1 << bit;
                        }
                    }
                }
                next
            },
            |page, bytes| Self::write_page(i2c, address, column, page, bytes),
        )
    }

    fn write_page(
        i2c: &mut Twim<'static>,
        address: u8,
        column: u8,
        page: usize,
        bytes: &[u8; PANEL_WIDTH as usize],
    ) -> Result<(), DisplayIoError> {
        for command in [0xb0 | page as u8, column & 0x0f, 0x10 | (column >> 4)] {
            i2c.blocking_write_timeout(address, &[CONTROL_COMMAND, command], IO_TIMEOUT)
                .map_err(|_| DisplayIoError::I2c)?;
        }
        let mut chunk = [0u8; PANEL_WIDTH as usize + 1];
        chunk[0] = CONTROL_DATA;
        chunk[1..].copy_from_slice(bytes);
        i2c.blocking_write_timeout(address, &chunk, IO_TIMEOUT)
            .map_err(|_| DisplayIoError::I2c)
    }

    fn command(&mut self, command: u8) -> Result<(), DisplayIoError> {
        self.i2c
            .blocking_write_timeout(self.address, &[CONTROL_COMMAND, command], IO_TIMEOUT)
            .map_err(|_| DisplayIoError::I2c)
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), DisplayIoError> {
        if !self.initialized {
            return Err(DisplayIoError::NotInitialized);
        }
        self.command(if visible { DISPLAY_ON } else { DISPLAY_OFF })
    }
}

impl ImmediateDisplayDevice for OledDisplay {
    fn present(&mut self, frame: &Frame) -> PresentationOutcome {
        match self.present_frame(frame) {
            Ok(()) => PresentationOutcome::Succeeded,
            Err(_) => PresentationOutcome::Failed,
        }
    }

    async fn apply_blanking(&mut self, command: BlankingCommand) -> BlankingOutcome {
        // Display-off keeps GDDRAM, so the last frame returns intact on wake.
        let (result, buffer_retention) = match command {
            BlankingCommand::Blank => (self.set_visible(false), BufferRetention::Preserved),
            BlankingCommand::Restore if self.initialized => {
                (self.set_visible(true), BufferRetention::Preserved)
            }
            BlankingCommand::Restore => (self.initialize().await, BufferRetention::Lost),
        };
        BlankingOutcome {
            result: match result {
                Ok(()) => BlankingResult::Succeeded,
                Err(_) => BlankingResult::Failed,
            },
            buffer_retention,
        }
    }
}
