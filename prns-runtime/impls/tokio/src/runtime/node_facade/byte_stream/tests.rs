use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::engine::{
    IssuedCommand, PacketReceiptDelivered, PrnsCommand, SendToChannelFailure, Settlement,
};
use crate::manifold::compression;
use crate::manifold::driver::{HostCommand, StreamInbound, StreamReceiveFailure};
use crate::routing::links::channel::byte_stream::{parse, MAX_STREAM_CHUNK_LEN, STREAM_DATA_TYPE};
use crate::routing::links::LinkId;
use crate::units::RttMillis;

use super::super::PrnsNodeHandle;
use super::{ByteStreamReader, ByteStreamWriter, StreamId, BYTE_STREAM_RECEIVE_QUEUE_DEPTH};

fn reader_pair() -> (
    tokio::sync::mpsc::Sender<StreamInbound>,
    tokio::sync::oneshot::Sender<StreamReceiveFailure>,
    ByteStreamReader,
) {
    let (sink, inbound) = tokio::sync::mpsc::channel(BYTE_STREAM_RECEIVE_QUEUE_DEPTH);
    let (failure, failure_rx) = tokio::sync::oneshot::channel();
    (sink, failure, ByteStreamReader::new(inbound, failure_rx))
}

enum ChunkKind {
    Plain,
    Compressed,
    End,
}

fn chunk(bytes: &[u8], kind: ChunkKind) -> StreamInbound {
    match kind {
        ChunkKind::Plain => StreamInbound {
            payload: bytes.to_vec(),
            eof: false,
            compressed: false,
        },
        ChunkKind::Compressed => StreamInbound {
            payload: bytes.to_vec(),
            eof: false,
            compressed: true,
        },
        ChunkKind::End => StreamInbound {
            payload: bytes.to_vec(),
            eof: true,
            compressed: false,
        },
    }
}

fn delivered() -> PacketReceiptDelivered {
    PacketReceiptDelivered {
        rtt: RttMillis::new(0),
        evidence: crate::engine::DeliveryEvidence::Proof(crate::engine::DeliveryProof::Implicit(
            crate::routing::dedup::PacketHash::new([0; 32]),
        )),
    }
}

#[tokio::test]
async fn reader_reassembles_chunks_in_order_and_stops_at_eof() {
    let (sink, _failure, mut reader) = reader_pair();
    sink.try_send(chunk(b"hello ", ChunkKind::Plain)).unwrap();
    sink.try_send(chunk(b"byte ", ChunkKind::Plain)).unwrap();
    sink.try_send(chunk(b"stream", ChunkKind::End)).unwrap();
    let mut out = std::vec::Vec::new();
    reader.read_to_end(&mut out).await.unwrap();
    assert_eq!(out, b"hello byte stream");
}

#[tokio::test]
async fn eof_frame_drains_its_payload_across_small_reads() {
    let (sink, _failure, mut reader) = reader_pair();
    sink.try_send(chunk(b"final", ChunkKind::End)).unwrap();
    let mut first = [0; 2];
    let mut second = [0; 2];
    reader.read_exact(&mut first).await.unwrap();
    reader.read_exact(&mut second).await.unwrap();
    let mut tail = [0; 2];
    let read = reader.read(&mut tail).await.unwrap();
    assert_eq!(first, *b"fi");
    assert_eq!(second, *b"na");
    assert_eq!(read, 1);
    assert_eq!(tail.first(), Some(&b'l'));
    assert_eq!(reader.read(&mut tail).await.unwrap(), 0);
}

#[tokio::test]
async fn reader_rejects_a_stopped_source_with_buffered_data() {
    let (sink, failure, mut reader) = reader_pair();
    sink.try_send(chunk(b"partial", ChunkKind::Plain)).unwrap();
    drop(sink);
    drop(failure);
    let mut out = std::vec::Vec::new();
    let error = reader.read_to_end(&mut out).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(out.is_empty());
}

#[tokio::test]
async fn reader_inflates_a_compressed_chunk() {
    let (sink, _failure, mut reader) = reader_pair();
    let original = std::vec![7u8; 2000];
    let compressed = compression::compress_if_smaller(&original).expect("a run compresses");
    sink.try_send(chunk(&compressed, ChunkKind::Compressed))
        .unwrap();
    sink.try_send(chunk(b"", ChunkKind::End)).unwrap();
    let mut out = std::vec::Vec::new();
    reader.read_to_end(&mut out).await.unwrap();
    assert_eq!(
        out, original,
        "a compressed chunk inflates back to its bytes"
    );
}

#[tokio::test]
async fn reader_errors_on_a_malformed_compressed_chunk() {
    let (sink, _failure, mut reader) = reader_pair();
    sink.try_send(chunk(b"not a bz2 stream", ChunkKind::Compressed))
        .unwrap();
    let mut out = std::vec::Vec::new();
    let err = reader.read_to_end(&mut out).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        err.get_ref()
            .and_then(|source| source.downcast_ref::<StreamReceiveFailure>()),
        Some(&StreamReceiveFailure::MalformedCompressedChunk)
    );
}

#[tokio::test]
async fn reader_reports_overflow_instead_of_returning_truncated_data() {
    let (sink, inbound) = tokio::sync::mpsc::channel(1);
    let (failure, failure_rx) = tokio::sync::oneshot::channel();
    let mut reader = ByteStreamReader::new(inbound, failure_rx);
    sink.try_send(chunk(b"partial", ChunkKind::Plain)).unwrap();
    failure.send(StreamReceiveFailure::Overflowed).unwrap();
    let mut out = std::vec::Vec::new();
    let error = reader.read_to_end(&mut out).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        error
            .get_ref()
            .and_then(|source| source.downcast_ref::<StreamReceiveFailure>()),
        Some(&StreamReceiveFailure::Overflowed)
    );
    assert!(out.is_empty());
}

#[tokio::test]
async fn reader_reports_link_loss_as_a_typed_terminal_failure() {
    let (_sink, failure, mut reader) = reader_pair();
    failure.send(StreamReceiveFailure::LinkClosed).unwrap();
    let mut out = std::vec::Vec::new();
    let error = reader.read_to_end(&mut out).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    assert_eq!(
        error
            .get_ref()
            .and_then(|source| source.downcast_ref::<StreamReceiveFailure>()),
        Some(&StreamReceiveFailure::LinkClosed)
    );
}

#[tokio::test]
async fn reader_reports_a_stopped_source_without_fabricating_eof() {
    let (_sink, failure, mut reader) = reader_pair();
    drop(failure);
    let mut out = std::vec::Vec::new();
    let error = reader.read_to_end(&mut out).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(
        error
            .get_ref()
            .and_then(|source| source.downcast_ref::<StreamReceiveFailure>()),
        Some(&StreamReceiveFailure::SourceStopped)
    );
}

#[tokio::test]
async fn reader_is_withheld_until_the_run_loop_acks_registration() {
    let (commands, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = PrnsNodeHandle::over(commands);
    let link = LinkId::new([5; 16]);
    let stream = StreamId::new(2).unwrap();
    let opener = handle.clone();
    let open = tokio::spawn(async move { opener.byte_stream_reader(link, stream).await });

    let HostCommand::RegisterStreamReader {
        link_id,
        stream_id,
        ready,
        ..
    } = command_rx
        .recv()
        .await
        .expect("the registration was issued")
    else {
        panic!("byte_stream_reader must register its sink");
    };
    assert_eq!(link_id, link);
    assert_eq!(stream_id, stream);
    assert!(
        !open.is_finished(),
        "the reader is held back until the run loop acknowledges the registration",
    );

    ready.send(Ok(())).expect("the opener is parked on the ack");
    open.await.expect("the reader future resolves once acked");
}

#[tokio::test]
async fn writer_frames_each_write_as_a_stream_data_send_and_closes_with_eof() {
    let (commands_tx, mut commands_rx) = tokio::sync::mpsc::unbounded_channel();
    let link = LinkId::new([7; 16]);
    let stream_id = StreamId::new(3).unwrap();
    let mut writer = ByteStreamWriter::new(PrnsNodeHandle::over(commands_tx), link, stream_id);

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
        let PrnsCommand::SendToChannel(send) = command else {
            panic!("expected a SendToChannel command");
        };
        assert_eq!(send.link_id, link);
        assert_eq!(send.message_type, STREAM_DATA_TYPE);
        let frame = parse(&send.body).unwrap();
        assert_eq!(frame.header.stream_id, stream_id);
        frames.push((frame.header.eof, frame.payload.to_vec()));
        completion
            .send(Settlement::SendToChannel(Ok(delivered())))
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
    let mut writer = ByteStreamWriter::new(PrnsNodeHandle::over(commands_tx), link, stream_id);

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
    let PrnsCommand::SendToChannel(send) = command else {
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
        .send(Settlement::SendToChannel(Ok(delivered())))
        .unwrap();
    write.await.unwrap();
}

#[tokio::test]
async fn a_mixed_stream_round_trips_writer_to_reader() {
    let (commands_tx, mut commands_rx) = tokio::sync::mpsc::unbounded_channel();
    let link = LinkId::new([7; 16]);
    let stream_id = StreamId::new(3).unwrap();
    let mut writer = ByteStreamWriter::new(PrnsNodeHandle::over(commands_tx), link, stream_id);

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

    let (sink, _failure, mut reader) = reader_pair();
    loop {
        let HostCommand::AwaitedEngine {
            issued: IssuedCommand { command, .. },
            completion,
        } = commands_rx.recv().await.unwrap()
        else {
            panic!("expected an awaited engine command");
        };
        let PrnsCommand::SendToChannel(send) = command else {
            panic!("expected a SendToChannel command");
        };
        let frame = parse(&send.body).unwrap();
        let eof = frame.header.eof;
        sink.try_send(StreamInbound {
            payload: frame.payload.to_vec(),
            eof,
            compressed: frame.header.compressed,
        })
        .unwrap();
        completion
            .send(Settlement::SendToChannel(Ok(delivered())))
            .unwrap();
        if eof {
            break;
        }
    }
    write.await.unwrap();

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
    let mut writer = ByteStreamWriter::new(PrnsNodeHandle::over(commands_tx), link, stream_id);

    let write = tokio::spawn(async move {
        writer.write_all(b"x").await.unwrap();
    });

    let HostCommand::AwaitedEngine { completion, .. } = commands_rx.recv().await.unwrap() else {
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
    let PrnsCommand::SendToChannel(send) = command else {
        panic!("expected a SendToChannel command");
    };
    assert_eq!(send.message_type, STREAM_DATA_TYPE);
    let frame = parse(&send.body).unwrap();
    assert_eq!(frame.payload, b"x");
    completion
        .send(Settlement::SendToChannel(Ok(delivered())))
        .unwrap();

    write.await.unwrap();
}
