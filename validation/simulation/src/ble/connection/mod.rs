use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use super::BleAddress;

mod data;
mod index;

pub(in crate::ble) use data::ConnectionSide;
pub(in crate::ble) use data::DataEvent;
pub use data::{BleConnectionDataSnapshot, BleDataCounters, BleDataSendObservation};

pub(super) use index::ConnectionIndex;

pub(super) struct Connection {
    addresses: [BleAddress; 2],
    closed: watch::Sender<bool>,
    data: Mutex<[data::DirectionActivity; 2]>,
    capture: Option<super::wire_capture::CapturedConnection>,
}

impl Connection {
    pub(super) fn new(first: BleAddress, second: BleAddress) -> Self {
        let (closed, _) = watch::channel(false);
        Self {
            addresses: [first, second],
            closed,
            data: Mutex::new(Default::default()),
            capture: None,
        }
    }

    pub(super) fn with_wire_capture(
        mut self,
        capture: super::wire_capture::CapturedConnection,
    ) -> Self {
        self.capture = Some(capture);
        self
    }

    pub(super) fn connects(&self, address: BleAddress) -> bool {
        self.addresses.contains(&address)
    }

    pub(in crate::ble) fn data_snapshot(&self) -> BleConnectionDataSnapshot {
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        BleConnectionDataSnapshot {
            dialer: self.addresses[0],
            listener: self.addresses[1],
            dialer_to_listener: data[0].counters,
            listener_to_dialer: data[1].counters,
            dialer_last_send: data[0].last_send.clone(),
            listener_last_send: data[1].last_send.clone(),
        }
    }

    pub(super) fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<bool> {
        self.closed.subscribe()
    }

    pub(super) fn close(&self) -> bool {
        self.closed.send_if_modified(|closed| {
            let changed = !*closed;
            *closed = true;
            changed
        })
    }
}

pub(super) struct ConnectionEndpoint {
    pub(super) connection: Arc<Connection>,
    pub(super) side: ConnectionSide,
}

impl ConnectionEndpoint {
    pub(in crate::ble) fn capture(&self, channel: super::BleWireChannel, bytes: &[u8]) {
        if let Some(capture) = &self.connection.capture {
            capture.record(
                self.connection.addresses[self.side.outgoing()],
                self.connection.addresses[self.side.incoming()],
                channel,
                bytes,
            );
        }
    }
    pub(in crate::ble) fn start_send(&self, frame: &[u8]) {
        let mut data = self
            .connection
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let activity = &mut data[self.side.outgoing()];
        activity.last_send = Some(BleDataSendObservation {
            header: personal_rns::wire::WirePacketHeader::parse(frame)
                .ok()
                .map(|(header, _)| header),
            frame_length: frame.len(),
            before: activity.counters,
        });
        activity.counters.record(DataEvent::SendStarted);
    }

    pub(in crate::ble) fn outgoing(&self, event: DataEvent) {
        self.connection
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[self.side.outgoing()]
        .counters
        .record(event);
    }

    pub(in crate::ble) fn incoming(&self, event: DataEvent) {
        self.connection
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[self.side.incoming()]
        .counters
        .record(event);
    }
}

impl Drop for ConnectionEndpoint {
    fn drop(&mut self) {
        let _ = self.connection.close();
    }
}
