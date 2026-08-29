use personal_rns::engine::{EstablishLinkFailure, LinkClosedReason, SendToChannelFailure};
use personal_rns::request_endpoints;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    Diagnostic, NoPersistence, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    SendError,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;

use super::common::{
    require_proof, required_environment, ProofFailure, COMPLETION_GRACE, COMPLETION_TIMEOUT,
};

const READY_MESSAGE_TYPE: MessageType = MessageType(0x1339);

#[allow(dead_code)]
#[derive(Debug)]
pub enum Failure {
    MissingTarget,
    Establish(SendError<EstablishLinkFailure>),
    Send(SendError<SendToChannelFailure>),
    Proof(ProofFailure),
    CloseNotQueued,
    AnnounceStreamClosed,
    ClosureStreamClosed,
    UnexpectedClosure,
    Timeout,
    NodeStopped,
}

pub async fn run() -> Result<(), Failure> {
    let target = required_environment("PRNS_TCP_TARGET").map_err(|_| Failure::MissingTarget)?;
    let client = TcpClientInterface::new(target);
    let (announce_tx, mut announce_rx) = tokio::sync::mpsc::unbounded_channel();
    let (closure_tx, mut closure_rx) = tokio::sync::mpsc::unbounded_channel();
    let (result_tx, mut result_rx) = tokio::sync::mpsc::unbounded_channel();
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
        on_event: move |event, _state| match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                let _ = announce_tx.send(destination);
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, reason }) => {
                let _ = closure_tx.send((link_id, reason));
            }
            _ => {}
        },
    });
    let handle = node.handle();
    tokio::spawn(async move {
        let Some(first_destination) = announce_rx.recv().await else {
            let _ = result_tx.send(Err(Failure::AnnounceStreamClosed));
            return;
        };
        let first_link = match handle.establish_link(first_destination).await {
            Ok(link_id) => link_id,
            Err(error) => {
                let _ = result_tx.send(Err(Failure::Establish(error)));
                return;
            }
        };
        let first_receipt = match handle
            .send_channel_message(first_link, READY_MESSAGE_TYPE, b"prns-ready-to-close")
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                let _ = result_tx.send(Err(Failure::Send(error)));
                return;
            }
        };
        if let Err(error) = require_proof(first_receipt) {
            let _ = result_tx.send(Err(Failure::Proof(error)));
            return;
        }
        if !handle.close_link(first_link) {
            let _ = result_tx.send(Err(Failure::CloseNotQueued));
            return;
        }
        println!("PRNS_CLOSED_STOCK_LINK queued=1");

        let second_destination = loop {
            let Some(destination) = announce_rx.recv().await else {
                let _ = result_tx.send(Err(Failure::AnnounceStreamClosed));
                return;
            };
            if destination != first_destination {
                break destination;
            }
        };
        let second_link = match handle.establish_link(second_destination).await {
            Ok(link_id) => link_id,
            Err(error) => {
                let _ = result_tx.send(Err(Failure::Establish(error)));
                return;
            }
        };
        let second_receipt = match handle
            .send_channel_message(
                second_link,
                READY_MESSAGE_TYPE,
                b"prns-ready-for-stock-close",
            )
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                let _ = result_tx.send(Err(Failure::Send(error)));
                return;
            }
        };
        if let Err(error) = require_proof(second_receipt) {
            let _ = result_tx.send(Err(Failure::Proof(error)));
            return;
        }
        println!("PRNS_READY_FOR_STOCK_CLOSE proven=1");
        let closure = wait_for_link_closure(&mut closure_rx, second_link).await;
        let _ = result_tx.send(closure);
    });
    println!("PRNS_LINK_CLOSURE_CLIENT_UP");

    let completion = async move {
        match result_rx.recv().await {
            Some(Ok(())) => {
                println!("PRNS_OBSERVED_STOCK_CLOSE reason=peerClosed");
                tokio::time::sleep(COMPLETION_GRACE).await;
                Ok(())
            }
            Some(Err(error)) => Err(error),
            None => Err(Failure::ClosureStreamClosed),
        }
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

async fn wait_for_link_closure(
    closures: &mut tokio::sync::mpsc::UnboundedReceiver<(LinkId, LinkClosedReason)>,
    expected_link: LinkId,
) -> Result<(), Failure> {
    while let Some((link_id, reason)) = closures.recv().await {
        if link_id == expected_link && reason == LinkClosedReason::PeerClosed {
            return Ok(());
        }
        if link_id == expected_link {
            return Err(Failure::UnexpectedClosure);
        }
    }
    Err(Failure::ClosureStreamClosed)
}
