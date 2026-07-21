//! Windows single-open COM transport.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;

use windows::Win32::Devices::Communication::{
    SetCommState, SetCommTimeouts, COMMTIMEOUTS, DCB, EVENPARITY, NOPARITY, ODDPARITY, ONESTOPBIT,
    TWOSTOPBITS,
};
use windows::Win32::Foundation::HANDLE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    None,
    Even,
    Odd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBits {
    One,
    Two,
}

/// A blocking Windows serial handle opened exactly once.
pub struct WindowsSerial {
    file: File,
}

/// Open and configure a COM port without an intermediate settings query or reopen.
pub fn open(
    path: &str,
    baud: u32,
    data_bits: u8,
    parity: Parity,
    stop_bits: StopBits,
) -> io::Result<WindowsSerial> {
    if !(5..=8).contains(&data_bits) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "serial data bits must be between 5 and 8",
        ));
    }
    let device_path = if path.starts_with(r"\\.\") {
        path.to_string()
    } else {
        format!(r"\\.\{path}")
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(device_path)?;
    let handle = HANDLE(file.as_raw_handle());
    let dcb = DCB {
        DCBlength: std::mem::size_of::<DCB>() as u32,
        BaudRate: baud,
        // fBinary is required. fParity is enabled only when parity checking was requested; all
        // flow-control, DTR, RTS, abort-on-error, and character-substitution flags stay disabled.
        _bitfield: 1 | if parity == Parity::None { 0 } else { 2 },
        ByteSize: data_bits,
        Parity: match parity {
            Parity::None => NOPARITY,
            Parity::Even => EVENPARITY,
            Parity::Odd => ODDPARITY,
        },
        StopBits: match stop_bits {
            StopBits::One => ONESTOPBIT,
            StopBits::Two => TWOSTOPBITS,
        },
        ..Default::default()
    };
    // SAFETY: `handle` is the live COM File handle owned by `file`; `dcb` has the documented size
    // and remains valid for the duration of this synchronous call.
    unsafe { SetCommState(handle, &dcb) }.map_err(io::Error::other)?;
    let timeouts = COMMTIMEOUTS {
        ReadIntervalTimeout: u32::MAX,
        ReadTotalTimeoutMultiplier: u32::MAX,
        ReadTotalTimeoutConstant: 20,
        WriteTotalTimeoutMultiplier: 0,
        WriteTotalTimeoutConstant: 20,
    };
    // SAFETY: `handle` remains live and `timeouts` is a fully initialized input structure.
    unsafe { SetCommTimeouts(handle, &timeouts) }.map_err(io::Error::other)?;
    Ok(WindowsSerial { file })
}

impl Read for WindowsSerial {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Write for WindowsSerial {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
