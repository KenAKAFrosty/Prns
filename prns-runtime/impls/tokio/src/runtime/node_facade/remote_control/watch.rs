use tokio::io::AsyncReadExt;

use crate::remote_control::{
    RemoteControlStreamEvent, RemoteControlStreamEventError, REMOTE_CONTROL_STREAM_EVENT_LEN,
};

use super::super::ByteStreamReader;

#[derive(Debug)]
pub enum RemoteControlWatchOpenError {
    Registration(super::super::StreamReaderRegistrationError),
    Operation(crate::runtime::RemoteControlTargetOperationError),
}

impl std::fmt::Display for RemoteControlWatchOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registration(error) => write!(f, "interface watch registration failed: {error}"),
            Self::Operation(error) => write!(f, "interface watch admission failed: {error:?}"),
        }
    }
}

impl std::error::Error for RemoteControlWatchOpenError {}

impl From<crate::runtime::RemoteControlError> for RemoteControlWatchOpenError {
    fn from(error: crate::runtime::RemoteControlError) -> Self {
        Self::Operation(error.into())
    }
}

impl From<crate::runtime::RemoteControlTargetOperationError> for RemoteControlWatchOpenError {
    fn from(error: crate::runtime::RemoteControlTargetOperationError) -> Self {
        Self::Operation(error)
    }
}

#[derive(Debug)]
pub enum RemoteControlWatchReadError {
    Io(std::io::Error),
    Frame(RemoteControlStreamEventError),
    SequenceGap { expected: u32, found: u32 },
}

impl std::fmt::Display for RemoteControlWatchReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "interface watch read failed: {error}"),
            Self::Frame(error) => write!(f, "invalid interface watch frame: {error:?}"),
            Self::SequenceGap { expected, found } => write!(
                f,
                "interface watch sequence was {found}, expected {expected}; refetch snapshots"
            ),
        }
    }
}

impl std::error::Error for RemoteControlWatchReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(_) | Self::SequenceGap { .. } => None,
        }
    }
}

/// A typed interface-watch stream. A sequence gap means the caller must refetch snapshots.
pub struct RemoteControlInterfaceWatch {
    reader: ByteStreamReader,
    next_sequence: u32,
    frame: [u8; REMOTE_CONTROL_STREAM_EVENT_LEN],
    received: usize,
}

impl RemoteControlInterfaceWatch {
    pub(super) fn new(reader: ByteStreamReader) -> Self {
        Self {
            reader,
            next_sequence: 1,
            frame: [0; REMOTE_CONTROL_STREAM_EVENT_LEN],
            received: 0,
        }
    }

    /// Cancellation safe: a partial frame remains buffered for the next call.
    pub async fn next_event(
        &mut self,
    ) -> Result<RemoteControlStreamEvent, RemoteControlWatchReadError> {
        while self.received < self.frame.len() {
            let count = self
                .reader
                .read(&mut self.frame[self.received..])
                .await
                .map_err(RemoteControlWatchReadError::Io)?;
            if count == 0 {
                return Err(RemoteControlWatchReadError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "interface watch ended before the next complete frame",
                )));
            }
            self.received += count;
        }
        self.received = 0;
        let event = RemoteControlStreamEvent::parse(&self.frame)
            .map_err(RemoteControlWatchReadError::Frame)?;
        let found = event.sequence();
        let expected = std::mem::replace(&mut self.next_sequence, found.wrapping_add(1));
        if found != expected {
            return Err(RemoteControlWatchReadError::SequenceGap { expected, found });
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::driver::StreamInbound;
    use tokio::sync::{mpsc, oneshot};

    #[tokio::test]
    async fn canceled_partial_read_preserves_frame_alignment() {
        let (inbound, receiver) = mpsc::channel(4);
        let (_failure, failure_rx) = oneshot::channel();
        let mut watch =
            RemoteControlInterfaceWatch::new(ByteStreamReader::new(receiver, failure_rx));
        let mut frame = [0; REMOTE_CONTROL_STREAM_EVENT_LEN];
        let event = RemoteControlStreamEvent::ResyncRequired { sequence: 1 };
        event.write_into(&mut frame).unwrap();
        inbound
            .send(StreamInbound {
                payload: frame[..5].to_vec(),
                eof: false,
                compressed: false,
            })
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), watch.next_event())
                .await
                .is_err()
        );
        assert_eq!(watch.received, 5);
        inbound
            .send(StreamInbound {
                payload: frame[5..].to_vec(),
                eof: false,
                compressed: false,
            })
            .await
            .unwrap();
        assert_eq!(watch.next_event().await.unwrap(), event);
    }

    #[tokio::test]
    async fn typed_reader_detects_a_sequence_gap_and_can_continue() {
        let (inbound, receiver) = mpsc::channel(4);
        let (_failure, failure_rx) = oneshot::channel();
        let mut watch =
            RemoteControlInterfaceWatch::new(ByteStreamReader::new(receiver, failure_rx));
        for sequence in [1, 3, 4] {
            let mut frame = [0; REMOTE_CONTROL_STREAM_EVENT_LEN];
            RemoteControlStreamEvent::Heartbeat { sequence }
                .write_into(&mut frame)
                .unwrap();
            inbound
                .send(StreamInbound {
                    payload: frame.to_vec(),
                    eof: false,
                    compressed: false,
                })
                .await
                .unwrap();
        }
        assert_eq!(
            watch.next_event().await.unwrap(),
            RemoteControlStreamEvent::Heartbeat { sequence: 1 },
        );
        assert!(matches!(
            watch.next_event().await,
            Err(RemoteControlWatchReadError::SequenceGap {
                expected: 2,
                found: 3,
            }),
        ));
        assert_eq!(
            watch.next_event().await.unwrap(),
            RemoteControlStreamEvent::Heartbeat { sequence: 4 },
        );
    }
}
