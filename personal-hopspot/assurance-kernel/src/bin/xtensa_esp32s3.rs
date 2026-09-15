#![cfg_attr(target_arch = "xtensa", no_std)]
#![cfg_attr(target_arch = "xtensa", no_main)]
#![deny(unsafe_code)]

#[cfg(target_arch = "xtensa")]
mod target {
    use core::fmt::{self, Write};
    use core::panic::PanicInfo;
    use personal_hopspot_xtensa_qemu as qemu;

    enum Output {
        StandardOutput,
        StandardError,
    }

    impl Write for Output {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            let result = match self {
                Self::StandardOutput => qemu::write_stdout(value.as_bytes()),
                Self::StandardError => qemu::write_stderr(value.as_bytes()),
            };
            result.map_err(|_| fmt::Error)
        }
    }

    #[panic_handler]
    fn panic(info: &PanicInfo<'_>) -> ! {
        let _ = writeln!(Output::StandardError, "PRNS_ISA_PANIC {info}");
        qemu::exit(qemu::ExitStatus::Failure)
    }

    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    extern "C" fn main() -> ! {
        match personal_hopspot_assurance_kernel::run() {
            Ok(evidence) => {
                if writeln!(Output::StandardOutput, "{evidence}").is_err() {
                    qemu::exit(qemu::ExitStatus::Failure);
                }
                qemu::exit(qemu::ExitStatus::Success);
            }
            Err(error) => {
                let _ = writeln!(Output::StandardError, "PRNS_ISA_ERROR {error:?}");
                qemu::exit(qemu::ExitStatus::Failure);
            }
        }
    }
}

#[cfg(not(target_arch = "xtensa"))]
fn main() -> std::process::ExitCode {
    eprintln!("assurance-xtensa-esp32s3 requires the xtensa-esp32s3-none-elf target");
    std::process::ExitCode::FAILURE
}
