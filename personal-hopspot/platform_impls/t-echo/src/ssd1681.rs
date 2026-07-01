use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal::spi::SpiDevice;

pub const WIDTH: u32 = 200;
pub const HEIGHT: u32 = 200;

const SW_RESET: u8 = 0x12;
const DRIVER_OUTPUT_CONTROL: u8 = 0x01;
const DATA_ENTRY_MODE: u8 = 0x11;
const TEMP_SENSOR_SELECTION: u8 = 0x18;
const MASTER_ACTIVATION: u8 = 0x20;
const DISPLAY_UPDATE_CONTROL_2: u8 = 0x22;
const WRITE_RAM_BW: u8 = 0x24;
const WRITE_RAM_PREV: u8 = 0x26;
const BORDER_WAVEFORM_CONTROL: u8 = 0x3C;
const SET_RAM_X_START_END: u8 = 0x44;
const SET_RAM_Y_START_END: u8 = 0x45;
const SET_RAM_X_COUNTER: u8 = 0x4E;
const SET_RAM_Y_COUNTER: u8 = 0x4F;

const SEQUENCE_FULL: u8 = 0xF7;
const SEQUENCE_PARTIAL: u8 = 0xFC;
const BORDER_FOLLOW_LUT: u8 = 0x05;

pub struct Ssd1681<SPI, BUSY, DC, RST, DELAY> {
    spi: SPI,
    busy: BUSY,
    dc: DC,
    rst: RST,
    delay: DELAY,
}

impl<SPI, BUSY, DC, RST, DELAY> Ssd1681<SPI, BUSY, DC, RST, DELAY>
where
    SPI: SpiDevice,
    BUSY: InputPin,
    DC: OutputPin,
    RST: OutputPin,
    DELAY: DelayNs,
{
    pub fn new(spi: SPI, busy: BUSY, dc: DC, rst: RST, delay: DELAY) -> Result<Self, SPI::Error> {
        let mut driver = Self {
            spi,
            busy,
            dc,
            rst,
            delay,
        };
        driver.reset();
        driver.init()?;
        Ok(driver)
    }

    pub fn full_update(&mut self, frame: &[u8]) -> Result<(), SPI::Error> {
        self.write_ram(WRITE_RAM_BW, frame)?;
        self.write_ram(WRITE_RAM_PREV, frame)?;
        self.run_sequence(SEQUENCE_FULL)?;
        Ok(())
    }

    pub fn partial_update(&mut self, frame: &[u8]) -> Result<(), SPI::Error> {
        self.write_ram(WRITE_RAM_BW, frame)?;
        self.run_sequence(SEQUENCE_PARTIAL)?;
        self.write_ram(WRITE_RAM_PREV, frame)?;
        Ok(())
    }

    fn init(&mut self) -> Result<(), SPI::Error> {
        self.wait_idle();
        self.cmd(SW_RESET)?;
        self.wait_idle();
        self.cmd_data(
            DRIVER_OUTPUT_CONTROL,
            &[(HEIGHT - 1) as u8, ((HEIGHT - 1) >> 8) as u8, 0x00],
        )?;
        self.cmd_data(BORDER_WAVEFORM_CONTROL, &[BORDER_FOLLOW_LUT])?;
        self.cmd_data(TEMP_SENSOR_SELECTION, &[0x80])?;
        self.set_ram_window()?;
        self.wait_idle();
        Ok(())
    }

    fn write_ram(&mut self, ram: u8, frame: &[u8]) -> Result<(), SPI::Error> {
        self.wait_idle();
        self.set_ram_window()?;
        self.cmd_data(ram, frame)?;
        Ok(())
    }

    fn run_sequence(&mut self, sequence: u8) -> Result<(), SPI::Error> {
        self.wait_idle();
        self.cmd_data(DISPLAY_UPDATE_CONTROL_2, &[sequence])?;
        self.cmd(MASTER_ACTIVATION)?;
        self.wait_idle();
        Ok(())
    }

    fn set_ram_window(&mut self) -> Result<(), SPI::Error> {
        self.cmd_data(DATA_ENTRY_MODE, &[0x03])?;
        self.cmd_data(SET_RAM_X_START_END, &[0x00, ((WIDTH - 1) >> 3) as u8])?;
        self.cmd_data(
            SET_RAM_Y_START_END,
            &[0x00, 0x00, (HEIGHT - 1) as u8, ((HEIGHT - 1) >> 8) as u8],
        )?;
        self.cmd_data(SET_RAM_X_COUNTER, &[0x00])?;
        self.cmd_data(SET_RAM_Y_COUNTER, &[0x00, 0x00])?;
        Ok(())
    }

    fn cmd(&mut self, command: u8) -> Result<(), SPI::Error> {
        let _ = self.dc.set_low();
        self.spi.write(&[command])
    }

    fn cmd_data(&mut self, command: u8, data: &[u8]) -> Result<(), SPI::Error> {
        self.cmd(command)?;
        let _ = self.dc.set_high();
        self.spi.write(data)
    }

    fn reset(&mut self) {
        let _ = self.rst.set_high();
        self.delay.delay_us(10_000);
        let _ = self.rst.set_low();
        self.delay.delay_us(10_000);
        let _ = self.rst.set_high();
        self.delay.delay_us(200_000);
    }

    fn wait_idle(&mut self) {
        while self.busy.is_high().unwrap_or(false) {
            self.delay.delay_us(5_000);
        }
    }
}
