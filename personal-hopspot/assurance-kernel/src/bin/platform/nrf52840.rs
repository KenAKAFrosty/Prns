#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

#[entry]
fn main() -> ! {
    prns_platform_application_entry()
}

#[no_mangle]
#[inline(never)]
extern "C" fn prns_platform_application_entry() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
