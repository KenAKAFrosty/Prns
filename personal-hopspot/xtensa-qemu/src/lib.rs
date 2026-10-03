#![no_std]
#![cfg_attr(target_arch = "xtensa", feature(asm_experimental_arch))]
#![deny(unsafe_code)]

#[cfg(target_arch = "xtensa")]
pub use xtensa::{exit, write_stderr, write_stdout, ExitStatus, WriteError};

#[cfg(target_arch = "xtensa")]
#[allow(unsafe_code)]
mod xtensa {
    use core::arch::asm;

    const EXIT_OPERATION: usize = 1;
    const WRITE_OPERATION: usize = 4;
    const STANDARD_OUTPUT: usize = 1;
    const STANDARD_ERROR: usize = 2;

    pub enum ExitStatus {
        Success,
        Failure,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum WriteError {
        Rejected,
        NoProgress,
        OverreportedLength,
    }

    pub fn write_stdout(bytes: &[u8]) -> Result<(), WriteError> {
        write_all(STANDARD_OUTPUT, bytes)
    }

    pub fn write_stderr(bytes: &[u8]) -> Result<(), WriteError> {
        write_all(STANDARD_ERROR, bytes)
    }

    fn write_all(file: usize, mut bytes: &[u8]) -> Result<(), WriteError> {
        while !bytes.is_empty() {
            // SAFETY: QEMU reads the borrowed buffer synchronously before SIMCALL returns.
            let written = unsafe {
                simcall(
                    WRITE_OPERATION,
                    file,
                    bytes.as_ptr() as usize,
                    bytes.len(),
                    0,
                )
            };
            let written = usize::try_from(written).map_err(|_| WriteError::Rejected)?;
            if written == 0 {
                return Err(WriteError::NoProgress);
            }
            if written > bytes.len() {
                return Err(WriteError::OverreportedLength);
            }
            bytes = &bytes[written..];
        }
        Ok(())
    }

    pub fn exit(status: ExitStatus) -> ! {
        let status = match status {
            ExitStatus::Success => 0,
            ExitStatus::Failure => 1,
        };
        // SAFETY: EXIT has no pointer arguments, and QEMU accepts every usize status.
        unsafe {
            simcall(EXIT_OPERATION, status, 0, 0, 0);
        }
        loop {
            core::hint::spin_loop();
        }
    }

    unsafe fn simcall(
        operation: usize,
        argument_0: usize,
        argument_1: usize,
        argument_2: usize,
        argument_3: usize,
    ) -> isize {
        let result;
        // SAFETY: These are QEMU's Xtensa SIMCALL registers; the instruction retains none of them.
        unsafe {
            asm!(
                "simcall",
                inout("a2") operation => result,
                in("a3") argument_0,
                in("a4") argument_1,
                in("a5") argument_2,
                in("a6") argument_3,
                options(nostack)
            );
        }
        result
    }
}
