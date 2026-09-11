#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]
#![deny(unsafe_code)]

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use embassy_executor::Spawner;
use personal_hopspot_memory::HELTEC_WIRELESS_STICK_LITE_V3;

#[allow(unsafe_code)]
mod qemu;

esp_bootloader_esp_idf::esp_app_desc!();

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    let _ = qemu::write_stderr(b"PRNS_PLATFORM_PANIC\n");
    qemu::exit(qemu::ExitStatus::Failure)
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    prns_platform_runtime_entry()
}

#[inline(never)]
fn prns_platform_runtime_entry() -> ! {
    let mut output = Output::new();
    let result = writeln!(
        output,
        "PRNS_PLATFORM_MILESTONE schema=1 platform=esp32s3 profile={} milestone=runtime-initialized",
        HELTEC_WIRELESS_STICK_LITE_V3.id.as_str()
    );
    if result.is_err() {
        qemu::exit(qemu::ExitStatus::Failure)
    }
    if qemu::write_stdout(output.as_bytes()).is_err() {
        qemu::exit(qemu::ExitStatus::Failure)
    }
    qemu::exit(qemu::ExitStatus::Success)
}

const OUTPUT_CAPACITY: usize = 160;

struct Output {
    bytes: [u8; OUTPUT_CAPACITY],
    length: usize,
}

impl Output {
    const fn new() -> Self {
        Self {
            bytes: [0; OUTPUT_CAPACITY],
            length: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for Output {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.length.checked_add(value.len()).ok_or(fmt::Error)?;
        let destination = self.bytes.get_mut(self.length..end).ok_or(fmt::Error)?;
        destination.copy_from_slice(value.as_bytes());
        self.length = end;
        Ok(())
    }
}
