use personal_rns::engine::FanTarget;
use personal_rns::interfaces::INTERFACE_ID_LEN;

use crate::runtime::{OutboundFrame, OutboundFrameKind, OutboundTarget};

const OUTBOUND_BATCH_MAGIC: u32 = u32::from_le_bytes(*b"POUT");
const OUTBOUND_BATCH_VERSION: u16 = 1;
const OUTBOUND_BATCH_HEADER_BYTES: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OutboundBatchError {
    TooManyFrames,
    FrameTooLarge,
    EmptyFrame,
    CapacityExceeded,
}

pub(crate) fn encode(frames: &[OutboundFrame]) -> Result<Vec<u8>, OutboundBatchError> {
    let count = u32::try_from(frames.len()).map_err(|_| OutboundBatchError::TooManyFrames)?;
    let capacity = frames
        .iter()
        .try_fold(OUTBOUND_BATCH_HEADER_BYTES, |capacity, frame| {
            if frame.bytes.is_empty() {
                return Err(OutboundBatchError::EmptyFrame);
            }
            let target_bytes = match &frame.target {
                OutboundTarget::Interface(_) => 1usize + INTERFACE_ID_LEN,
                OutboundTarget::Broadcast { fan, .. } => 1usize
                    .checked_add(1)
                    .and_then(|size| size.checked_add(1))
                    .and_then(|size| {
                        size.checked_add(match fan {
                            FanTarget::All => 0,
                            FanTarget::Only(_) | FanTarget::AllExcept(_) => INTERFACE_ID_LEN,
                        })
                    })
                    .ok_or(OutboundBatchError::CapacityExceeded)?,
            };
            let kind_bytes = match frame.kind {
                OutboundFrameKind::Frame => 1usize,
                OutboundFrameKind::Announce { .. } => 2,
            };
            capacity
                .checked_add(kind_bytes)
                .and_then(|size| size.checked_add(target_bytes))
                .and_then(|size| size.checked_add(size_of::<u32>()))
                .and_then(|size| size.checked_add(frame.bytes.len()))
                .ok_or(OutboundBatchError::CapacityExceeded)
        })?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| OutboundBatchError::CapacityExceeded)?;
    push_u32(&mut bytes, OUTBOUND_BATCH_MAGIC);
    push_u16(&mut bytes, OUTBOUND_BATCH_VERSION);
    push_u16(&mut bytes, 0);
    push_u32(&mut bytes, count);
    for frame in frames {
        match frame.kind {
            OutboundFrameKind::Frame => bytes.push(0),
            OutboundFrameKind::Announce { hops } => {
                bytes.push(1);
                bytes.push(hops);
            }
        }
        match &frame.target {
            OutboundTarget::Interface(interface) => {
                bytes.push(0);
                bytes.extend_from_slice(interface.as_bytes());
            }
            OutboundTarget::Broadcast { supervisor, fan } => {
                bytes.push(1);
                bytes.push(*supervisor as u8);
                match fan {
                    FanTarget::All => bytes.push(0),
                    FanTarget::Only(interface) => {
                        bytes.push(1);
                        bytes.extend_from_slice(interface.as_bytes());
                    }
                    FanTarget::AllExcept(interface) => {
                        bytes.push(2);
                        bytes.extend_from_slice(interface.as_bytes());
                    }
                }
            }
        }
        let frame_bytes =
            u32::try_from(frame.bytes.len()).map_err(|_| OutboundBatchError::FrameTooLarge)?;
        push_u32(&mut bytes, frame_bytes);
        bytes.extend_from_slice(&frame.bytes);
    }
    Ok(bytes)
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use personal_rns::engine::FanTarget;
    use personal_rns::interfaces::{InterfaceId, InterfaceKind};

    use super::{encode, OutboundBatchError};
    use crate::runtime::{OutboundFrame, OutboundFrameKind, OutboundTarget};

    #[test]
    fn encodes_every_outbound_shape_without_strings() {
        let direct = InterfaceId::new([4, 1, 2, 3, 4, 5, 6, 7]);
        let excluded = InterfaceId::new([13, 8, 9, 10, 11, 12, 13, 14]);
        let frames = [
            OutboundFrame {
                target: OutboundTarget::Interface(direct),
                bytes: vec![0x21, 0x22],
                kind: OutboundFrameKind::Frame,
            },
            OutboundFrame {
                target: OutboundTarget::Broadcast {
                    supervisor: InterfaceKind::BluetoothAuto,
                    fan: FanTarget::AllExcept(excluded),
                },
                bytes: vec![0x31],
                kind: OutboundFrameKind::Announce { hops: 3 },
            },
        ];

        assert_eq!(
            encode(&frames),
            Ok(vec![
                b'P', b'O', b'U', b'T', 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 4, 1, 2, 3, 4, 5, 6, 7, 2, 0,
                0, 0, 0x21, 0x22, 1, 3, 1, 12, 2, 13, 8, 9, 10, 11, 12, 13, 14, 1, 0, 0, 0, 0x31,
            ]),
        );
    }

    #[test]
    fn rejects_empty_frames_before_producing_a_partial_batch() {
        let frames = [OutboundFrame {
            target: OutboundTarget::Interface(InterfaceId::new([4, 0, 0, 0, 0, 0, 0, 0])),
            bytes: Vec::new(),
            kind: OutboundFrameKind::Frame,
        }];

        assert_eq!(encode(&frames), Err(OutboundBatchError::EmptyFrame));
    }
}
