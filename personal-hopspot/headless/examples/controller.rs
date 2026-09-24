//! Persistent lab controller exercising the public, identity-authenticated API.
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use personal_hopspot_headless::control::{
    app_operator_requests, operator_requests, public_identity, watch_operator_requests,
};
use personal_rns::identity::PublicIdentityMaterial;
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    RemoteControlAppMessage, RemoteControlControllerAuthority, RemoteControlInterfaceConfigOutcome,
    RemoteControlInterfaceContinuation, RemoteControlInterfacePage,
    RemoteControlInterfacePeersOutcome, RemoteControlPeerContinuation, RemoteControlPeerPage,
    RemoteControlStreamEvent,
};
use personal_rns::runtime::{
    RemoteControlIdentityDirectory, RemoteControlTargetAccessControl, StreamId,
};
use serde_json::json;

#[derive(Parser)]
struct Options {
    #[arg(long)]
    state_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create/load the controller identity and print only its public provisioning data.
    Identity,
    /// Authenticate to an explicitly provisioned target and perform one operation.
    Invoke {
        #[arg(long)]
        tcp: SocketAddr,
        #[arg(long, value_parser = public_identity)]
        target_key: PublicIdentityMaterial,
        #[arg(long, value_enum)]
        action: Action,
        #[arg(long, value_parser = interface_id)]
        interface_id: Option<InterfaceId>,
        #[arg(long)]
        message_hex: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Action {
    Describe,
    Announce,
    Build,
    Interfaces,
    Config,
    Peers,
    AppMessage,
    Watch,
}

fn interface_id(value: &str) -> Result<InterfaceId, String> {
    let bytes = hex::decode(value).map_err(|error| error.to_string())?;
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| "interface ID must be 8 bytes".to_owned())?;
    Ok(InterfaceId::new(bytes))
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Identity(#[from] personal_rns::runtime::RemoteControlFileIdentityBootstrapError),
    #[error("controller state: {0}")]
    Io(#[from] std::io::Error),
    #[error("controller identity is already in use: {0}")]
    Lock(std::fs::TryLockError),
    #[error("target grant: {0:?}")]
    Grant(RemoteControlTargetAccessError),
    #[error("target provisioning: {0:?}")]
    Provision(personal_rns::runtime::SetRemoteControlTargetAccessControlError),
    #[error("target path: {0:?}")]
    Path(personal_rns::runtime::RequestPathError),
    #[error("target connection: {0:?}")]
    Connect(personal_rns::runtime::ConnectRemoteControlTargetError),
    #[error("remote operation: {0:?}")]
    Operation(personal_rns::runtime::RemoteControlTargetOperationError),
    #[error("controller exceeded 45 seconds at stage {0}; a timeout does not identify the cause")]
    Timeout(&'static str),
    #[error("missing --interface-id for this action")]
    MissingInterface,
    #[error("invalid --message-hex: {0}")]
    Message(String),
    #[error("remote inventory exceeded 256 pages")]
    Pagination,
    #[error("invalid stream ID: {0:?}")]
    StreamId(personal_rns::routing::links::channel::byte_stream::StreamIdError),
    #[error("watch admission: {0}")]
    WatchOpen(personal_rns::runtime::RemoteControlWatchOpenError),
    #[error("watch stream: {0}")]
    WatchRead(personal_rns::runtime::RemoteControlWatchReadError),
    #[error("controller node stopped: {0:?}")]
    Node(Result<(), personal_rns::runtime::NodeRunError>),
}

async fn run(options: Options) -> Result<(), Error> {
    std::fs::create_dir_all(&options.state_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&options.state_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(options.state_dir.join("controller.lock"))?;
    lock.try_lock().map_err(Error::Lock)?;
    let (secrets, _) = RemoteControlIdentityDirectory::new(options.state_dir.join("identity"))
        .load_or_generate()?
        .into_parts();
    let identities = secrets.identities();
    println!(
        "{}",
        json!({
            "event": "controller_identity",
            "hash": hex::encode(identities.controller().identity_hash().as_bytes()),
            "public_key": hex::encode(identities.controller().public_keys().public_key_bytes()),
        })
    );
    let Command::Invoke {
        tcp,
        target_key,
        action,
        interface_id,
        message_hex,
    } = options.command
    else {
        return Ok(());
    };
    let target = RemoteControlTargetIdentity::new(target_key.public_keys());
    let endpoint = target.endpoint().destination_hash();
    let target_hash = target.identity_hash();
    let grant = RemoteControlTargetAccess::new(
        target,
        RemoteControlControllerAuthority::Operator,
        match action {
            Action::AppMessage => app_operator_requests(),
            Action::Watch => watch_operator_requests(),
            _ => operator_requests(),
        },
    )
    .map_err(Error::Grant)?;
    let interface = TcpClientInterface::new(tcp.to_string());
    let status = interface.status();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: RemoteControlService::new(
            secrets,
            RemoteControlInitialControllerGrants::Nobody,
            RemoteControlSelfAnnouncement::Unavailable,
        )
        .into(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: personal_rns::runtime::NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                eprintln!(
                    "controller_heard_announce destination={}",
                    hex::encode(destination.as_bytes())
                );
            }
        },
    });
    let handle = node.handle();
    handle.add_interface(interface);
    let stage = std::cell::Cell::new("interface connection");
    let operation = async {
        while status.connection() != ConnectionState::Connected {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stage.set("target provisioning");
        handle
            .set_remote_control_target_access(grant)
            .await
            .map_err(Error::Provision)?;
        stage.set("path discovery");
        handle.request_path(endpoint).await.map_err(Error::Path)?;
        stage.set("authenticated target connection");
        let connection = handle
            .connect_remote_control_target(target_hash)
            .await
            .map_err(Error::Connect)?;
        stage.set("remote operation");
        match action {
            Action::Describe => {
                let (description, rtt) = connection.describe().await.map_err(Error::Operation)?;
                println!(
                    "{}",
                    json!({"event":"describe","description":format!("{description:?}"),"rtt":format!("{rtt:?}")})
                );
            }
            Action::Announce => {
                let rtt = connection.announce_self().await.map_err(Error::Operation)?;
                println!("{}", json!({"event":"announce","rtt":format!("{rtt:?}")}));
            }
            Action::Build => {
                let (build, rtt) = connection
                    .describe_build()
                    .await
                    .map_err(Error::Operation)?;
                println!(
                    "{}",
                    json!({"event":"build","version":build.as_str(),"rtt":format!("{rtt:?}")})
                );
            }
            Action::Interfaces => {
                let mut page = RemoteControlInterfacePage::First;
                let mut completion = Err(Error::Pagination);
                for index in 0..256 {
                    stage.set("interface inventory");
                    let (inventory, rtt) = connection
                        .inventory_interfaces_page(page)
                        .await
                        .map_err(Error::Operation)?;
                    let entries = inventory.entries().iter().map(|entry| json!({
                        "id":hex::encode(entry.id.as_bytes()),"kind":format!("{:?}",entry.kind),
                        "mode":format!("{:?}",entry.mode),"connection":format!("{:?}",entry.connection),
                        "enabled":entry.enabled,"tx_bytes":entry.tx_bytes,"rx_bytes":entry.rx_bytes,
                        "links":entry.links,
                        "rate_bytes_per_sec":entry.rate_bytes_per_sec.map(core::num::NonZeroU32::get),
                    })).collect::<Vec<_>>();
                    println!(
                        "{}",
                        json!({"event":"interfaces","page":index,"entries":entries,"rtt":format!("{rtt:?}")})
                    );
                    match inventory.continuation() {
                        RemoteControlInterfaceContinuation::Complete => {
                            completion = Ok(());
                            break;
                        }
                        RemoteControlInterfaceContinuation::More(cursor) => {
                            page = RemoteControlInterfacePage::After(cursor)
                        }
                    }
                }
                completion?;
            }
            Action::Config => {
                let id = interface_id.ok_or(Error::MissingInterface)?;
                stage.set("interface configuration");
                let (outcome, rtt) = connection
                    .inventory_interface_config(id)
                    .await
                    .map_err(Error::Operation)?;
                let card = match outcome {
                    RemoteControlInterfaceConfigOutcome::UnknownInterface => {
                        json!({"status":"unknown_interface"})
                    }
                    RemoteControlInterfaceConfigOutcome::Card(card) => json!({
                        "status":"card","name":card.name.as_str(),"group":card.group.as_str(),
                        "config":card.config.as_str(),"failure":card.failure.as_str(),
                        "destinations":card.destinations,"transported_links":card.transported_links,
                    }),
                };
                println!(
                    "{}",
                    json!({"event":"interface_config","id":hex::encode(id.as_bytes()),"result":card,"rtt":format!("{rtt:?}")})
                );
            }
            Action::Peers => {
                let id = interface_id.ok_or(Error::MissingInterface)?;
                let mut page = RemoteControlPeerPage::First;
                let mut completion = Err(Error::Pagination);
                for index in 0..256 {
                    stage.set("interface peers");
                    let (outcome, rtt) = connection
                        .inventory_interface_peers(id, page)
                        .await
                        .map_err(Error::Operation)?;
                    let RemoteControlInterfacePeersOutcome::Page(peers) = outcome else {
                        println!(
                            "{}",
                            json!({"event":"interface_peers","id":hex::encode(id.as_bytes()),"status":"unknown_interface"})
                        );
                        completion = Ok(());
                        break;
                    };
                    let entries = peers.peers.iter().map(|peer| json!({
                        "id":hex::encode(peer.id.as_bytes()),"connection":format!("{:?}",peer.connection),
                        "tx_bytes":peer.tx_bytes,"rx_bytes":peer.rx_bytes,"links":peer.links,
                        "destinations":peer.destinations,
                        "rate_bytes_per_sec":peer.rate_bytes_per_sec.map(core::num::NonZeroU32::get),
                        "radio":format!("{:?}",peer.radio),"details":format!("{:?}",peer.details),
                    })).collect::<Vec<_>>();
                    println!(
                        "{}",
                        json!({"event":"interface_peers","id":hex::encode(id.as_bytes()),"page":index,"entries":entries,"rtt":format!("{rtt:?}")})
                    );
                    match peers.continuation() {
                        RemoteControlPeerContinuation::Complete => {
                            completion = Ok(());
                            break;
                        }
                        RemoteControlPeerContinuation::More(cursor) => {
                            page = RemoteControlPeerPage::After(cursor)
                        }
                    }
                }
                completion?;
            }
            Action::AppMessage => {
                let bytes = hex::decode(message_hex.as_deref().unwrap_or("0101"))
                    .map_err(|error| Error::Message(error.to_string()))?;
                let message = RemoteControlAppMessage::from_slice(&bytes)
                    .map_err(|_| Error::Message("request exceeds 96 bytes".to_owned()))?;
                stage.set("application message");
                let (response, rtt) = connection
                    .app_message(message)
                    .await
                    .map_err(Error::Operation)?;
                println!(
                    "{}",
                    json!({"event":"app_message","request_hex":hex::encode(bytes),"response_hex":hex::encode(response.as_slice()),"rtt":format!("{rtt:?}")})
                );
            }
            Action::Watch => {
                stage.set("interface watch admission");
                let stream_id = StreamId::new(0x3201).map_err(Error::StreamId)?;
                let (mut reader, rtt) = connection
                    .watch_interfaces(stream_id)
                    .await
                    .map_err(Error::WatchOpen)?;
                println!(
                    "{}",
                    json!({"event":"watch_admitted","stream_id":stream_id.get(),"rtt":format!("{rtt:?}")})
                );
                stage.set("interface watch stream");
                let deadline = std::time::Instant::now() + Duration::from_secs(12);
                loop {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(remaining, reader.next_event()).await {
                        Ok(Ok(event)) => {
                            let sequence = event.sequence();
                            let (kind, interface_id) = match event {
                                RemoteControlStreamEvent::InterfaceChanged {
                                    interface, ..
                                } => ("interface_changed", Some(hex::encode(interface.as_bytes()))),
                                RemoteControlStreamEvent::PeersChanged { interface, .. } => {
                                    ("peers_changed", Some(hex::encode(interface.as_bytes())))
                                }
                                RemoteControlStreamEvent::ResyncRequired { .. } => {
                                    ("resync_required", None)
                                }
                                RemoteControlStreamEvent::Heartbeat { .. } => ("heartbeat", None),
                            };
                            println!(
                                "{}",
                                json!({"event":"interface_watch","sequence":sequence,"kind":kind,"interface_id":interface_id})
                            );
                        }
                        Ok(Err(error)) => return Err(Error::WatchRead(error)),
                        Err(_) => break,
                    }
                }
            }
        }
        connection.close();
        Ok(())
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(45), operation) => result.map_err(|_| Error::Timeout(stage.get()))?,
        result = node.run() => Err(Error::Node(result)),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match run(Options::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("controller_failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
