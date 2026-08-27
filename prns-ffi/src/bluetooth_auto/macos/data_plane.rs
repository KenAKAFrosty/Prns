use core::cell::RefCell;
use core::ffi::c_void;
use core::ptr::NonNull;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::Message;
use objc2_core_bluetooth::CBL2CAPChannel;
use objc2_core_foundation::{
    CFOptionFlags, CFReadStream, CFStreamClientContext, CFStreamEventType, CFWriteStream,
};
use objc2_foundation::{NSInputStream, NSOutputStream};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use prns_core::interfaces::bluetooth_auto::{BLE_HW_MTU, STREAM_FRAME_PREFIX_LEN};

use super::{core_bluetooth_peer_id, CoreBluetoothPeerId};

pub(super) const L2CAP_SDU_LEN: usize = STREAM_FRAME_PREFIX_LEN + BLE_HW_MTU;
const READ_CHUNK: usize = L2CAP_SDU_LEN;
const READ_EVENTS: CFOptionFlags = CFStreamEventType::HasBytesAvailable.0
    | CFStreamEventType::ErrorOccurred.0
    | CFStreamEventType::EndEncountered.0;
const WRITE_EVENTS: CFOptionFlags = CFStreamEventType::CanAcceptBytes.0
    | CFStreamEventType::ErrorOccurred.0
    | CFStreamEventType::EndEncountered.0;
pub(super) struct Outbound {
    pub(super) pending: VecDeque<u8>,
    pub(super) closed: bool,
}

pub(super) struct StreamPump {
    input: Retained<NSInputStream>,
    output: Retained<NSOutputStream>,
    inbound_tx: RefCell<Option<tokio_mpsc::Sender<Box<[u8]>>>>,
    outbound: Arc<Mutex<Outbound>>,
    on_dead: RefCell<Option<(super::SendPeripheralDelegate, CoreBluetoothPeerId)>>,
    dead_fired: AtomicBool,
    _channel: Retained<CBL2CAPChannel>,
}

fn mark_data_plane_dead(pump: &StreamPump) {
    if pump.dead_fired.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Ok(mut out) = pump.outbound.lock() {
        out.closed = true;
    }
    pump.inbound_tx.borrow_mut().take();
    if let Some((delegate, peer_id)) = pump.on_dead.borrow_mut().take() {
        delegate.end_inbound_session(peer_id);
    }
}

#[derive(Clone, Copy)]
pub(super) struct PumpPtr(pub(super) *const StreamPump);
// SAFETY: PumpPtr is only dereferenced by jobs on the owning serial dispatch queue, and PumpHandle
// schedules destruction on that same queue after all cloned handles have been dropped.
unsafe impl Send for PumpPtr {}

struct SendStreamPtr(*mut StreamPump);
// SAFETY: ownership of this pointer is transferred exactly once to the owning serial dispatch
// queue, where it is reconstructed into the original Box and freed.
unsafe impl Send for SendStreamPtr {}

pub(super) struct PumpHandle {
    ptr: *mut StreamPump,
    queue: DispatchRetained<DispatchQueue>,
}
// SAFETY: the handle never directly accesses the pointee; Drop transfers the final raw pointer to
// the owning serial dispatch queue, so moving the handle between threads cannot race the pump.
unsafe impl Send for PumpHandle {}
// SAFETY: shared references only clone/hold the dispatch queue and pointer. All pointee access and
// final destruction are serialized on that queue.
unsafe impl Sync for PumpHandle {}

impl Drop for PumpHandle {
    fn drop(&mut self) {
        let raw = SendStreamPtr(self.ptr);
        self.queue.exec_async(move || {
            let raw = raw;
            // SAFETY: this is the unique pointer produced by Box::into_raw in `wire_l2cap`. The
            // queue owns all stream callbacks; removing both clients before closing the streams
            // guarantees no callback can access the pump after Box::from_raw frees it.
            unsafe {
                let pump = &*raw.0;
                let cf_in = &*(Retained::as_ptr(&pump.input) as *const CFReadStream);
                let cf_out = &*(Retained::as_ptr(&pump.output) as *const CFWriteStream);
                cf_in.set_client(0, None, core::ptr::null_mut());
                cf_out.set_client(0, None, core::ptr::null_mut());
                pump.input.close();
                pump.output.close();
                drop(Box::from_raw(raw.0));
            }
        });
    }
}

pub(super) fn flush(pump: &StreamPump) {
    let Ok(mut out) = pump.outbound.lock() else {
        return;
    };
    if out.closed {
        return;
    }
    while !out.pending.is_empty() && pump.output.hasSpaceAvailable() {
        let (ptr, len) = {
            let (front, _) = out.pending.as_slices();
            (front.as_ptr() as *mut u8, front.len())
        };
        // SAFETY: `front` is non-empty and points into `out.pending`, whose mutex guard remains held
        // for the call, so the non-null buffer is valid for exactly `len` bytes.
        let written = unsafe {
            pump.output
                .write_maxLength(NonNull::new_unchecked(ptr), len)
        };
        if written > 0 {
            out.pending.drain(..written as usize);
        } else {
            if written < 0 {
                crate::diagnostic_log::warn!(
                    "bluetooth: L2CAP write returned {written} — data plane down"
                );
                mark_data_plane_dead(pump);
            }
            break;
        }
    }
}

unsafe extern "C-unwind" fn read_cb(
    _stream: *mut CFReadStream,
    event: CFStreamEventType,
    info: *mut c_void,
) {
    // SAFETY: `wire_l2cap` registers `info` as its Box-backed StreamPump and unregisters the client
    // callbacks on the same serial queue before freeing the allocation.
    let pump = unsafe { &*(info as *const StreamPump) };
    if (event.0 & CFStreamEventType::HasBytesAvailable.0) != 0 {
        let mut buf = [0u8; READ_CHUNK];
        while pump.input.hasBytesAvailable() {
            // SAFETY: `buf.as_mut_ptr()` is non-null and valid for READ_CHUNK writable bytes for the
            // duration of the synchronous NSInputStream read.
            let read = unsafe {
                pump.input
                    .read_maxLength(NonNull::new_unchecked(buf.as_mut_ptr()), READ_CHUNK)
            };
            if read > 0 {
                let accepted = pump
                    .inbound_tx
                    .borrow()
                    .as_ref()
                    .is_some_and(|tx| tx.try_send(Box::from(&buf[..read as usize])).is_ok());
                if !accepted {
                    mark_data_plane_dead(pump);
                    break;
                }
            } else {
                break;
            }
        }
    }
    if (event.0 & (CFStreamEventType::ErrorOccurred.0 | CFStreamEventType::EndEncountered.0)) != 0 {
        crate::diagnostic_log::warn!(
            "bluetooth: L2CAP read stream closed/errored — inbound data plane down"
        );
        mark_data_plane_dead(pump);
    }
}

unsafe extern "C-unwind" fn write_cb(
    _stream: *mut CFWriteStream,
    event: CFStreamEventType,
    info: *mut c_void,
) {
    // SAFETY: `wire_l2cap` registers `info` as its Box-backed StreamPump and unregisters the client
    // callbacks on the same serial queue before freeing the allocation.
    let pump = unsafe { &*(info as *const StreamPump) };
    if (event.0 & CFStreamEventType::CanAcceptBytes.0) != 0 {
        flush(pump);
    }
    if (event.0 & (CFStreamEventType::ErrorOccurred.0 | CFStreamEventType::EndEncountered.0)) != 0 {
        crate::diagnostic_log::warn!(
            "bluetooth: L2CAP write stream closed/errored — outbound data plane down"
        );
        mark_data_plane_dead(pump);
    }
}

pub(super) fn wire_l2cap(
    channel: &CBL2CAPChannel,
    queue: &DispatchRetained<DispatchQueue>,
    on_dead: impl FnOnce(CoreBluetoothPeerId) -> Option<(super::SendPeripheralDelegate, CoreBluetoothPeerId)>,
) -> Option<(CoreBluetoothPeerId, DataPlane)> {
    // SAFETY: CoreBluetooth supplied a live retained channel, and its accessors return retained
    // objects whose runtime types match the generated bindings.
    let (peer_id, input, output) = unsafe {
        let peer = channel.peer()?;
        (
            core_bluetooth_peer_id(&peer),
            channel.inputStream()?,
            channel.outputStream()?,
        )
    };
    let (inbound_tx, inbound_rx) = tokio_mpsc::channel::<Box<[u8]>>(16);
    let outbound = Arc::new(Mutex::new(Outbound {
        pending: VecDeque::new(),
        closed: false,
    }));
    let on_dead = on_dead(peer_id);
    let pump = Box::into_raw(Box::new(StreamPump {
        input,
        output,
        inbound_tx: RefCell::new(Some(inbound_tx)),
        outbound: outbound.clone(),
        on_dead: RefCell::new(on_dead),
        dead_fired: AtomicBool::new(false),
        _channel: channel.retain(),
    }));
    // SAFETY: `pump` is a live Box allocation kept by PumpHandle. NSInputStream/NSOutputStream are
    // toll-free bridged to the corresponding CFStream types. The client context pointer stays live
    // until PumpHandle unregisters both callbacks on this same serial queue before freeing it.
    unsafe {
        let pump_ref = &*pump;
        let cf_in = &*(Retained::as_ptr(&pump_ref.input) as *const CFReadStream);
        let cf_out = &*(Retained::as_ptr(&pump_ref.output) as *const CFWriteStream);
        let mut ctx = CFStreamClientContext {
            version: 0,
            info: pump as *mut c_void,
            retain: None,
            release: None,
            copyDescription: None,
        };
        cf_in.set_client(READ_EVENTS, Some(read_cb), &mut ctx);
        cf_in.set_dispatch_queue(Some(&**queue));
        cf_out.set_client(WRITE_EVENTS, Some(write_cb), &mut ctx);
        cf_out.set_dispatch_queue(Some(&**queue));
        pump_ref.input.open();
        pump_ref.output.open();
    }
    Some((
        peer_id,
        DataPlane {
            inbound_rx,
            outbound,
            queue: queue.clone(),
            pump_ptr: PumpPtr(pump),
            pump: Arc::new(PumpHandle {
                ptr: pump,
                queue: queue.clone(),
            }),
        },
    ))
}

pub(super) struct DataPlane {
    pub(super) inbound_rx: tokio_mpsc::Receiver<Box<[u8]>>,
    pub(super) outbound: Arc<Mutex<Outbound>>,
    pub(super) queue: DispatchRetained<DispatchQueue>,
    pub(super) pump_ptr: PumpPtr,
    pub(super) pump: Arc<PumpHandle>,
}

const MAX_BUFFERED_L2CAP: usize = 4;

#[derive(Default)]
pub(super) struct PendingL2cap {
    waiters: VecDeque<oneshot::Sender<DataPlane>>,
    ready: VecDeque<DataPlane>,
}

impl PendingL2cap {
    pub(super) fn deliver(&mut self, mut data: DataPlane) {
        while let Some(tx) = self.waiters.pop_front() {
            match tx.send(data) {
                Ok(()) => return,
                Err(returned) => data = returned,
            }
        }
        self.ready.push_back(data);
        while self.ready.len() > MAX_BUFFERED_L2CAP {
            self.ready.pop_front();
        }
    }

    pub(super) fn arm(&mut self, tx: oneshot::Sender<DataPlane>) {
        match self.ready.pop_front() {
            Some(data) => {
                let _ = tx.send(data);
            }
            None => self.waiters.push_back(tx),
        }
    }
}
