//! The host `AsyncRead`/`AsyncWrite` faces of RNS's `Buffer`: a byte pipe over one channel's
//! reserved stream type. The wire framing lives in
//! [`channel::byte_stream`](crate::routing::links::channel::byte_stream); this is the tokio veneer
//! that chunks writes into stream-data channel sends and reassembles inbound chunks the run loop's
//! demux routes here by `(link, stream id)`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::engine::{
    EngineCommand, SendChannel, SendChannelBody, SendChannelFailure, Settlement,
    MAX_SEND_CHANNEL_BODY_LEN,
};
use crate::reactor::impls::tokio_reactor::StreamInbound;
use crate::routing::links::channel::byte_stream::{StreamDataHeader, HEADER_LEN, STREAM_DATA_TYPE};
use crate::routing::links::LinkId;

use super::tokio_bind::PrnsHandle;

pub use crate::routing::links::channel::byte_stream::StreamId;

/// The most stream payload one channel send carries: the consumer channel body cap less the header.
const CHUNK_CEILING: usize = MAX_SEND_CHANNEL_BODY_LEN - HEADER_LEN;

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
                    if inbound.compressed {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "compressed byte streams are not yet supported",
                        )));
                    }
                    if inbound.eof {
                        this.eof = true;
                    }
                    this.current = Some(inbound.payload);
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
    handle: PrnsHandle,
    link_id: LinkId,
    header: StreamDataHeader,
    payload: std::vec::Vec<u8>,
    consumed: usize,
) -> io::Result<usize> {
    loop {
        let mut body = SendChannelBody::new();
        if body.extend_from_slice(&header.to_bytes()).is_err()
            || body.extend_from_slice(&payload).is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stream chunk exceeds the channel body",
            ));
        }
        let command = EngineCommand::SendChannel(SendChannel {
            link_id,
            message_type: STREAM_DATA_TYPE,
            body,
        });
        match handle.settle(command).await {
            Some(Settlement::SendChannel(Ok(_))) => return Ok(consumed),
            Some(Settlement::SendChannel(Err(SendChannelFailure::WindowFull))) => {
                tokio::time::sleep(WINDOW_BACKOFF).await;
            }
            Some(Settlement::SendChannel(Err(failure))) => {
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

type SendFuture<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

/// The write half of a byte stream: an `AsyncWrite` that frames each write as a stream-data channel
/// send under the reserved type, one chunk in flight at a time. `poll_shutdown` sends the eof frame.
pub struct ByteStreamWriter {
    handle: PrnsHandle,
    link_id: LinkId,
    stream_id: StreamId,
    pending: Option<SendFuture<usize>>,
    closing: Option<SendFuture<()>>,
}

impl ByteStreamWriter {
    pub(crate) fn new(handle: PrnsHandle, link_id: LinkId, stream_id: StreamId) -> Self {
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
                let take = buf.len().min(CHUNK_CEILING);
                let header = StreamDataHeader {
                    stream_id: this.stream_id,
                    eof: false,
                    compressed: false,
                };
                let mut fut: SendFuture<usize> = Box::pin(send_chunk(
                    this.handle.clone(),
                    this.link_id,
                    header,
                    buf[..take].to_vec(),
                    take,
                ));
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
    use tokio::io::AsyncReadExt;

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
    async fn reader_errors_on_a_compressed_chunk_until_bz2_lands() {
        let (sink, inbound) = tokio::sync::mpsc::unbounded_channel();
        let mut reader = ByteStreamReader::new(inbound);
        sink.send(chunk(b"\x42\x5a", false, true)).unwrap();
        let mut out = std::vec::Vec::new();
        let err = reader.read_to_end(&mut out).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}
