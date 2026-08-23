use core::time::Duration;

use personal_rns::engine::{SendGroupFailure, SendPlainPacketFailure};
use personal_rns::identity::IdentityHash;
use personal_rns::request_endpoints;
use personal_rns::routing::delivery::Delivery;
use personal_rns::runtime::{
    Message, NoPersistence, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    SendError,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;

use super::common::{required_environment, COMPLETION_GRACE, COMPLETION_TIMEOUT};

const GROUP_IDENTITY: IdentityHash = IdentityHash::new([
    0x4C, 0xD0, 0xCC, 0x45, 0xA7, 0x40, 0x5D, 0xBD, 0x5C, 0xF9, 0xB5, 0xBE, 0x1E, 0xF9, 0x2F, 0x10,
]);
const GROUP_KEY: [u8; 64] = [0x42; 64];
const EXPECTED_PLAIN: &[u8] = &[
    0x00, 0x73, 0x74, 0x6F, 0x63, 0x6B, 0x2D, 0x70, 0x6C, 0x61, 0x69, 0x6E, 0xFF,
];
const EXPECTED_GROUP: &[u8] = &[
    0xFF, 0x73, 0x74, 0x6F, 0x63, 0x6B, 0x2D, 0x67, 0x72, 0x6F, 0x75, 0x70, 0x00,
];
const SENT_PLAIN: &[u8] = &[
    0xFF, 0x70, 0x72, 0x6E, 0x73, 0x2D, 0x70, 0x6C, 0x61, 0x69, 0x6E, 0x00,
];
const SENT_GROUP: &[u8] = &[
    0x00, 0x70, 0x72, 0x6E, 0x73, 0x2D, 0x67, 0x72, 0x6F, 0x75, 0x70, 0xFF,
];
const SEND_INTERVAL: Duration = Duration::from_millis(300);

enum Observation {
    Plain {
        destination: personal_rns::wire::DestinationHash,
        payload: Vec<u8>,
    },
    Group {
        destination: personal_rns::wire::DestinationHash,
        plaintext: Vec<u8>,
    },
    PlainSendFailed(SendError<SendPlainPacketFailure>),
    GroupSendFailed(SendError<SendGroupFailure>),
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Failure {
    MissingTarget,
    InvalidPlainDestination,
    InvalidGroupDestination,
    UnexpectedPlain,
    UnexpectedGroup,
    PlainSend(SendError<SendPlainPacketFailure>),
    GroupSend(SendError<SendGroupFailure>),
    EventStreamClosed,
    Timeout,
    NodeStopped,
}

pub async fn run() -> Result<(), Failure> {
    let target =
        required_environment("PRNS_PLAIN_GROUP_TARGET").map_err(|_| Failure::MissingTarget)?;
    let plain = PreConfiguredDestination::Plain {
        app_name: "prns",
        aspects: &["destination", "plain"],
    };
    let group = PreConfiguredDestination::Group {
        app_name: "prns",
        aspects: &["destination", "group"],
        identity: GROUP_IDENTITY,
        shared_key: &GROUP_KEY,
    };
    let plain_hash = plain
        .destination_hash()
        .map_err(|_| Failure::InvalidPlainDestination)?;
    let group_hash = group
        .destination_hash()
        .map_err(|_| Failure::InvalidGroupDestination)?;
    let client = TcpClientInterface::new(target);
    let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
    let delivered_tx = observed_tx.clone();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [plain, group],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: move |handle: &personal_rns::PrnsNodeHandle| {
            handle.attach(client);
        },
        persistence: NoPersistence,
        on_event: move |event, _state| match event {
            PrnsEvent::Message(Message::Delivered(Delivery::Plain(delivery))) => {
                let _ = delivered_tx.send(Observation::Plain {
                    destination: delivery.destination,
                    payload: delivery.payload.to_vec(),
                });
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Group(delivery))) => {
                let _ = delivered_tx.send(Observation::Group {
                    destination: delivery.destination,
                    plaintext: delivery.plaintext.to_vec(),
                });
            }
            _ => {}
        },
    });
    let sender = node.handle();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SEND_INTERVAL);
        loop {
            interval.tick().await;
            if let Err(error) = sender.send_plain_packet(plain_hash, SENT_PLAIN).await {
                let _ = observed_tx.send(Observation::PlainSendFailed(error));
                return;
            }
            if let Err(error) = sender.send_group_packet(group_hash, SENT_GROUP).await {
                let _ = observed_tx.send(Observation::GroupSendFailed(error));
                return;
            }
        }
    });
    println!("PRNS_PLAIN_GROUP_PEER_UP");

    let completion = async move {
        let mut plain_received = false;
        let mut group_received = false;
        while let Some(observation) = observed_rx.recv().await {
            match observation {
                Observation::Plain {
                    destination,
                    payload,
                } if destination == plain_hash && payload == EXPECTED_PLAIN => {
                    plain_received = true;
                }
                Observation::Group {
                    destination,
                    plaintext,
                } if destination == group_hash && plaintext == EXPECTED_GROUP => {
                    group_received = true;
                }
                Observation::Plain { .. } => return Err(Failure::UnexpectedPlain),
                Observation::Group { .. } => return Err(Failure::UnexpectedGroup),
                Observation::PlainSendFailed(error) => return Err(Failure::PlainSend(error)),
                Observation::GroupSendFailed(error) => return Err(Failure::GroupSend(error)),
            }
            if plain_received && group_received {
                println!("PRNS_PLAIN_GROUP_OK received_plain=1 received_group=1");
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
