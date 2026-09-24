use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::{self, Receiver};
use tokio::sync::oneshot;

use crate::engine::{
    PrnsCommand, SendToChannel, SendToChannelBody, SendToChannelFailure, Settlement,
    MAX_SEND_TO_CHANNEL_BODY_LEN,
};
use crate::manifold::compression;
use crate::manifold::driver::{HostCommand, StreamInbound, StreamReceiveFailure};
use crate::routing::links::channel::byte_stream::{
    StreamDataHeader, HEADER_LEN, MAX_STREAM_CHUNK_LEN, STREAM_DATA_TYPE,
};
use crate::routing::links::LinkId;

use super::PrnsNodeHandle;

pub use crate::manifold::driver::StreamReaderRegistrationError;
pub use crate::routing::links::channel::byte_stream::StreamId;

/// RNS `StreamDataMessage.MAX_DATA_LEN`.
const CHUNK_CEILING: usize = MAX_SEND_TO_CHANNEL_BODY_LEN - HEADER_LEN;

/// RNS `RawChannelWriter.write`; a chunk this small or smaller is never worth a compression attempt.
const COMPRESSION_MIN_CHUNK: usize = 32;
const MAX_COMPRESSION_TRIES: usize = 4;

const WINDOW_BACKOFF: Duration = Duration::from_millis(5);
/// A slow reader must fail explicitly before queued stream chunks grow without bound.
pub const BYTE_STREAM_RECEIVE_QUEUE_DEPTH: usize = 32;

enum StreamReadPhase {
    Receiving,
    Complete,
    Failed(StreamReceiveFailure),
}

pub struct ByteStreamReader {
    inbound: Receiver<StreamInbound>,
    failure: oneshot::Receiver<StreamReceiveFailure>,
    current: Option<std::vec::Vec<u8>>,
    cursor: usize,
    phase: StreamReadPhase,
}

impl ByteStreamReader {
    pub(crate) fn new(
        inbound: Receiver<StreamInbound>,
        failure: oneshot::Receiver<StreamReceiveFailure>,
    ) -> Self {
        Self {
            inbound,
            failure,
            current: None,
            cursor: 0,
            phase: StreamReadPhase::Receiving,
        }
    }
}

fn receive_error(failure: StreamReceiveFailure) -> io::Error {
    let kind = match failure {
        StreamReceiveFailure::AlreadyRegistered => io::ErrorKind::AlreadyExists,
        StreamReceiveFailure::Overflowed | StreamReceiveFailure::MalformedCompressedChunk => {
            io::ErrorKind::InvalidData
        }
        StreamReceiveFailure::LinkClosed => io::ErrorKind::UnexpectedEof,
        StreamReceiveFailure::SourceStopped => io::ErrorKind::BrokenPipe,
    };
    io::Error::new(kind, failure)
}

impl AsyncRead for ByteStreamReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let StreamReadPhase::Failed(failure) = &this.phase {
            return Poll::Ready(Err(receive_error(*failure)));
        }
        if matches!(&this.phase, StreamReadPhase::Receiving) {
            let failure = match Pin::new(&mut this.failure).poll(cx) {
                Poll::Ready(Ok(failure)) => Some(failure),
                Poll::Ready(Err(_)) => Some(StreamReceiveFailure::SourceStopped),
                Poll::Pending => None,
            };
            if let Some(failure) = failure {
                this.phase = StreamReadPhase::Failed(failure);
                return Poll::Ready(Err(receive_error(failure)));
            }
        }
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
            if matches!(&this.phase, StreamReadPhase::Complete) {
                return Poll::Ready(Ok(()));
            }
            match this.inbound.poll_recv(cx) {
                Poll::Ready(Some(inbound)) => {
                    let payload = if inbound.compressed {
                        match compression::decompress_bounded(
                            &inbound.payload,
                            MAX_STREAM_CHUNK_LEN as u64,
                        ) {
                            Ok(bytes) => bytes,
                            Err(_) => {
                                let failure = StreamReceiveFailure::MalformedCompressedChunk;
                                this.phase = StreamReadPhase::Failed(failure);
                                return Poll::Ready(Err(receive_error(failure)));
                            }
                        }
                    } else {
                        inbound.payload
                    };
                    if inbound.eof {
                        this.phase = StreamReadPhase::Complete;
                    }
                    this.current = Some(payload);
                    this.cursor = 0;
                }
                Poll::Ready(None) => {
                    this.phase = StreamReadPhase::Failed(StreamReceiveFailure::SourceStopped);
                    return Poll::Ready(Err(receive_error(StreamReceiveFailure::SourceStopped)));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

async fn send_chunk(
    handle: PrnsNodeHandle,
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
        let command = PrnsCommand::SendToChannel(SendToChannel {
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

enum StreamChunkEncoding {
    Plain,
    Compressed,
}

struct PreparedStreamChunk {
    payload: std::vec::Vec<u8>,
    encoding: StreamChunkEncoding,
    consumed: usize,
}

/// RNS `RawChannelWriter.write`'s compression choice
fn compress_stream_chunk(input: std::vec::Vec<u8>) -> PreparedStreamChunk {
    let chunk_len = input.len();
    let mut comp_try = 1;
    while chunk_len > COMPRESSION_MIN_CHUNK && comp_try < MAX_COMPRESSION_TRIES {
        let segment_len = chunk_len / comp_try;
        if let Some(compressed) = compression::compress_if_smaller(&input[..segment_len]) {
            if compressed.len() < CHUNK_CEILING {
                return PreparedStreamChunk {
                    payload: compressed,
                    encoding: StreamChunkEncoding::Compressed,
                    consumed: segment_len,
                };
            }
        }
        comp_try += 1;
    }
    let take = chunk_len.min(CHUNK_CEILING);
    PreparedStreamChunk {
        payload: input[..take].to_vec(),
        encoding: StreamChunkEncoding::Plain,
        consumed: take,
    }
}

type SendFuture<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

pub struct ByteStreamWriter {
    handle: PrnsNodeHandle,
    link_id: LinkId,
    stream_id: StreamId,
    pending: Option<SendFuture<usize>>,
    closing: Option<SendFuture<()>>,
}

impl ByteStreamWriter {
    pub(crate) fn new(handle: PrnsNodeHandle, link_id: LinkId, stream_id: StreamId) -> Self {
        Self {
            handle,
            link_id,
            stream_id,
            pending: None,
            closing: None,
        }
    }
}

impl PrnsNodeHandle {
    /// Open a byte-stream reader on this link and stream id. Awaits the run loop's acknowledgement that the sink is live before yielding the reader, so a chunk arriving the instant the link opens is buffered for the reader, never forwarded past it to the app.
    pub async fn byte_stream_reader(
        &self,
        link_id: LinkId,
        stream_id: StreamId,
    ) -> ByteStreamReader {
        self.register_byte_stream_reader(link_id, stream_id).await.0
    }

    /// Register exclusively; an existing reader and stream remain untouched on failure.
    pub async fn try_byte_stream_reader(
        &self,
        link_id: LinkId,
        stream_id: StreamId,
    ) -> Result<ByteStreamReader, StreamReaderRegistrationError> {
        let (reader, registered) = self.register_byte_stream_reader(link_id, stream_id).await;
        registered.map(|()| reader)
    }

    async fn register_byte_stream_reader(
        &self,
        link_id: LinkId,
        stream_id: StreamId,
    ) -> (ByteStreamReader, Result<(), StreamReaderRegistrationError>) {
        let (sink, inbound) = mpsc::channel(BYTE_STREAM_RECEIVE_QUEUE_DEPTH);
        let (failure, failure_rx) = oneshot::channel();
        let (ready, registered) = oneshot::channel();
        let _ = self.commands.send(HostCommand::RegisterStreamReader {
            link_id,
            stream_id,
            sink,
            failure,
            ready,
        });
        let registration = registered
            .await
            .unwrap_or(Err(StreamReaderRegistrationError::NodeStopped));
        (ByteStreamReader::new(inbound, failure_rx), registration)
    }

    /// Open a byte-stream writer on this link and stream id: an `AsyncWrite` framing each write as a stream-data channel send.
    pub fn byte_stream_writer(&self, link_id: LinkId, stream_id: StreamId) -> ByteStreamWriter {
        ByteStreamWriter::new(self.clone(), link_id, stream_id)
    }

    /// Open a bidirectional byte stream: a reader on `rx` and a writer on `tx` over one link's channel, RNS's `create_bidirectional_buffer`. Awaits the reader's registration (see [`byte_stream_reader`](Self::byte_stream_reader)) so the read half is live before either is handed back.
    pub async fn byte_stream(
        &self,
        link_id: LinkId,
        rx: StreamId,
        tx: StreamId,
    ) -> (ByteStreamReader, ByteStreamWriter) {
        (
            self.byte_stream_reader(link_id, rx).await,
            self.byte_stream_writer(link_id, tx),
        )
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
                    let prepared = if chunk_len > COMPRESSION_MIN_CHUNK {
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
                        compressed: matches!(prepared.encoding, StreamChunkEncoding::Compressed),
                    };
                    send_chunk(handle, link_id, header, prepared.payload, prepared.consumed).await
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
mod tests;
