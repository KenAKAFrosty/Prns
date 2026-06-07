use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::super::core::{host_descriptor, node_tag_for, Capabilities, NodeTag};
use super::super::discovery::{Discoverer, PortId, PumpCadence, UsbAutoContext};
use crate::interfaces::{
    ControlCommand, ControlEndpoint, ControlReport, InterfaceId, SelfDrivenInterface,
};

const SCAN_INTERVAL: Duration = Duration::from_millis(300);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(2);
const ANDROID_PORT_NAME: &str = "android-usb";

pub struct AndroidUsbBridge {
    inbound: Arc<Mutex<VecDeque<u8>>>,
    outbound: Arc<Mutex<VecDeque<u8>>>,
    connected: Arc<AtomicBool>,
}

impl Clone for AndroidUsbBridge {
    fn clone(&self) -> Self {
        Self {
            inbound: Arc::clone(&self.inbound),
            outbound: Arc::clone(&self.outbound),
            connected: Arc::clone(&self.connected),
        }
    }
}

impl AndroidUsbBridge {
    fn new() -> Self {
        Self {
            inbound: Arc::new(Mutex::new(VecDeque::new())),
            outbound: Arc::new(Mutex::new(VecDeque::new())),
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Release);
    }

    pub fn push_inbound(&self, bytes: &[u8]) {
        if let Ok(mut queue) = self.inbound.lock() {
            queue.extend(bytes.iter().copied());
        }
    }

    pub fn pull_outbound(&self, out: &mut [u8]) -> usize {
        let Ok(mut queue) = self.outbound.lock() else {
            return 0;
        };
        let mut written = 0;
        for slot in out.iter_mut() {
            let Some(byte) = queue.pop_front() else {
                break;
            };
            *slot = byte;
            written += 1;
        }
        written
    }

    fn open_port(&self) -> AndroidUsbPort {
        AndroidUsbPort {
            inbound: Arc::clone(&self.inbound),
            outbound: Arc::clone(&self.outbound),
        }
    }
}

struct AndroidUsbPort {
    inbound: Arc<Mutex<VecDeque<u8>>>,
    outbound: Arc<Mutex<VecDeque<u8>>>,
}

impl Read for AndroidUsbPort {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Ok(mut queue) = self.inbound.lock() else {
            return Ok(0);
        };
        let mut read = 0;
        for slot in buf.iter_mut() {
            let Some(byte) = queue.pop_front() else {
                break;
            };
            *slot = byte;
            read += 1;
        }
        Ok(read)
    }
}

impl Write for AndroidUsbPort {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(mut queue) = self.outbound.lock() {
            queue.extend(buf.iter().copied());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn android_usb_auto_interface(
    id: InterfaceId,
) -> (
    SelfDrivenInterface<impl FnOnce(UsbAutoContext)>,
    AndroidUsbBridge,
) {
    let node_tag = node_tag_for(id);
    let bridge = AndroidUsbBridge::new();
    let worker_bridge = bridge.clone();
    let interface = SelfDrivenInterface::new(host_descriptor(id), move |ctx| {
        thread::spawn(move || serve(ctx, node_tag, worker_bridge));
    });
    (interface, bridge)
}

fn serve(mut ctx: UsbAutoContext, node_tag: NodeTag, bridge: AndroidUsbBridge) {
    let mut discoverer: Discoverer<AndroidUsbPort> =
        Discoverer::new(node_tag, Capabilities::host());
    let port_id = PortId::new(ANDROID_PORT_NAME.to_string());
    let mut last_scan: Option<Instant> = None;

    loop {
        if last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            last_scan = Some(Instant::now());
            let present: &[PortId] = if bridge.connected.load(Ordering::Acquire) {
                std::slice::from_ref(&port_id)
            } else {
                &[]
            };
            discoverer.reconcile_present(present, |_id| Ok(bridge.open_port()));
        }

        let cadence = discoverer.pump(&mut ctx);
        if matches!(ctx.control.next_command(), Some(ControlCommand::Stop)) {
            break;
        }
        if matches!(cadence, PumpCadence::Idle) {
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }
    ctx.control.report(ControlReport::Stopped);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_reach_the_bridge_and_inbound_pushes_reach_the_port() {
        let bridge = AndroidUsbBridge::new();
        let mut port = bridge.open_port();

        port.write_all(&[1, 2, 3, 4]).unwrap();
        let mut out = [0u8; 8];
        assert_eq!(bridge.pull_outbound(&mut out), 4);
        assert_eq!(&out[..4], &[1, 2, 3, 4]);

        bridge.push_inbound(&[9, 8, 7]);
        let mut buf = [0u8; 8];
        assert_eq!(port.read(&mut buf).unwrap(), 3);
        assert_eq!(&buf[..3], &[9, 8, 7]);
    }

    #[test]
    fn an_empty_inbound_reads_as_zero_not_an_error() {
        let bridge = AndroidUsbBridge::new();
        let mut port = bridge.open_port();
        let mut buf = [0u8; 8];
        assert_eq!(port.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn pull_outbound_is_bounded_by_the_caller_buffer() {
        let bridge = AndroidUsbBridge::new();
        let mut port = bridge.open_port();
        port.write_all(&[1, 2, 3, 4, 5]).unwrap();

        let mut small = [0u8; 2];
        assert_eq!(bridge.pull_outbound(&mut small), 2);
        assert_eq!(&small, &[1, 2]);
        assert_eq!(bridge.pull_outbound(&mut small), 2);
        assert_eq!(&small, &[3, 4]);
        assert_eq!(bridge.pull_outbound(&mut small), 1);
        assert_eq!(small[0], 5);
        assert_eq!(bridge.pull_outbound(&mut small), 0);
    }

    #[test]
    fn separate_ports_from_one_bridge_share_the_same_queues() {
        let bridge = AndroidUsbBridge::new();
        let mut writer = bridge.open_port();
        let mut reader = bridge.open_port();

        bridge.push_inbound(&[42]);
        let mut buf = [0u8; 1];
        assert_eq!(reader.read(&mut buf).unwrap(), 1);
        assert_eq!(buf[0], 42);

        writer.write_all(&[7]).unwrap();
        let mut out = [0u8; 1];
        assert_eq!(bridge.pull_outbound(&mut out), 1);
        assert_eq!(out[0], 7);
    }
}
