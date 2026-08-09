use std::io::{stderr, stdout};
use std::os::windows::io::{AsRawHandle, RawHandle};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{
    GetConsoleMode, SetConsoleMode, CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
};

pub fn enable_ansi_sequences() {
    for handle in [stdout().as_raw_handle(), stderr().as_raw_handle()] {
        enable_on(handle);
    }
}

fn enable_on(handle: RawHandle) -> bool {
    if handle.is_null() {
        return false;
    }
    let console = HANDLE(handle);
    let mut mode = CONSOLE_MODE(0);
    // SAFETY: `handle` is one of the calling process's standard handles (or a caller-owned
    // handle in tests), live for this synchronous call; `mode` is a valid output slot, and
    // GetConsoleMode fails harmlessly when the handle is not a console.
    if unsafe { GetConsoleMode(console, &mut mode) }.is_err() {
        return false;
    }
    if mode.contains(ENABLE_VIRTUAL_TERMINAL_PROCESSING) {
        return true;
    }
    // SAFETY: `console` was just verified to be a live console handle; adding a mode flag
    // never invalidates it, and failure leaves the previous mode untouched.
    unsafe { SetConsoleMode(console, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) }.is_ok()
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;

    use super::*;

    #[test]
    fn non_console_handles_are_refused_without_side_effects() {
        assert!(!enable_on(std::ptr::null_mut()));
        let file = File::open("Cargo.toml").expect("crate manifest");
        assert!(!enable_on(file.as_raw_handle()));
    }

    #[test]
    fn enabling_twice_is_idempotent() {
        let first = enable_on(std::io::stdout().as_raw_handle());
        let second = enable_on(std::io::stdout().as_raw_handle());
        assert_eq!(first, second);
    }
}
