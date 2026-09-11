#![no_std]
#![no_main]

use core::fmt::{self, Write};
use core::panic::PanicInfo;

const STDOUT: i32 = 1;
const STDERR: i32 = 2;
const SUCCESS: i32 = 0;
const FAILURE: i32 = 1;

unsafe extern "C" {
    fn exit(status: i32) -> !;
    fn write(file: i32, buffer: *const u8, length: usize) -> isize;
}

struct Output(i32);

impl Write for Output {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let mut remaining = value.as_bytes();
        while !remaining.is_empty() {
            // SAFETY: Espressif newlib consumes this borrowed buffer before returning.
            let written = unsafe { write(self.0, remaining.as_ptr(), remaining.len()) };
            let Ok(written) = usize::try_from(written) else {
                return Err(fmt::Error);
            };
            if written == 0 || written > remaining.len() {
                return Err(fmt::Error);
            }
            remaining = &remaining[written..];
        }
        Ok(())
    }
}

fn terminate(status: i32) -> ! {
    // SAFETY: Espressif libgloss accepts every i32 status and never returns from exit.
    unsafe { exit(status) }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let _ = writeln!(Output(STDERR), "PRNS_ISA_PANIC {info}");
    terminate(FAILURE)
}

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    match personal_hopspot_assurance_kernel::run() {
        Ok(evidence) => {
            if writeln!(Output(STDOUT), "{evidence}").is_err() {
                terminate(FAILURE);
            }
            terminate(SUCCESS);
        }
        Err(error) => {
            let _ = writeln!(Output(STDERR), "PRNS_ISA_ERROR {error:?}");
            terminate(FAILURE);
        }
    }
}
