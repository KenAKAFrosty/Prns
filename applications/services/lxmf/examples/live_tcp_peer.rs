#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::request_endpoints;
use personal_rns::runtime::{NoPersistence, PrnsNode, PrnsNodeRecipe};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpClientInterface;
use personal_rns::ManuallyAttached;
use prns_lxmf::direct::{
    prepare_local_lxmf_destination, DirectLxmfService, PrnsDirectNetwork, SendDirectTextOutcome,
};
use prns_lxmf::wire::encode_current_lxmf_announce;
use prns_lxmf::LxmfVerification;

const LOCAL_SECRET: [u8; IDENTITY_SECRET_KEY_LEN] = [0x31; IDENTITY_SECRET_KEY_LEN];
const EXPECTED_FROM_PYTHON: &[u8] = b"python-to-rust";
const SENT_FROM_RUST: &[u8] = b"rust-to-python";
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    MissingTarget,
    AnnounceEncoding,
    InvalidDestination,
    NoTokioRuntime,
    PeerTimeout,
    InboundTimeout,
    SendRejected,
    OutboundTimeout,
    StopTimedOut,
    NodeStopped,
}

async fn wait_for_peer(service: &DirectLxmfService) -> Result<[u8; 16], Failure> {
    let deadline = tokio::time::Instant::now() + COMPLETION_TIMEOUT;
    loop {
        if let Some(peer) = service.snapshot().await.peers.first() {
            return Ok(peer.destination);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::PeerTimeout);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_verified_python_message(service: &DirectLxmfService) -> Result<(), Failure> {
    let deadline = tokio::time::Instant::now() + COMPLETION_TIMEOUT;
    let mut next_debug = tokio::time::Instant::now();
    loop {
        let snapshot = service.snapshot().await;
        let received = snapshot.messages.iter().any(|message| {
            message.content == EXPECTED_FROM_PYTHON
                && message.verification == LxmfVerification::Verified
        });
        if received {
            return Ok(());
        }
        if std::env::var_os("PRNS_LXMF_DEBUG").is_some()
            && tokio::time::Instant::now() >= next_debug
        {
            eprintln!("PRNS_LXMF_PROGRESS {snapshot:?}");
            next_debug = tokio::time::Instant::now() + Duration::from_secs(1);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::InboundTimeout);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_outbound_proof(service: &DirectLxmfService) -> Result<(), Failure> {
    let deadline = tokio::time::Instant::now() + COMPLETION_TIMEOUT;
    loop {
        let snapshot = service.snapshot().await;
        let delivered = snapshot.messages.iter().any(|message| {
            message.content == SENT_FROM_RUST
                && message.delivery_state == prns_lxmf::LxmfDeliveryState::Delivered
        });
        if delivered {
            return Ok(());
        }
        if snapshot.messages.iter().any(|message| {
            message.content == SENT_FROM_RUST
                && message.delivery_state == prns_lxmf::LxmfDeliveryState::Failed
        }) {
            return Err(Failure::OutboundTimeout);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::OutboundTimeout);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn complete_exchange(service: DirectLxmfService) -> Result<(), Failure> {
    let peer = wait_for_peer(&service).await?;
    // Waiting for Python's verified message proves it learned our authenticated
    // announce before we send the reverse direction.
    wait_for_verified_python_message(&service).await?;
    match service
        .send_direct_text(peer, 1_700_000_000_123, b"Rust", SENT_FROM_RUST)
        .await
    {
        SendDirectTextOutcome::Started { .. } => {}
        _ => return Err(Failure::SendRejected),
    }
    wait_for_outbound_proof(&service).await?;
    service.stop().await.map_err(|_| Failure::StopTimedOut)?;
    println!("PRNS_LXMF_LIVE_OK inbound=verified outbound=proof links=two");
    Ok(())
}

#[tokio::main]
async fn main() {
    let result = run().await;
    if let Err(failure) = result {
        eprintln!("FAILED {failure:?}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Failure> {
    let target = std::env::var("PRNS_LXMF_TCP_TARGET").map_err(|_| Failure::MissingTarget)?;
    let mut announce = [0u8; 64];
    let announce_len = encode_current_lxmf_announce(b"Prns live peer", &mut announce)
        .map_err(|_| Failure::AnnounceEncoding)?;
    let (identity, destination) =
        prepare_local_lxmf_destination(Zeroizing::new(LOCAL_SECRET), &announce[..announce_len])
            .map_err(|_| Failure::InvalidDestination)?;
    let (pending, callbacks) = DirectLxmfService::prepare(identity);
    let event_callbacks = callbacks.clone();
    let client = TcpClientInterface::new(target);
    let node = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: move |event, _state: &()| {
            let _outcome = event_callbacks.on_prns_event(&event);
        },
    })
    .with_accepted_announce_observer(callbacks.accepted_announce_observer());
    let handle = node.handle();
    handle.attach(client);
    let service = pending
        .start(Arc::new(PrnsDirectNetwork::new(handle)))
        .map_err(|_| Failure::NoTokioRuntime)?;
    let announcer = service.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(400));
        loop {
            interval.tick().await;
            if announcer.announce().await.is_err() {
                return;
            }
        }
    });

    tokio::select! {
        _result = node.run() => Err(Failure::NodeStopped),
        result = complete_exchange(service) => result,
    }
}
