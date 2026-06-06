use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use std::vec::Vec;

use mio::{Events, Interest, Poll, Registry, Token, Waker};
use mio_serial::SerialStream;
use serialport::SerialPort;

use super::super::core::host_descriptor;
use super::super::discovery::{Discoverer, PortId, PumpCadence, UsbAutoContext};
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InterfaceId, SelfDrivenInterface,
};

/// USB CDC ignores baud, but the serialport API still wants a number.
const CDC_BAUD: u32 = 115_200;
const SCAN_INTERVAL: Duration = Duration::from_millis(300);
const PENDING_FLUSH_INTERVAL: Duration = Duration::from_millis(1);
const WAKE_TOKEN: Token = Token(0);
const POLL_EVENTS_CAPACITY: usize = 16;

pub fn usb_auto_interface(id: InterfaceId) -> SelfDrivenInterface<impl FnOnce(UsbAutoContext)> {
    SelfDrivenInterface::new(host_descriptor(id), move |ctx| {
        thread::spawn(move || serve(ctx));
    })
}

fn serve(mut ctx: UsbAutoContext) {
    let Ok(mut poll) = Poll::new() else {
        ctx.control.report(ControlReport::Stopped);
        return;
    };
    let Ok(waker) = Waker::new(poll.registry(), WAKE_TOKEN).map(Arc::new) else {
        ctx.control.report(ControlReport::Stopped);
        return;
    };
    ctx.outbound.arm_wake({
        let waker = Arc::clone(&waker);
        move || {
            let _ = waker.wake();
        }
    });

    let mut events = Events::with_capacity(POLL_EVENTS_CAPACITY);
    let mut discoverer: Discoverer<SerialStream> = Discoverer::new();
    let mut registered: HashSet<PortId> = HashSet::new();
    let mut next_token = WAKE_TOKEN.0 + 1;
    let mut last_scan: Option<Instant> = None;

    loop {
        if last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            last_scan = Some(Instant::now());
            discoverer.reconcile_present(&scan_cdc_ports(), open_cdc_port);
            register_ports(
                poll.registry(),
                &mut discoverer,
                &mut registered,
                &mut next_token,
            );
        }

        let cadence = discoverer.pump(&mut ctx);
        if matches!(ctx.control.next_command(), Some(ControlCommand::Stop)) {
            break;
        }
        if matches!(cadence, PumpCadence::Idle) {
            let timeout = if discoverer.has_pending_writes() {
                PENDING_FLUSH_INTERVAL
            } else {
                last_scan.map_or(Duration::ZERO, |t| {
                    SCAN_INTERVAL.saturating_sub(t.elapsed())
                })
            };
            if let Err(e) = poll.poll(&mut events, Some(timeout)) {
                if e.kind() != io::ErrorKind::Interrupted {
                    break;
                }
            }
        }
    }
    ctx.control.report(ControlReport::Stopped);
}

fn register_ports(
    registry: &Registry,
    discoverer: &mut Discoverer<SerialStream>,
    registered: &mut HashSet<PortId>,
    next_token: &mut usize,
) {
    let mut current: Vec<PortId> = Vec::new();
    for (id, port) in discoverer.ports_mut() {
        current.push(id.clone());
        if !registered.contains(id) {
            let token = Token(*next_token);
            if registry.register(port, token, Interest::READABLE).is_ok() {
                *next_token += 1;
                registered.insert(id.clone());
            }
        }
    }
    registered.retain(|id| current.contains(id));
}

fn scan_cdc_ports() -> Vec<PortId> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|info| matches!(info.port_type, serialport::SerialPortType::UsbPort(_)))
        .map(|info| PortId::new(info.port_name))
        .collect()
}

fn open_cdc_port(id: &PortId) -> io::Result<SerialStream> {
    let mut port =
        SerialStream::open(&serialport::new(id.as_str(), CDC_BAUD)).map_err(io::Error::other)?;
    // An ESP32's native USB-serial-jtag maps the modem lines to its boot/reset
    // pins (RTS→EN, DTR→GPIO0); it reads the single combination DTR=0, RTS=1 as
    // a chip reset. Linux cdc-acm opens the port at DTR=1, RTS=1 (a safe combo),
    // so we must lower RTS *before* DTR — `(1,1)→(1,0)→(0,0)` — to settle the
    // lines without ever passing through the reset combination. Dropping DTR
    // first would momentarily sit at `(0,1)` and reboot the board on every open.
    // (A board behind a USB-UART bridge ignores these, so it is harmless there.)
    let _ = port.write_request_to_send(false);
    let _ = port.write_data_terminal_ready(false);
    Ok(port)
}
