use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::time::{Instant, Rate};
use esp_println::println;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use personal_hopspot_ui as screen;

esp_app_desc!();

pub fn run() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    println!("HOPSPOT_S3 up");

    // Heltec V4: Vext (active-low) gates panel power; pulse RST; I2C0 on 17/18.
    let mut vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
    vext.set_low();
    let mut rst = Output::new(p.GPIO21, Level::High, OutputConfig::default());
    rst.set_low();
    block_ms(20);
    rst.set_high();
    block_ms(20);

    let i2c = I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(400)))
        .expect("i2c0")
        .with_sda(p.GPIO17)
        .with_scl(p.GPIO18);
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate90,
    )
    .into_buffered_graphics_mode();
    display.init().expect("ssd1306 init");

    screen::splash(&mut display, "Heltec S3");
    let _ = display.flush();

    // Pre-launch app work done; later `Prns::run(platform)` blocks here instead of idling.
    loop {
        block_ms(1000);
    }
}

fn block_ms(ms: u64) {
    let target = Instant::now().duration_since_epoch().as_millis() + ms;
    while Instant::now().duration_since_epoch().as_millis() < target {}
}
