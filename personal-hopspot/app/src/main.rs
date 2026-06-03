#![cfg_attr(target_arch = "xtensa", no_std)]
#![cfg_attr(target_arch = "xtensa", no_main)]

#[cfg(not(target_arch = "xtensa"))]
mod desktop;
#[cfg(target_arch = "xtensa")]
mod s3;

#[cfg(not(target_arch = "xtensa"))]
fn main() {
    desktop::run();
}

#[cfg(target_arch = "xtensa")]
#[esp_hal::main]
fn main() -> ! {
    s3::run()
}
