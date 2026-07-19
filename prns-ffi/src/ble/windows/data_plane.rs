use std::sync::{Arc, Mutex};

use prns_core::interfaces::bluetooth_auto::core::{
    fragments_of, BleAddress, Control, Dialect, Fragment, L2capPlan, Reassembler, BLE_HW_MTU,
    CONTROL_MAX_LEN,
};
use prns_core::interfaces::bluetooth_auto::seam::{BleLink, BleSink, BleSource};
use tokio::sync::{mpsc as tokio_mpsc, watch};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattDeviceService, GattLocalCharacteristic, GattSession,
    GattSubscribedClient, GattWriteOption,
};
use windows::Devices::Bluetooth::{
    BluetoothLEDevice, BluetoothLEPreferredConnectionParameters,
    BluetoothLEPreferredConnectionParametersRequest,
};

use super::central::gatt_write;
use super::peripheral::notify_local;
use super::WindowsBleError;

pub(super) const GATT_FRAGMENT_PAYLOAD: usize = 180;
const GATT_REASSEMBLY_CAP: usize = BLE_HW_MTU;
const GATT_FRAGMENT_OVERHEAD: usize = 8;

fn central_fragment_payload(session: &GattSession) -> usize {
    match session.MaxPduSize() {
        Ok(pdu) => {
            let payload = (pdu as usize)
                .saturating_sub(GATT_FRAGMENT_OVERHEAD)
                .clamp(GATT_FRAGMENT_PAYLOAD, BLE_HW_MTU);
            crate::diagnostic_log::debug!(
                "bluetooth: negotiated MaxPduSize={pdu}, data fragment payload={payload}"
            );
            payload
        }
        Err(_) => GATT_FRAGMENT_PAYLOAD,
    }
}

pub(super) fn request_throughput(
    device: &BluetoothLEDevice,
) -> Option<BluetoothLEPreferredConnectionParametersRequest> {
    let preferred = BluetoothLEPreferredConnectionParameters::ThroughputOptimized().ok()?;
    match device.RequestPreferredConnectionParameters(&preferred) {
        Ok(request) => {
            crate::diagnostic_log::debug!(
                "bluetooth: requested throughput-optimized connection parameters"
            );
            Some(request)
        }
        Err(error) => {
            crate::diagnostic_log::debug!(
                "bluetooth: connection-parameter request rejected ({error:?})"
            );
            None
        }
    }
}

pub(super) type ClientSlot = Arc<Mutex<Option<GattSubscribedClient>>>;

pub(super) enum LinkPlane {
    Central {
        control_char: GattCharacteristic,
        data_char: GattCharacteristic,
        device: BluetoothLEDevice,
        service: GattDeviceService,
        session: GattSession,
        connection_request: Option<BluetoothLEPreferredConnectionParametersRequest>,
    },
    Peripheral {
        control_char: GattLocalCharacteristic,
        data_char: GattLocalCharacteristic,
        control_client: ClientSlot,
        data_client: ClientSlot,
    },
}

pub struct WinGattLink {
    pub(super) address: BleAddress,
    pub(super) control_rx: tokio_mpsc::UnboundedReceiver<Control>,
    pub(super) data_rx: Option<tokio_mpsc::UnboundedReceiver<Box<[u8]>>>,
    pub(super) closed: watch::Receiver<bool>,
    pub(super) plane: LinkPlane,
}

impl BleLink for WinGattLink {
    type Error = WindowsBleError;
    type Source = WinGattSource;
    type Sink = WinGattSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        self.address
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), WindowsBleError> {
        let mut buf = [0u8; CONTROL_MAX_LEN];
        let len = msg
            .encode(&mut buf)
            .ok_or(WindowsBleError::ControlTooLarge)?;
        let bytes = buf
            .get(..len)
            .ok_or(WindowsBleError::ControlTooLarge)?
            .to_vec();
        match &self.plane {
            LinkPlane::Central { control_char, .. } => {
                gatt_write(
                    control_char.clone(),
                    bytes,
                    GattWriteOption::WriteWithResponse,
                )
                .await?;
            }
            LinkPlane::Peripheral {
                control_char,
                control_client,
                ..
            } => {
                notify_local(control_char.clone(), control_client.clone(), bytes).await?;
            }
        }
        crate::diagnostic_log::debug!("bluetooth: {:02x?} -> {msg:?}", self.address.octets());
        Ok(())
    }

    async fn control_recv(&mut self) -> Result<Control, WindowsBleError> {
        if *self.closed.borrow() {
            return Err(WindowsBleError::Closed);
        }
        let control = tokio::select! {
            msg = self.control_rx.recv() => msg.ok_or(WindowsBleError::Closed)?,
            _ = self.closed.changed() => return Err(WindowsBleError::Closed),
        };
        crate::diagnostic_log::debug!("bluetooth: {:02x?} <- {control:?}", self.address.octets());
        Ok(control)
    }

    async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), WindowsBleError> {
        // GATT-only: WinRT has no app-level L2CAP, so the upgrade is a permanent no-op. The floor
        // carries every frame; never failing keeps the link alive (the seam contract for upgrade).
        Ok(())
    }

    fn into_data(self) -> (WinGattSource, WinGattSink) {
        let (merged_tx, merged_rx) = tokio_mpsc::unbounded_channel::<Box<[u8]>>();
        if let Some(mut inbound_rx) = self.data_rx {
            tokio::spawn(async move {
                let mut reassembler = Reassembler::<GATT_REASSEMBLY_CAP>::new();
                while let Some(message) = inbound_rx.recv().await {
                    let Some(fragment) = Fragment::decode(&message) else {
                        crate::diagnostic_log::warn!(
                            "bluetooth: data fragment decode failed ({} bytes)",
                            message.len()
                        );
                        continue;
                    };
                    if let Some(frame) = reassembler.absorb(&fragment) {
                        crate::diagnostic_log::debug!(
                            "bluetooth: reassembled data frame {} bytes",
                            frame.len()
                        );
                        if merged_tx.send(Box::from(frame)).is_err() {
                            break;
                        }
                    }
                }
            });
        }
        let (keepalive, sink_plane, fragment_payload) = match self.plane {
            LinkPlane::Central {
                data_char,
                device,
                service,
                session,
                connection_request,
                ..
            } => {
                let sink_session = session.clone();
                let fragment_payload = central_fragment_payload(&session);
                (
                    SourceKeepalive::Central {
                        _device: device,
                        _service: service,
                        _session: session,
                        _connection_request: connection_request,
                    },
                    SinkPlane::Central {
                        data_char,
                        _session: sink_session,
                    },
                    fragment_payload,
                )
            }
            LinkPlane::Peripheral {
                data_char,
                data_client,
                ..
            } => (
                SourceKeepalive::Peripheral,
                SinkPlane::Peripheral {
                    data_char,
                    data_client,
                },
                GATT_FRAGMENT_PAYLOAD,
            ),
        };
        (
            WinGattSource {
                inbound: merged_rx,
                closed: self.closed,
                _keepalive: keepalive,
            },
            WinGattSink {
                plane: sink_plane,
                address: self.address,
                fragment_payload,
            },
        )
    }
}

enum SourceKeepalive {
    Central {
        _device: BluetoothLEDevice,
        _service: GattDeviceService,
        _session: GattSession,
        _connection_request: Option<BluetoothLEPreferredConnectionParametersRequest>,
    },
    Peripheral,
}

enum SinkPlane {
    Central {
        data_char: GattCharacteristic,
        _session: GattSession,
    },
    Peripheral {
        data_char: GattLocalCharacteristic,
        data_client: ClientSlot,
    },
}

pub struct WinGattSource {
    inbound: tokio_mpsc::UnboundedReceiver<Box<[u8]>>,
    closed: watch::Receiver<bool>,
    _keepalive: SourceKeepalive,
}

impl BleSource for WinGattSource {
    type Error = WindowsBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, WindowsBleError> {
        if *self.closed.borrow() {
            return Err(WindowsBleError::Closed);
        }
        let frame = tokio::select! {
            frame = self.inbound.recv() => frame.ok_or(WindowsBleError::Closed)?,
            _ = self.closed.changed() => return Err(WindowsBleError::Closed),
        };
        let len = frame.len().min(out.len());
        let dst = out.get_mut(..len).ok_or(WindowsBleError::FrameTooLarge)?;
        let src = frame.get(..len).ok_or(WindowsBleError::FrameTooLarge)?;
        dst.copy_from_slice(src);
        Ok(len)
    }
}

pub struct WinGattSink {
    plane: SinkPlane,
    address: BleAddress,
    fragment_payload: usize,
}

impl BleSink for WinGattSink {
    type Error = WindowsBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), WindowsBleError> {
        for fragment in fragments_of(frame, self.fragment_payload) {
            let mut buf = vec![0u8; self.fragment_payload + FRAGMENT_SCRATCH];
            let len = fragment
                .encode(&mut buf)
                .ok_or(WindowsBleError::FrameTooLarge)?;
            buf.truncate(len);
            let bytes = buf;
            match &self.plane {
                SinkPlane::Central { data_char, .. } => {
                    gatt_write(
                        data_char.clone(),
                        bytes,
                        GattWriteOption::WriteWithoutResponse,
                    )
                    .await?;
                }
                SinkPlane::Peripheral {
                    data_char,
                    data_client,
                } => {
                    notify_local(data_char.clone(), data_client.clone(), bytes).await?;
                }
            }
        }
        Ok(())
    }
}

pub(super) const FRAGMENT_SCRATCH: usize = 8;
