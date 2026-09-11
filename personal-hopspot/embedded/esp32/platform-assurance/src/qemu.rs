use core::arch::asm;

const EXIT: usize = 1;
const WRITE: usize = 4;
const STDOUT: usize = 1;
const STDERR: usize = 2;

pub enum ExitStatus {
    Success,
    Failure,
}

pub struct WriteError;

pub fn write_stdout(bytes: &[u8]) -> Result<(), WriteError> {
    write_all(STDOUT, bytes)
}

pub fn write_stderr(bytes: &[u8]) -> Result<(), WriteError> {
    write_all(STDERR, bytes)
}

fn write_all(file: usize, mut bytes: &[u8]) -> Result<(), WriteError> {
    while !bytes.is_empty() {
        // SAFETY: QEMU reads the borrowed buffer synchronously before SIMCALL returns.
        let written = unsafe { simcall(WRITE, file, bytes.as_ptr() as usize, bytes.len(), 0) };
        let Ok(written) = usize::try_from(written) else {
            return Err(WriteError);
        };
        if written == 0 || written > bytes.len() {
            return Err(WriteError);
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
        simcall(EXIT, status, 0, 0, 0);
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
    // SAFETY: These are QEMU's Xtensa SIMCALL registers; the instruction does not retain them.
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
