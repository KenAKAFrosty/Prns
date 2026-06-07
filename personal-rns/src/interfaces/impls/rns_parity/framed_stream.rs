//WIP NEEDS REVIEW
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::Duration;

use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use crate::interfaces::{ControlCommand, ControlEndpoint, InboundSink, OutboundDrain};

const MTU: usize = crate::wire::MTU;
const READ_CHUNK: usize = 4096;
const WRITE_PARK_FALLBACK: Duration = Duration::from_millis(250);

pub(crate) enum ConnectionEnd {
    Stopped,
    Disconnected,
}

pub(crate) fn read_loop<R: Read, I: InboundSink>(
    mut reader: R,
    inbound: &mut I,
    connection_dead: &AtomicBool,
    wake: &SyncSender<()>,
) {
    let mut decoder = RnsSerialDecoder::<MTU>::new();
    let mut read_buf = [0u8; READ_CHUNK];

    loop {
        match reader.read(&mut read_buf) {
            Ok(0) => break,
            Ok(n) => {
                decoder.feed_slice(&read_buf[..n], |frame| {
                    if !frame.is_empty() {
                        let _ = inbound.submit(|buf| {
                            buf[..frame.len()].copy_from_slice(frame);
                            frame.len()
                        });
                    }
                });
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }

    connection_dead.store(true, Ordering::Release);
    let _ = wake.try_send(());
}

pub(crate) fn write_loop<W: Write, O: OutboundDrain, C: ControlEndpoint>(
    writer: &mut W,
    outbound: &mut O,
    control: &mut C,
    wake_rx: &Receiver<()>,
    connection_dead: &AtomicBool,
) -> ConnectionEnd {
    let mut frame_buf = [0u8; rns_serial_framing::max_encoded_len(MTU)];

    loop {
        if matches!(control.next_command(), Some(ControlCommand::Stop)) {
            return ConnectionEnd::Stopped;
        }
        if connection_dead.load(Ordering::Acquire) {
            return ConnectionEnd::Disconnected;
        }

        let mut write_failed = false;
        outbound.drain_each(|packet| {
            if write_failed {
                return;
            }
            if let Ok(n) = rns_serial_framing::encode(packet.bytes, &mut frame_buf) {
                if writer
                    .write_all(&frame_buf[..n])
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    write_failed = true;
                }
            }
        });
        if write_failed {
            return ConnectionEnd::Disconnected;
        }

        let _ = wake_rx.recv_timeout(WRITE_PARK_FALLBACK);
    }
}
