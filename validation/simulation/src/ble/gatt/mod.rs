use std::fmt;
use std::sync::Arc;

use personal_rns::interfaces::bluetooth_auto::{
    copy_received_frame, fragments_of, BleReceiveError, BleSink, BleSource, Fragment, Reassembler,
    BLE_HW_MTU, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN,
};
use tokio::sync::mpsc;

use super::connection::ConnectionEndpoint;
use super::VirtualBleError;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualGattConfig {
    pub(super) control_value_limit: usize,
    pub(super) data_value_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualGattConfigError {
    ZeroControlValueLimit,
    DataValueLimitTooSmall { requested: usize, minimum: usize },
}

impl fmt::Display for VirtualGattConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroControlValueLimit => formatter.write_str("control value limit must be nonzero"),
            Self::DataValueLimitTooSmall { requested, minimum } => write!(formatter,
                "data value limit {requested} is smaller than the fragment header plus one byte ({minimum})"),
        }
    }
}

impl std::error::Error for VirtualGattConfigError {}

impl VirtualGattConfig {
    /// Limits are complete characteristic values, including any fragment header.
    pub fn new(
        control_value_limit: usize,
        data_value_limit: usize,
    ) -> Result<Self, VirtualGattConfigError> {
        if control_value_limit == 0 {
            return Err(VirtualGattConfigError::ZeroControlValueLimit);
        }
        if data_value_limit <= FRAGMENT_HEADER_LEN {
            return Err(VirtualGattConfigError::DataValueLimitTooSmall {
                requested: data_value_limit,
                minimum: FRAGMENT_HEADER_LEN + 1,
            });
        }
        Ok(Self {
            control_value_limit,
            data_value_limit,
        })
    }

    pub(super) fn negotiated(self, peer: Self) -> Self {
        Self {
            control_value_limit: self.control_value_limit.min(peer.control_value_limit),
            data_value_limit: self.data_value_limit.min(peer.data_value_limit),
        }
    }
}

pub(super) struct ControlValue {
    bytes: [u8; CONTROL_MAX_LEN],
    len: usize,
}

impl ControlValue {
    pub(super) fn new(bytes: &[u8], maximum: usize) -> Result<Self, VirtualBleError> {
        let maximum = maximum.min(CONTROL_MAX_LEN);
        if bytes.len() > maximum {
            return Err(VirtualBleError::ControlValueTooLong {
                length: bytes.len(),
                maximum,
            });
        }
        let mut value = Self {
            bytes: [0; CONTROL_MAX_LEN],
            len: bytes.len(),
        };
        value.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(value)
    }

    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub struct VirtualBleSource {
    receiver: mpsc::Receiver<Vec<u8>>,
    reassembler: Reassembler<BLE_HW_MTU>,
    endpoint: Arc<ConnectionEndpoint>,
}

impl VirtualBleSource {
    pub(super) fn new(
        receiver: mpsc::Receiver<Vec<u8>>,
        endpoint: Arc<ConnectionEndpoint>,
    ) -> Self {
        Self {
            receiver,
            reassembler: Reassembler::new(),
            endpoint,
        }
    }
}

impl BleSource for VirtualBleSource {
    type Error = VirtualBleError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        let mut closed = self.endpoint.connection.subscribe();
        loop {
            if *closed.borrow_and_update() {
                return Err(VirtualBleError::LinkClosed);
            }
            let value = tokio::select! {
                biased;
                _ = closed.changed() => return Err(VirtualBleError::LinkClosed),
                value = self.receiver.recv() => value.ok_or(VirtualBleError::LinkClosed)?,
            };
            let Some(fragment) = Fragment::decode(&value) else {
                continue;
            };
            let Some(frame) = self.reassembler.absorb(&fragment) else {
                continue;
            };
            return copy_received_frame(frame, out).map_err(|error| match error {
                BleReceiveError::BufferTooSmall { length, capacity } => {
                    VirtualBleError::ReceiveBufferTooSmall {
                        frame: length,
                        buffer: capacity,
                    }
                }
            });
        }
    }
}

pub struct VirtualBleSink {
    sender: mpsc::Sender<Vec<u8>>,
    maximum_frame_length: usize,
    value_limit: usize,
    endpoint: Arc<ConnectionEndpoint>,
}

impl VirtualBleSink {
    pub(super) fn new(
        sender: mpsc::Sender<Vec<u8>>,
        maximum_frame_length: usize,
        value_limit: usize,
        endpoint: Arc<ConnectionEndpoint>,
    ) -> Self {
        Self {
            sender,
            maximum_frame_length,
            value_limit,
            endpoint,
        }
    }
}

impl BleSink for VirtualBleSink {
    type Error = VirtualBleError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        if frame.is_empty() {
            return Err(VirtualBleError::EmptyFrame);
        }
        if frame.len() > self.maximum_frame_length {
            return Err(VirtualBleError::FrameTooLong {
                length: frame.len(),
                maximum: self.maximum_frame_length,
            });
        }
        let mut closed = self.endpoint.connection.subscribe();
        let mut value = [0; BLE_HW_MTU + FRAGMENT_HEADER_LEN];
        for fragment in fragments_of(frame, self.value_limit) {
            if *closed.borrow_and_update() {
                return Err(VirtualBleError::LinkClosed);
            }
            let len = fragment
                .encode(&mut value)
                .ok_or(VirtualBleError::FragmentEncodingFailed)?;
            tokio::select! {
                biased;
                _ = closed.changed() => return Err(VirtualBleError::LinkClosed),
                result = self.sender.send(value[..len].to_vec()) => {
                    result.map_err(|_| VirtualBleError::LinkClosed)?;
                }
            }
        }
        Ok(())
    }
}
