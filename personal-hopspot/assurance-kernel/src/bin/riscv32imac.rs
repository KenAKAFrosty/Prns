#![no_std]
#![no_main]

use riscv_rt::entry;
use semihosting::{eprintln, println, process};

#[entry]
fn main() -> ! {
    match personal_hopspot_assurance_kernel::run() {
        Ok(evidence) => {
            println!("{}", evidence);
            process::exit(0);
        }
        Err(error) => {
            eprintln!("PRNS_ISA_ERROR {:?}", error);
            process::exit(1);
        }
    }
}
