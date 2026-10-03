#![no_std]
#![no_main]

use cortex_m_rt::entry;
use cortex_m_semihosting::{debug, hprintln};
use panic_halt as _;

#[entry]
fn main() -> ! {
    match personal_hopspot_assurance_kernel::run() {
        Ok(evidence) => {
            hprintln!("{}", evidence);
            debug::exit(debug::EXIT_SUCCESS);
        }
        Err(error) => {
            hprintln!("PRNS_ISA_ERROR {:?}", error);
            debug::exit(debug::EXIT_FAILURE);
        }
    }
    loop {
        core::hint::spin_loop();
    }
}
