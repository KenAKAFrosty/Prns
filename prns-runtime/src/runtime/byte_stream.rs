//! The host `AsyncRead`/`AsyncWrite` faces of RNS's `Buffer`: a byte pipe over one channel's
//! reserved stream type. The wire framing lives in
//! [`channel::byte_stream`](crate::routing::links::channel::byte_stream); this veneer chunks
//! writes into stream-data channel sends and reassembles inbound chunks by `(link, stream id)`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::engine::{
    EngineCommand, SendToChannel, SendToChannelBody, SendToChannelFailure, Settlement,
    MAX_SEND_TO_CHANNEL_BODY_LEN,
};
use crate::reactor::impls::compression;
use crate::reactor::impls::tokio_reactor::StreamInbound;
use crate::routing::links::channel::byte_stream::{
    StreamDataHeader, HEADER_LEN, MAX_STREAM_CHUNK_LEN, STREAM_DATA_TYPE,
};
use crate::routing::links::LinkId;

use super::tokio_bind::TokioPrnsHandle;

pub use crate::routing::links::channel::byte_stream::StreamId;

/// The most stream payload one channel send carries: the consumer channel body cap less the header.
/// RNS `StreamDataMessage.MAX_DATA_LEN`.
const CHUNK_CEILING: usize = MAX_SEND_TO_CHANNEL_BODY_LEN - HEADER_LEN;

/// RNS `RawChannelWriter.write`: a chunk this small is never worth a bz2 attempt.
const COMPRESSION_MIN_CHUNK: usize = 32;

/// RNS `RawChannelWriter.COMPRESSION_TRIES`: how many progressively smaller segments of one input
/// chunk the writer tries to compress into a single message before giving up and sending raw.
const COMPRESSION_TRIES: usize = 4;

/// How long a writer waits for in-flight sends to ack before retrying a window-full chunk.
const WINDOW_BACKOFF: Duration = Duration::from_millis(5);

/// The read half of a byte stream: an `AsyncRead` over the inbound chunks the run loop's demux
/// routes here. Reassembles chunks in order and ends at the eof frame.
pub struct ByteStreamReader {
    inbound: UnboundedReceiver<StreamInbound>,
    current: Option<std::vec::Vec<u8>>,
    cursor: usize,
    eof: bool,
}

impl ByteStreamReader {
    pub(crate) fn new(inbound: UnboundedReceiver<StreamInbound>) -> Self {
        Self {
            inbound,
            current: None,
            cursor: 0,
            eof: false,
        }
    }
}

impl AsyncRead for ByteStreamReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if let Some(chunk) = this.current.as_ref() {
                if this.cursor < chunk.len() {
                    let take = (chunk.len() - this.cursor).min(buf.remaining());
                    buf.put_slice(&chunk[this.cursor..this.cursor + take]);
                    this.cursor += take;
                    return Poll::Ready(Ok(()));
                }
            }
            this.current = None;
            this.cursor = 0;
            if this.eof {
                return Poll::Ready(Ok(()));
            }
            match this.inbound.poll_recv(cx) {
                Poll::Ready(Some(inbound)) => {
                    if inbound.eof {
                        this.eof = true;
                    }
                    let payload = if inbound.compressed {
                        match compression::decompress_bounded(
                            &inbound.payload,
                            MAX_STREAM_CHUNK_LEN as u64,
                        ) {
                            Ok(bytes) => bytes,
                            Err(_) => {
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "malformed compressed stream chunk",
                                )))
                            }
                        }
                    } else {
                        inbound.payload
                    };
                    this.current = Some(payload);
                    this.cursor = 0;
                }
                Poll::Ready(None) => {
                    this.eof = true;
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Frame `payload` under `header` and send it on the channel, retrying past a full send window until
/// it delivers; resolves to the bytes consumed, or an `io::Error` on teardown/timeout.
async fn send_chunk(
    handle: TokioPrnsHandle,
    link_id: LinkId,
    header: StreamDataHeader,
    payload: std::vec::Vec<u8>,
    consumed: usize,
) -> io::Result<usize> {
    loop {
        let mut body = SendToChannelBody::new();
        if body.extend_from_slice(&header.to_bytes()).is_err()
            || body.extend_from_slice(&payload).is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stream chunk exceeds the channel body",
            ));
        }
        let command = EngineCommand::SendToChannel(SendToChannel {
            link_id,
            message_type: STREAM_DATA_TYPE,
            body,
        });
        match handle.settle(command).await {
            Some(Settlement::SendToChannel(Ok(_))) => return Ok(consumed),
            Some(Settlement::SendToChannel(Err(SendToChannelFailure::WindowFull))) => {
                tokio::time::sleep(WINDOW_BACKOFF).await;
            }
            Some(Settlement::SendToChannel(Err(failure))) => {
                return Err(io::Error::other(std::format!(
                    "channel send failed: {failure:?}"
                )));
            }
            Some(_) | None => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "the node has stopped",
                ))
            }
        }
    }
}

/// RNS `RawChannelWriter.write`'s compression choice: try to compress progressively smaller
/// segments of one input chunk until one fits a single message, and if none does, send a raw
/// message. Returns the payload to frame, whether it is compressed, and how many input bytes it
/// consumed (a compressed message consumes more input than it carries on the wire).
fn compress_stream_chunk(input: std::vec::Vec<u8>) -> (std::vec::Vec<u8>, bool, usize) {
    let chunk_len = input.len();
    let mut comp_try = 1;
    while chunk_len > COMPRESSION_MIN_CHUNK && comp_try < COMPRESSION_TRIES {
        let segment_len = chunk_len / comp_try;
        if let Some(compressed) = compression::compress_if_smaller(&input[..segment_len]) {
            if compressed.len() < CHUNK_CEILING {
                return (compressed, true, segment_len);
            }
        }
        comp_try += 1;
    }
    let take = chunk_len.min(CHUNK_CEILING);
    (input[..take].to_vec(), false, take)
}

type SendFuture<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

/// The write half of a byte stream: an `AsyncWrite` that frames each write as a stream-data channel
/// send under the reserved type, one chunk in flight at a time. `poll_shutdown` sends the eof frame.
pub struct ByteStreamWriter {
    handle: TokioPrnsHandle,
    link_id: LinkId,
    stream_id: StreamId,
    pending: Option<SendFuture<usize>>,
    closing: Option<SendFuture<()>>,
}

impl ByteStreamWriter {
    pub(crate) fn new(handle: TokioPrnsHandle, link_id: LinkId, stream_id: StreamId) -> Self {
        Self {
            handle,
            link_id,
            stream_id,
            pending: None,
            closing: None,
        }
    }
}

impl AsyncWrite for ByteStreamWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match this.pending.as_mut() {
            None => {
                if buf.is_empty() {
                    return Poll::Ready(Ok(0));
                }
                let chunk_len = buf.len().min(MAX_STREAM_CHUNK_LEN);
                let input = buf[..chunk_len].to_vec();
                let handle = this.handle.clone();
                let link_id = this.link_id;
                let stream_id = this.stream_id;
                let mut fut: SendFuture<usize> = Box::pin(async move {
                    let (payload, compressed, consumed) = if chunk_len > COMPRESSION_MIN_CHUNK {
                        tokio::task::spawn_blocking(move || compress_stream_chunk(input))
                            .await
                            .map_err(|_| {
                                io::Error::new(io::ErrorKind::BrokenPipe, "the node has stopped")
                            })?
                    } else {
                        compress_stream_chunk(input)
                    };
                    let header = StreamDataHeader {
                        stream_id,
                        eof: false,
                        compressed,
                    };
                    send_chunk(handle, link_id, header, payload, consumed).await
                });
                match fut.as_mut().poll(cx) {
                    Poll::Ready(result) => Poll::Ready(result),
                    Poll::Pending => {
                        this.pending = Some(fut);
                        Poll::Pending
                    }
                }
            }
            Some(pending) => match pending.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    this.pending = None;
                    Poll::Ready(result)
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(pending) = this.pending.as_mut() {
            match pending.as_mut().poll(cx) {
                Poll::Ready(Ok(_)) => this.pending = None,
                Poll::Ready(Err(e)) => {
                    this.pending = None;
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
        match this.closing.as_mut() {
            None => {
                let header = StreamDataHeader {
                    stream_id: this.stream_id,
                    eof: true,
                    compressed: false,
                };
                let handle = this.handle.clone();
                let link_id = this.link_id;
                let mut fut: SendFuture<()> = Box::pin(async move {
                    send_chunk(handle, link_id, header, std::vec::Vec::new(), 0)
                        .await
                        .map(|_| ())
                });
                match fut.as_mut().poll(cx) {
                    Poll::Ready(result) => Poll::Ready(result),
                    Poll::Pending => {
                        this.closing = Some(fut);
                        Poll::Pending
                    }
                }
            }
            Some(closing) => closing.as_mut().poll(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{IssuedCommand, PacketReceiptDelivered};
    use crate::reactor::impls::tokio_reactor::HostCommand;
    use crate::routing::links::channel::byte_stream::parse;
    use crate::units::RttMillis;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn chunk(bytes: &[u8], eof: bool, compressed: bool) -> StreamInbound {
        StreamInbound {
            payload: bytes.to_vec(),
            eof,
            compressed,
        }
    }

    #[tokio::test]
    async fn reader_reassembles_chunks_in_order_and_stops_at_eof() {
        let (sink, inbound) = tokio::sync::mpsc::unbounded_channel();
        let mut reader = ByteStreamReader::new(inbound);
        sink.send(chunk(b"hello ", false, false)).unwrap();
        sink.send(chunk(b"byte ", false, false)).unwrap();
        sink.send(chunk(b"stream", true, false)).unwrap();
        let mut out = std::vec::Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello byte stream");
    }

    #[tokio::test]
    async fn reader_treats_a_dropped_sink_as_end_of_stream() {
        let (sink, inbound) = tokio::sync::mpsc::unbounded_channel();
        let mut reader = ByteStreamReader::new(inbound);
        sink.send(chunk(b"partial", false, false)).unwrap();
        drop(sink);
        let mut out = std::vec::Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"partial");
    }

    #[tokio::test]
    async fn reader_inflates_a_compressed_chunk() {
        let (sink, inbound) = tokio::sync::mpsc::unbounded_channel();
        let mut reader = ByteStreamReader::new(inbound);
        let original = std::vec![7u8; 2000];
        let compressed = compression::compress_if_smaller(&original).expect("a run compresses");
        sink.send(chunk(&compressed, false, true)).unwrap();
        sink.send(chunk(b"", true, false)).unwrap();
        let mut out = std::vec::Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(
            out, original,
            "a compressed chunk inflates back to its bytes"
        );
    }

    #[tokio::test]
    async fn reader_errors_on_a_malformed_compressed_chunk() {
        let (sink, inbound) = tokio::sync::mpsc::unbounded_channel();
        let mut reader = ByteStreamReader::new(inbound);
        sink.send(chunk(b"not a bz2 stream", false, true)).unwrap();
        let mut out = std::vec::Vec::new();
        let err = reader.read_to_end(&mut out).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn writer_frames_each_write_as_a_stream_data_send_and_closes_with_eof() {
        let (commands_tx, mut commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let link = LinkId::new([7; 16]);
        let stream_id = StreamId::new(3).unwrap();
        let mut writer = ByteStreamWriter::new(TokioPrnsHandle::over(commands_tx), link, stream_id);

        let write = tokio::spawn(async move {
            writer.write_all(b"hello").await.unwrap();
            writer.shutdown().await.unwrap();
        });

        let mut frames = std::vec::Vec::new();
        for _ in 0..2 {
            let HostCommand::AwaitedEngine {
                issued: IssuedCommand { command, .. },
                completion,
            } = commands_rx.recv().await.unwrap()
            else {
                panic!("expected an awaited engine command");
            };
            let EngineCommand::SendToChannel(send) = command else {
                panic!("expected a SendToChannel command");
            };
            assert_eq!(send.link_id, link);
            assert_eq!(send.message_type, STREAM_DATA_TYPE);
            let frame = parse(&send.body).unwrap();
            assert_eq!(frame.header.stream_id, stream_id);
            frames.push((frame.header.eof, frame.payload.to_vec()));
            completion
                .send(Settlement::SendToChannel(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(0),
                })))
                .unwrap();
        }

        write.await.unwrap();
        assert_eq!(frames[0], (false, b"hello".to_vec()));
        assert_eq!(frames[1], (true, std::vec::Vec::new()));
    }

    #[tokio::test]
    async fn writer_packs_a_compressible_write_into_one_compressed_message() {
        let (commands_tx, mut commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let link = LinkId::new([7; 16]);
        let stream_id = StreamId::new(3).unwrap();
        let mut writer = ByteStreamWriter::new(TokioPrnsHandle::over(commands_tx), link, stream_id);

        let original = std::vec![7u8; 4096];
        let original_for_task = original.clone();
        let write = tokio::spawn(async move {
            writer.write_all(&original_for_task).await.unwrap();
        });

        let HostCommand::AwaitedEngine {
            issued: IssuedCommand { command, .. },
            completion,
        } = commands_rx.recv().await.unwrap()
        else {
            panic!("expected an awaited engine command");
        };
        let EngineCommand::SendToChannel(send) = command else {
            panic!("expected a SendToChannel command");
        };
        let frame = parse(&send.body).unwrap();
        assert!(
            frame.header.compressed,
            "a 4 KiB run rides a single compressed message",
        );
        assert!(
            frame.payload.len() < original.len(),
            "the message on the wire is far smaller than the input it carries",
        );
        assert_eq!(
            compression::decompress_bounded(frame.payload, MAX_STREAM_CHUNK_LEN as u64),
            Ok(original),
            "the compressed message inflates back to the whole write",
        );
        completion
            .send(Settlement::SendToChannel(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(0),
            })))
            .unwrap();
        write.await.unwrap();
    }

    #[tokio::test]
    async fn a_mixed_stream_round_trips_writer_to_reader() {
        let (commands_tx, mut commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let link = LinkId::new([7; 16]);
        let stream_id = StreamId::new(3).unwrap();
        let mut writer = ByteStreamWriter::new(TokioPrnsHandle::over(commands_tx), link, stream_id);

        let mut original = std::vec![9u8; 5000];
        let mut x = 0x1234_5678_9abc_def0u64;
        original.extend((0..5000).map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        }));

        let original_for_task = original.clone();
        let write = tokio::spawn(async move {
            writer.write_all(&original_for_task).await.unwrap();
            writer.shutdown().await.unwrap();
        });

        let (sink, inbound) = tokio::sync::mpsc::unbounded_channel();
        loop {
            let HostCommand::AwaitedEngine {
                issued: IssuedCommand { command, .. },
                completion,
            } = commands_rx.recv().await.unwrap()
            else {
                panic!("expected an awaited engine command");
            };
            let EngineCommand::SendToChannel(send) = command else {
                panic!("expected a SendToChannel command");
            };
            let frame = parse(&send.body).unwrap();
            let eof = frame.header.eof;
            sink.send(StreamInbound {
                payload: frame.payload.to_vec(),
                eof,
                compressed: frame.header.compressed,
            })
            .unwrap();
            completion
                .send(Settlement::SendToChannel(Ok(PacketReceiptDelivered {
                    rtt: RttMillis::new(0),
                })))
                .unwrap();
            if eof {
                break;
            }
        }
        write.await.unwrap();

        let mut reader = ByteStreamReader::new(inbound);
        let mut out = std::vec::Vec::new();
        reader.read_to_end(&mut out).await.unwrap();
        assert_eq!(
            out, original,
            "a stream of compressed and raw messages reassembles to the source",
        );
    }

    #[tokio::test]
    async fn writer_retries_a_chunk_past_a_full_send_window() {
        let (commands_tx, mut commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let link = LinkId::new([9; 16]);
        let stream_id = StreamId::new(1).unwrap();
        let mut writer = ByteStreamWriter::new(TokioPrnsHandle::over(commands_tx), link, stream_id);

        let write = tokio::spawn(async move {
            writer.write_all(b"x").await.unwrap();
        });

        let HostCommand::AwaitedEngine { completion, .. } = commands_rx.recv().await.unwrap()
        else {
            panic!("expected an awaited engine command");
        };
        completion
            .send(Settlement::SendToChannel(Err(
                SendToChannelFailure::WindowFull,
            )))
            .unwrap();

        let HostCommand::AwaitedEngine {
            issued: IssuedCommand { command, .. },
            completion,
        } = commands_rx.recv().await.unwrap()
        else {
            panic!("expected the retried command");
        };
        let EngineCommand::SendToChannel(send) = command else {
            panic!("expected a SendToChannel command");
        };
        assert_eq!(send.message_type, STREAM_DATA_TYPE);
        let frame = parse(&send.body).unwrap();
        assert_eq!(frame.payload, b"x");
        completion
            .send(Settlement::SendToChannel(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(0),
            })))
            .unwrap();

        write.await.unwrap();
    }
}
