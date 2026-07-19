use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2_core_bluetooth::CBCharacteristicWriteType;
use objc2_foundation::NSData;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use prns_core::interfaces::bluetooth_auto::core::{
    encode_stream_frame, fragments_of, BleAddress, Control, Dialect, Fragment, L2capPlan,
    Reassembler, StreamDeframer, BLE_HW_MTU, CONTROL_MAX_LEN,
};
use prns_core::interfaces::bluetooth_auto::seam::{BleLink, BleSink, BleSource};

use super::data_plane::{flush, DataPlane, Outbound, PumpHandle, PumpPtr, L2CAP_SDU_LEN};
use super::{
    MacosBleError, SendCharacteristic, SendCharacteristicRef, SendPeripheral,
    SendPeripheralDelegate, SendPeripheralManager,
};

const GATT_REASSEMBLY_CAP: usize = BLE_HW_MTU;
const GATT_FRAGMENT_PAYLOAD: usize = 180;
const GATT_FRAGMENT_BUF: usize = 256;
pub(super) enum ControlPlane {
    Listener {
        manager: SendPeripheralManager,
        characteristic: SendCharacteristic,
        data_characteristic: SendCharacteristic,
        delegate: SendPeripheralDelegate,
    },
    Central {
        peripheral: SendPeripheral,
        characteristic: SendCharacteristicRef,
        data_characteristic: Option<SendCharacteristicRef>,
        peripheral_manager: SendPeripheralDelegate,
    },
}

enum GattWriter {
    Central {
        peripheral: SendPeripheral,
        characteristic: SendCharacteristicRef,
    },
    Listener {
        manager: SendPeripheralManager,
        characteristic: SendCharacteristic,
    },
}

impl GattWriter {
    fn send(&self, frame: &[u8]) -> Result<(), MacosBleError> {
        let mut buf = [0u8; GATT_FRAGMENT_BUF];
        for fragment in fragments_of(frame, GATT_FRAGMENT_PAYLOAD) {
            let len = fragment
                .encode(&mut buf)
                .ok_or(MacosBleError::FrameTooLarge)?;
            let data = NSData::with_bytes(&buf[..len]);
            match self {
                GattWriter::Central {
                    peripheral,
                    characteristic,
                } => unsafe {
                    peripheral.0.writeValue_forCharacteristic_type(
                        &data,
                        &characteristic.0,
                        CBCharacteristicWriteType::WithoutResponse,
                    );
                },
                GattWriter::Listener {
                    manager,
                    characteristic,
                } => {
                    let sent = unsafe {
                        manager
                            .0
                            .updateValue_forCharacteristic_onSubscribedCentrals(
                                &data,
                                &characteristic.0,
                                None,
                            )
                    };
                    if !sent {
                        crate::diagnostic_log::warn!(
                            "bluetooth: GATT-data notify queue full — fragment dropped, peer will retransmit"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

pub struct GattLink {
    pub(super) control: ControlPlane,
    pub(super) control_rx: tokio_mpsc::UnboundedReceiver<Control>,
    pub(super) address: BleAddress,
    pub(super) data_inbound_rx: Option<tokio_mpsc::UnboundedReceiver<Box<[u8]>>>,
    pub(super) l2cap_pending: Option<oneshot::Receiver<DataPlane>>,
}

impl BleLink for GattLink {
    type Error = MacosBleError;
    type Source = GattSource;
    type Sink = GattSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), MacosBleError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg.encode(&mut buf).ok_or(MacosBleError::ControlTooLarge)?;
        let data = NSData::with_bytes(&buf[..len]);
        match &self.control {
            ControlPlane::Listener {
                manager,
                characteristic,
                ..
            } => {
                let sent = unsafe {
                    manager
                        .0
                        .updateValue_forCharacteristic_onSubscribedCentrals(
                            &data,
                            &characteristic.0,
                            None,
                        )
                };
                if sent {
                    crate::diagnostic_log::debug!(
                        "bluetooth: {:02x?} -> {msg:?}",
                        self.address.octets()
                    );
                    Ok(())
                } else {
                    crate::diagnostic_log::warn!(
                        "bluetooth: {:02x?} notify failed — control PDU did not reach the central, handshake will stall",
                        self.address.octets()
                    );
                    Err(MacosBleError::NotifyFailed)
                }
            }
            ControlPlane::Central {
                peripheral,
                characteristic,
                ..
            } => {
                let max = unsafe {
                    peripheral
                        .0
                        .maximumWriteValueLengthForType(CBCharacteristicWriteType::WithResponse)
                };
                if max < len {
                    crate::diagnostic_log::warn!(
                        "bluetooth: {:02x?} control write {len}B exceeds max single write {max}B (negotiated ATT MTU is small) — CoreBluetooth will use a long/prepared write; the peer GATT server must reassemble it",
                        self.address.octets()
                    );
                } else {
                    crate::diagnostic_log::debug!(
                        "bluetooth: {:02x?} control write {len}B fits one ATT packet (max {max}B)",
                        self.address.octets()
                    );
                }
                unsafe {
                    peripheral.0.writeValue_forCharacteristic_type(
                        &data,
                        &characteristic.0,
                        CBCharacteristicWriteType::WithResponse,
                    )
                };
                crate::diagnostic_log::debug!(
                    "bluetooth: {:02x?} -> {msg:?}",
                    self.address.octets()
                );
                Ok(())
            }
        }
    }

    async fn control_recv(&mut self) -> Result<Control, MacosBleError> {
        let control = self.control_rx.recv().await.ok_or(MacosBleError::Closed)?;
        crate::diagnostic_log::debug!("bluetooth: {:02x?} <- {control:?}", self.address.octets());
        Ok(control)
    }

    async fn upgrade(&mut self, plan: &L2capPlan) -> Result<(), MacosBleError> {
        match plan {
            L2capPlan::Accept => {
                let (tx, rx) = oneshot::channel::<DataPlane>();
                match &self.control {
                    ControlPlane::Central {
                        peripheral_manager, ..
                    } => peripheral_manager.0.arm_pending_channel(tx),
                    ControlPlane::Listener { delegate, .. } => delegate.0.arm_pending_channel(tx),
                };
                self.l2cap_pending = Some(rx);
                crate::diagnostic_log::debug!(
                    "bluetooth: {:02x?} armed the L2CAP acceptor — the peer's CoC will upgrade the live GATT-floor link in the background",
                    self.address.octets()
                );
                Ok(())
            }
            L2capPlan::Open { .. } => {
                crate::diagnostic_log::warn!(
                    "bluetooth: {:02x?} asked to open a CoC, but the macOS backend is acceptor-only (a central-side open bonds) — staying on the GATT floor",
                    self.address.octets()
                );
                Ok(())
            }
            L2capPlan::None => Ok(()),
        }
    }

    fn into_data(self) -> (GattSource, GattSink) {
        let (merged_tx, merged_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();

        if let Some(mut inbound_rx) = self.data_inbound_rx {
            let frames = merged_tx.clone();
            tokio::spawn(async move {
                let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
                while let Some(message) = inbound_rx.recv().await {
                    let Some(fragment) = Fragment::decode(&message) else {
                        continue;
                    };
                    if let Some(frame) = reassembler.absorb(&fragment) {
                        if frames.send(Box::from(frame)).is_err() {
                            break;
                        }
                    }
                }
            });
        }

        let l2cap_pending = self.l2cap_pending.map(|pending| {
            let (write_tx, write_rx) = oneshot::channel::<L2capWriteHalf>();
            let frames = merged_tx.clone();
            tokio::spawn(async move {
                let Ok(data) = pending.await else {
                    return;
                };
                crate::diagnostic_log::debug!("bluetooth: L2CAP fast lane up — data now rides the channel, GATT stays the floor");
                let DataPlane {
                    mut inbound_rx,
                    outbound,
                    queue,
                    pump_ptr,
                    pump,
                } = data;
                let _ = write_tx.send(L2capWriteHalf {
                    outbound,
                    queue,
                    pump_ptr,
                    _pump: pump.clone(),
                });
                let _read_pump = pump;
                let mut deframer = StreamDeframer::<{ 2 * L2CAP_SDU_LEN }>::new();
                let mut frame = std::vec![0u8; 2 * L2CAP_SDU_LEN];
                while let Some(chunk) = inbound_rx.recv().await {
                    if !deframer.absorb(&chunk) {
                        break;
                    }
                    while let Some(len) = deframer.next_frame(&mut frame) {
                        if frames.send(Box::from(&frame[..len])).is_err() {
                            return;
                        }
                    }
                }
            });
            write_rx
        });

        drop(merged_tx);
        (
            GattSource { inbound: merged_rx },
            GattSink {
                gatt: gatt_writer(&self.control),
                l2cap: None,
                l2cap_pending,
            },
        )
    }
}

fn gatt_writer(control: &ControlPlane) -> Option<GattWriter> {
    match control {
        ControlPlane::Central {
            peripheral,
            data_characteristic: Some(data_characteristic),
            ..
        } => Some(GattWriter::Central {
            peripheral: SendPeripheral(peripheral.0.clone()),
            characteristic: SendCharacteristicRef(data_characteristic.0.clone()),
        }),
        ControlPlane::Central {
            data_characteristic: None,
            ..
        } => None,
        ControlPlane::Listener {
            manager,
            data_characteristic,
            ..
        } => Some(GattWriter::Listener {
            manager: SendPeripheralManager(manager.0.clone()),
            characteristic: SendCharacteristic(data_characteristic.0.clone()),
        }),
    }
}

pub struct GattSource {
    inbound: tokio_mpsc::UnboundedReceiver<Box<[u8]>>,
}

impl BleSource for GattSource {
    type Error = MacosBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, MacosBleError> {
        let frame = self.inbound.recv().await.ok_or(MacosBleError::Closed)?;
        let len = frame.len().min(out.len());
        out[..len].copy_from_slice(&frame[..len]);
        Ok(len)
    }
}

struct L2capWriteHalf {
    outbound: Arc<Mutex<Outbound>>,
    queue: DispatchRetained<DispatchQueue>,
    pump_ptr: PumpPtr,
    _pump: Arc<PumpHandle>,
}

impl L2capWriteHalf {
    fn send(&self, frame: &[u8]) -> Result<(), MacosBleError> {
        let mut framed = [0u8; L2CAP_SDU_LEN];
        let len = encode_stream_frame(frame, &mut framed).ok_or(MacosBleError::FrameTooLarge)?;
        {
            let Ok(mut out) = self.outbound.lock() else {
                return Err(MacosBleError::Closed);
            };
            if out.closed {
                return Err(MacosBleError::Closed);
            }
            out.pending.extend(framed[..len].iter().copied());
        }
        let ptr = self.pump_ptr;
        self.queue.exec_async(move || {
            let ptr = ptr;
            flush(unsafe { &*ptr.0 });
        });
        Ok(())
    }
}

pub struct GattSink {
    gatt: Option<GattWriter>,
    l2cap: Option<L2capWriteHalf>,
    l2cap_pending: Option<oneshot::Receiver<L2capWriteHalf>>,
}

impl BleSink for GattSink {
    type Error = MacosBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), MacosBleError> {
        if self.l2cap.is_none() {
            if let Some(pending) = self.l2cap_pending.as_mut() {
                match pending.try_recv() {
                    Ok(half) => {
                        self.l2cap = Some(half);
                        self.l2cap_pending = None;
                    }
                    Err(oneshot::error::TryRecvError::Closed) => self.l2cap_pending = None,
                    Err(oneshot::error::TryRecvError::Empty) => {}
                }
            }
        }
        if let Some(l2cap) = &self.l2cap {
            match l2cap.send(frame) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    self.l2cap = None;
                    if self.gatt.is_none() {
                        return Err(err);
                    }
                    crate::diagnostic_log::warn!(
                        "bluetooth: L2CAP send failed — the fast lane is down, frames fall back to the GATT floor"
                    );
                }
            }
        }
        if let Some(gatt) = &self.gatt {
            return gatt.send(frame);
        }
        Ok(())
    }
}
