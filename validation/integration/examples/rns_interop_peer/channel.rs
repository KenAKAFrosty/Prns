use personal_rns::engine::{EstablishLinkFailure, SendToChannelFailure};
use personal_rns::request_endpoints;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::runtime::{
    Diagnostic, Message, NoPersistence, PreConfiguredDestination, PrnsEvent, PrnsNode,
    PrnsNodeRecipe, SendError,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;

use super::common::{
    require_proof, required_environment, ProofFailure, COMPLETION_GRACE, COMPLETION_TIMEOUT,
};

const MESSAGE_TYPE: MessageType = MessageType(0x1337);
const EXPECTED_FROM_STOCK: [&[u8]; 2] = [b"stock-channel-zero", b"stock-channel-one"];
const SENT_FROM_PRNS: [&[u8]; 2] = [b"prns-channel-zero", b"prns-channel-one"];

enum Observation {
    Message(Vec<u8>),
    OutboundComplete,
    Failed(Failure),
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Failure {
    MissingTarget,
    Establish(SendError<EstablishLinkFailure>),
    Send(SendError<SendToChannelFailure>),
    Proof(ProofFailure),
    UnexpectedMessageType(MessageType),
    UnexpectedMessage,
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run() -> Result<(), Failure> {
    let target = required_environment("PRNS_TCP_TARGET").map_err(|_| Failure::MissingTarget)?;
    let client = TcpClientInterface::new(target);
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    let (destination_tx, mut destination_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_tx = observed_tx.clone();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: move |handle: &personal_rns::PrnsNodeHandle| {
            handle.attach(client);
        },
        persistence: NoPersistence,
        on_event: move |event, _state| match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                let _ = destination_tx.send(destination);
            }
            PrnsEvent::Message(Message::ChannelMessage {
                message_type, data, ..
            }) if message_type == MESSAGE_TYPE => {
                let _ = event_tx.send(Observation::Message(data.to_vec()));
            }
            PrnsEvent::Message(Message::ChannelMessage { message_type, .. }) => {
                let _ = event_tx.send(Observation::Failed(Failure::UnexpectedMessageType(
                    message_type,
                )));
            }
            _ => {}
        },
    });
    let handle = node.handle();
    tokio::spawn(async move {
        let Some(destination) = destination_rx.recv().await else {
            let _ = observed_tx.send(Observation::Failed(Failure::EventStreamClosed));
            return;
        };
        let link_id = match handle.establish_link(destination).await {
            Ok(link_id) => link_id,
            Err(error) => {
                let _ = observed_tx.send(Observation::Failed(Failure::Establish(error)));
                return;
            }
        };
        for payload in SENT_FROM_PRNS {
            let receipt = match handle
                .send_channel_message(link_id, MESSAGE_TYPE, payload)
                .await
            {
                Ok(receipt) => receipt,
                Err(error) => {
                    let _ = observed_tx.send(Observation::Failed(Failure::Send(error)));
                    return;
                }
            };
            if let Err(error) = require_proof(receipt) {
                let _ = observed_tx.send(Observation::Failed(Failure::Proof(error)));
                return;
            }
        }
        let _ = observed_tx.send(Observation::OutboundComplete);
    });
    println!("PRNS_CHANNEL_CLIENT_UP");

    let completion = async move {
        let mut received = Vec::new();
        let mut outbound_complete = false;
        while let Some(observation) = observed_rx.recv().await {
            match observation {
                Observation::Message(payload) => {
                    let Some(expected) = EXPECTED_FROM_STOCK.get(received.len()) else {
                        return Err(Failure::UnexpectedMessage);
                    };
                    if payload != *expected {
                        return Err(Failure::UnexpectedMessage);
                    }
                    received.push(payload);
                }
                Observation::OutboundComplete => outbound_complete = true,
                Observation::Failed(error) => return Err(error),
            }
            if outbound_complete && received.len() == EXPECTED_FROM_STOCK.len() {
                println!("PRNS_CHANNEL_OK messages=2 ordered=1 proven=2");
                tokio::time::sleep(COMPLETION_GRACE).await;
                return Ok(());
            }
        }
        Err(Failure::EventStreamClosed)
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
