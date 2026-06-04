use std::io::{self, Read, Write};
use std::thread;
use std::time::{Duration, Instant};
use std::vec::Vec;

use super::super::core::host_descriptor;
use super::super::discovery::{Discoverer, PortId, UsbAutoContext};
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InterfaceId, SelfDrivenInterface,
};

/// USB CDC ignores baud, but the serialport API still wants a number.
const CDC_BAUD: u32 = 115_200;
const READ_TIMEOUT: Duration = Duration::from_millis(5);
const SERVICE_INTERVAL: Duration = Duration::from_millis(10);
const SCAN_INTERVAL: Duration = Duration::from_millis(300);

pub fn usb_auto_interface(id: InterfaceId) -> SelfDrivenInterface<impl FnOnce(UsbAutoContext)> {
    SelfDrivenInterface::new(host_descriptor(id), move |ctx| {
        thread::spawn(move || serve(ctx));
    })
}

fn serve(mut ctx: UsbAutoContext) {
    let mut discoverer = Discoverer::new();
    let mut last_scan: Option<Instant> = None;
    loop {
        if last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            last_scan = Some(Instant::now());
            discoverer.reconcile_present(&scan_cdc_ports(), open_cdc_port);
        }
        discoverer.pump(&mut ctx);
        if matches!(ctx.control.next_command(), Some(ControlCommand::Stop)) {
            break;
        }
        thread::sleep(SERVICE_INTERVAL);
    }
    ctx.control.report(ControlReport::Stopped);
}

fn scan_cdc_ports() -> Vec<PortId> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|info| matches!(info.port_type, serialport::SerialPortType::UsbPort(_)))
        .map(|info| PortId::new(info.port_name))
        .collect()
}

struct CdcPort(Box<dyn serialport::SerialPort>);

impl Read for CdcPort {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for CdcPort {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

fn open_cdc_port(id: &PortId) -> io::Result<CdcPort> {
    let mut port = serialport::new(id.as_str(), CDC_BAUD)
        .timeout(READ_TIMEOUT)
        .open()
        .map_err(io::Error::other)?;
    // An ESP32's native USB-serial-jtag maps the modem lines to its boot/reset
    // pins (RTS→EN, DTR→GPIO0); it reads the single combination DTR=0, RTS=1 as
    // a chip reset. Linux cdc-acm opens the port at DTR=1, RTS=1 (a safe combo),
    // so we must lower RTS *before* DTR — `(1,1)→(1,0)→(0,0)` — to settle the
    // lines without ever passing through the reset combination. Dropping DTR
    // first would momentarily sit at `(0,1)` and reboot the board on every open.
    // (A board behind a USB-UART bridge ignores these, so it is harmless there.)
    let _ = port.write_request_to_send(false);
    let _ = port.write_data_terminal_ready(false);
    Ok(CdcPort(port))
}
