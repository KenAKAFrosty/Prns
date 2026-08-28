use tokio::io::{AsyncReadExt, AsyncWriteExt};

use personal_rns::engine::EstablishLinkFailure;
use personal_rns::request_endpoints;
use personal_rns::routing::links::channel::byte_stream::{StreamId, StreamIdError};
use personal_rns::runtime::{
    Diagnostic, NoPersistence, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    SendError,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;
use personal_rns::wire::DestinationHash;
use personal_rns::PrnsNodeHandle;

use super::common::{required_environment, COMPLETION_GRACE, COMPLETION_TIMEOUT};

const PAYLOAD_SIZE: usize = 4_096;
const RECEIVE_STREAM_ID: u16 = 11;
const SEND_STREAM_ID: u16 = 7;
const WRITE_BOUNDARIES: [usize; 5] = [1, 73, 1_027, 19, 509];
const READ_BOUNDARIES: [usize; 5] = [2, 257, 31, 1_021, 7];

fn candidate_payload() -> Vec<u8> {
    (0..PAYLOAD_SIZE)
        .map(|index| (index.wrapping_mul(29).wrapping_add(7)) as u8)
        .collect()
}

fn stock_payload() -> Vec<u8> {
    (0..PAYLOAD_SIZE)
        .map(|index| (index.wrapping_mul(17).wrapping_add(3)) as u8)
        .collect()
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Failure {
    MissingTarget,
    Establish(SendError<EstablishLinkFailure>),
    InvalidStreamId(StreamIdError),
    Read(std::io::Error),
    Write(std::io::Error),
    UnexpectedStockPayload { received_bytes: usize },
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

async fn exchange(handle: PrnsNodeHandle, destination: DestinationHash) -> Result<(), Failure> {
    let link_id = handle
        .establish_link(destination)
        .await
        .map_err(Failure::Establish)?;
    let receive_stream = StreamId::new(RECEIVE_STREAM_ID).map_err(Failure::InvalidStreamId)?;
    let send_stream = StreamId::new(SEND_STREAM_ID).map_err(Failure::InvalidStreamId)?;
    let (mut reader, mut writer) = handle
        .byte_stream(link_id, receive_stream, send_stream)
        .await;

    let outbound = async {
        let payload = candidate_payload();
        let mut offset = 0;
        let mut boundary = 0;
        while offset < payload.len() {
            let end =
                (offset + WRITE_BOUNDARIES[boundary % WRITE_BOUNDARIES.len()]).min(payload.len());
            writer
                .write_all(&payload[offset..end])
                .await
                .map_err(Failure::Write)?;
            offset = end;
            boundary += 1;
        }
        writer.shutdown().await.map_err(Failure::Write)
    };

    let inbound = async {
        let expected = stock_payload();
        let mut received = Vec::with_capacity(expected.len());
        let mut boundary = 0;
        loop {
            let mut buffer = vec![0; READ_BOUNDARIES[boundary % READ_BOUNDARIES.len()]];
            let read = reader.read(&mut buffer).await.map_err(Failure::Read)?;
            if read == 0 {
                break;
            }
            received.extend_from_slice(&buffer[..read]);
            boundary += 1;
        }
        if received != expected {
            return Err(Failure::UnexpectedStockPayload {
                received_bytes: received.len(),
            });
        }
        Ok(())
    };

    tokio::try_join!(outbound, inbound)?;
    Ok(())
}

pub async fn run() -> Result<(), Failure> {
    let target = required_environment("PRNS_TCP_TARGET").map_err(|_| Failure::MissingTarget)?;
    let client = TcpClientInterface::new(target);
    let (destination_tx, mut destination_rx) = tokio::sync::mpsc::unbounded_channel();
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: move |handle: &personal_rns::PrnsNodeHandle| {
            handle.attach(client);
        },
        persistence: NoPersistence,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = destination_tx.send(destination);
            }
        },
    });
    let handle = node.handle();
    println!("PRNS_BUFFER_STREAM_CLIENT_UP");

    let completion = async move {
        let Some(destination) = destination_rx.recv().await else {
            return Err(Failure::EventStreamClosed);
        };
        exchange(handle, destination).await?;
        println!("PRNS_BUFFER_STREAM_OK received=4096 sent=4096 eof=1");
        tokio::time::sleep(COMPLETION_GRACE).await;
        Ok(())
    };
    tokio::select! {
        result = node.run() => {
            let _ = result;
            Err(Failure::NodeStopped)
        }
        result = tokio::time::timeout(COMPLETION_TIMEOUT, completion) => {
            result.map_err(|_| Failure::Timeout)?
        }
    }
}
