use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};

/// A stream carries invalidations, not potentially stale or partial inventory pages.
/// The controller fetches the affected snapshot using its existing read-only grant.
const STREAM_EVENT_HEADER_LEN: usize = 2;
const STREAM_EVENT_SEQUENCE_LEN: usize = core::mem::size_of::<u32>();
pub const REMOTE_CONTROL_STREAM_EVENT_LEN: usize =
    STREAM_EVENT_HEADER_LEN + STREAM_EVENT_SEQUENCE_LEN + INTERFACE_ID_LEN;
pub const REMOTE_CONTROL_STREAM_VERSION: u8 = 1;

#[repr(u8)]
enum RemoteControlStreamEventKind {
    InterfaceChanged = 1,
    PeersChanged = 2,
    ResyncRequired = 3,
    Heartbeat = 4,
}

impl RemoteControlStreamEventKind {
    const fn from_wire(value: u8) -> Option<Self> {
        match value {
            value if value == Self::InterfaceChanged as u8 => Some(Self::InterfaceChanged),
            value if value == Self::PeersChanged as u8 => Some(Self::PeersChanged),
            value if value == Self::ResyncRequired as u8 => Some(Self::ResyncRequired),
            value if value == Self::Heartbeat as u8 => Some(Self::Heartbeat),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlStreamEvent {
    InterfaceChanged {
        sequence: u32,
        interface: InterfaceId,
    },
    PeersChanged {
        sequence: u32,
        interface: InterfaceId,
    },
    ResyncRequired {
        sequence: u32,
    },
    Heartbeat {
        sequence: u32,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlStreamEventError {
    WrongLength,
    UnsupportedVersion,
    UnknownKind,
    InvalidInterface,
}

impl RemoteControlStreamEvent {
    pub const fn sequence(&self) -> u32 {
        match self {
            Self::InterfaceChanged { sequence, .. }
            | Self::PeersChanged { sequence, .. }
            | Self::ResyncRequired { sequence }
            | Self::Heartbeat { sequence } => *sequence,
        }
    }

    pub fn write_into(&self, output: &mut [u8]) -> Result<usize, RemoteControlStreamEventError> {
        let target = output
            .get_mut(..REMOTE_CONTROL_STREAM_EVENT_LEN)
            .ok_or(RemoteControlStreamEventError::WrongLength)?;
        let Some((version, rest)) = target.split_first_mut() else {
            return Err(RemoteControlStreamEventError::WrongLength);
        };
        let Some((kind, body)) = rest.split_first_mut() else {
            return Err(RemoteControlStreamEventError::WrongLength);
        };
        let (event_kind, interface_bytes) = match self {
            Self::InterfaceChanged { interface, .. } => (
                RemoteControlStreamEventKind::InterfaceChanged,
                *interface.as_bytes(),
            ),
            Self::PeersChanged { interface, .. } => (
                RemoteControlStreamEventKind::PeersChanged,
                *interface.as_bytes(),
            ),
            Self::ResyncRequired { .. } => (
                RemoteControlStreamEventKind::ResyncRequired,
                [0; INTERFACE_ID_LEN],
            ),
            Self::Heartbeat { .. } => (
                RemoteControlStreamEventKind::Heartbeat,
                [0; INTERFACE_ID_LEN],
            ),
        };
        *version = REMOTE_CONTROL_STREAM_VERSION;
        *kind = event_kind as u8;
        body.get_mut(..STREAM_EVENT_SEQUENCE_LEN)
            .ok_or(RemoteControlStreamEventError::WrongLength)?
            .copy_from_slice(&self.sequence().to_be_bytes());
        body.get_mut(STREAM_EVENT_SEQUENCE_LEN..)
            .ok_or(RemoteControlStreamEventError::WrongLength)?
            .copy_from_slice(&interface_bytes);
        Ok(REMOTE_CONTROL_STREAM_EVENT_LEN)
    }

    pub fn parse(input: &[u8]) -> Result<Self, RemoteControlStreamEventError> {
        if input.len() != REMOTE_CONTROL_STREAM_EVENT_LEN {
            return Err(RemoteControlStreamEventError::WrongLength);
        }
        let Some((version, rest)) = input.split_first() else {
            return Err(RemoteControlStreamEventError::WrongLength);
        };
        if *version != REMOTE_CONTROL_STREAM_VERSION {
            return Err(RemoteControlStreamEventError::UnsupportedVersion);
        }
        let Some((kind, body)) = rest.split_first() else {
            return Err(RemoteControlStreamEventError::WrongLength);
        };
        let kind = RemoteControlStreamEventKind::from_wire(*kind)
            .ok_or(RemoteControlStreamEventError::UnknownKind)?;
        let sequence = u32::from_be_bytes(
            body.get(..STREAM_EVENT_SEQUENCE_LEN)
                .ok_or(RemoteControlStreamEventError::WrongLength)?
                .try_into()
                .map_err(|_| RemoteControlStreamEventError::WrongLength)?,
        );
        let interface_bytes: [u8; INTERFACE_ID_LEN] = body
            .get(STREAM_EVENT_SEQUENCE_LEN..)
            .ok_or(RemoteControlStreamEventError::WrongLength)?
            .try_into()
            .map_err(|_| RemoteControlStreamEventError::WrongLength)?;
        match kind {
            RemoteControlStreamEventKind::ResyncRequired => {
                if interface_bytes != [0; INTERFACE_ID_LEN] {
                    return Err(RemoteControlStreamEventError::InvalidInterface);
                }
                Ok(Self::ResyncRequired { sequence })
            }
            RemoteControlStreamEventKind::Heartbeat => {
                if interface_bytes != [0; INTERFACE_ID_LEN] {
                    return Err(RemoteControlStreamEventError::InvalidInterface);
                }
                Ok(Self::Heartbeat { sequence })
            }
            RemoteControlStreamEventKind::InterfaceChanged => Ok(Self::InterfaceChanged {
                sequence,
                interface: InterfaceId::new(interface_bytes),
            }),
            RemoteControlStreamEventKind::PeersChanged => Ok(Self::PeersChanged {
                sequence,
                interface: InterfaceId::new(interface_bytes),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_event_shapes() {
        let interface = InterfaceId::new([7; INTERFACE_ID_LEN]);
        for event in [
            RemoteControlStreamEvent::InterfaceChanged {
                sequence: 42,
                interface,
            },
            RemoteControlStreamEvent::PeersChanged {
                sequence: 43,
                interface,
            },
            RemoteControlStreamEvent::ResyncRequired { sequence: 44 },
            RemoteControlStreamEvent::Heartbeat { sequence: 45 },
        ] {
            let mut bytes = [0; REMOTE_CONTROL_STREAM_EVENT_LEN];
            assert_eq!(event.write_into(&mut bytes), Ok(bytes.len()));
            assert_eq!(RemoteControlStreamEvent::parse(&bytes), Ok(event));
        }
    }

    #[test]
    fn rejects_unknown_or_truncated_frames() {
        let mut bytes = [0; REMOTE_CONTROL_STREAM_EVENT_LEN];
        (RemoteControlStreamEvent::ResyncRequired { sequence: 1 })
            .write_into(&mut bytes)
            .unwrap();
        assert_eq!(
            RemoteControlStreamEvent::parse(&bytes[..2]),
            Err(RemoteControlStreamEventError::WrongLength)
        );
        *bytes.get_mut(0).unwrap() = 2;
        assert_eq!(
            RemoteControlStreamEvent::parse(&bytes),
            Err(RemoteControlStreamEventError::UnsupportedVersion)
        );
        *bytes.get_mut(0).unwrap() = REMOTE_CONTROL_STREAM_VERSION;
        *bytes.get_mut(1).unwrap() = 99;
        assert_eq!(
            RemoteControlStreamEvent::parse(&bytes),
            Err(RemoteControlStreamEventError::UnknownKind)
        );
        *bytes.get_mut(1).unwrap() = RemoteControlStreamEventKind::ResyncRequired as u8;
        *bytes.last_mut().unwrap() = 1;
        assert_eq!(
            RemoteControlStreamEvent::parse(&bytes),
            Err(RemoteControlStreamEventError::InvalidInterface)
        );
    }
}
