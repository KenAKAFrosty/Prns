use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::engine::{
    EngineCommand, IssuedCommand, PacketReceiptDelivered, SendToChannelFailure, Settlement,
};
use crate::reactor::compression;
use crate::reactor::driver::{HostCommand, StreamInbound};
use crate::routing::links::channel::byte_stream::{parse, MAX_STREAM_CHUNK_LEN, STREAM_DATA_TYPE};
use crate::routing::links::LinkId;
use crate::units::RttMillis;

use super::super::PrnsNodeHandle;
use super::{ByteStreamReader, ByteStreamWriter, StreamId};

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

    ready.send(()).expect("the opener is parked on the ack");
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
