mod args;
mod io;

use personal_rns::runtime::NoPersistence;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

pub use args::RncpArgs;
use io::{canonical_directory, expand_user_path, resolve_fetch, CpIoError, ReceiveTarget};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EstablishLinkFailure, IdentifyFailure,
    RatchetPolicy, SendRequestFailure, SetResourceStrategyFailure,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentityHash, IdentitySigner};
use personal_rns::rncp::{
    parse_fetch_path, parse_fetch_reply, write_fetch_path, write_fetch_reply, write_file_metadata,
    FetchReply, APP_NAME, FETCH_PATH, RECEIVE_ASPECT,
};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::{RequestHandlerError, RequestPathHash};
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy, RequestEndpointSet,
};
use personal_rns::runtime::{
    load_or_create_identity_secret, AnnounceNowError, IdentitySecretFileError, NodeRunError,
    PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle, ResourceAdmissionPeer,
    ResourceOfferAdmission, ResourceReceiveError, ResourceSendError, SegmentCompression, SendError,
    ServeMyRequestEndpoints,
};
use personal_rns::shared_instance::{
    connect_existing_shared_instance, ExistingSharedInstanceUnavailable,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;
use tokio::sync::{mpsc, oneshot};

use super::configuration::{LoadedConfiguration, UtilityConfigurationError};
use super::session::{
    UtilityNodeIdentity, UtilityNodeSession, UtilityNodeSessionError, UtilityNodeStopped,
    UtilityPathError,
};

struct FetchPlan {
    jail: Option<PathBuf>,
    compression: SegmentCompression,
}

struct ListenerState {
    handle: PrnsNodeHandle,
    fetch: Arc<FetchPlan>,
    events: mpsc::UnboundedSender<ListenerEvent>,
    allowed: Arc<[IdentityHash]>,
    no_auth: bool,
}

enum ListenerEvent {
    Established(LinkId),
    Identified(LinkId, IdentityHash),
    Closed(LinkId),
    ReceiverReady {
        link_id: LinkId,
        reply: oneshot::Sender<personal_rns::runtime::ResourceOfferMonitor>,
    },
}

struct LinkState {
    identity: Option<IdentityHash>,
    ready: Option<oneshot::Sender<personal_rns::runtime::ResourceOfferMonitor>>,
}

struct AuthenticatedFetch;
struct PublicFetch;

impl RequestEndpoint<ListenerState> for AuthenticatedFetch {
    const ENDPOINT_ID: &'static str = FETCH_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

    async fn handle(
        context: RequestContext<'_, ListenerState>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        handle_fetch(context).await
    }
}

impl RequestEndpoint<ListenerState> for PublicFetch {
    const ENDPOINT_ID: &'static str = FETCH_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        context: RequestContext<'_, ListenerState>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        handle_fetch(context).await
    }
}

async fn handle_fetch(mut context: RequestContext<'_, ListenerState>) -> Result<(), Decline> {
    let requested = match parse_fetch_path(context.data) {
        Ok(requested) => requested,
        Err(_) => return respond_fetch(&mut context, FetchReply::RemoteError),
    };
    let path = match resolve_fetch(requested, context.state.fetch.jail.as_deref()) {
        Ok(path) => path,
        Err(CpIoError::OutsideJail { .. }) => {
            return respond_fetch(&mut context, FetchReply::NotAllowed)
        }
        Err(CpIoError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return respond_fetch(&mut context, FetchReply::NotFound)
        }
        Err(_) => return respond_fetch(&mut context, FetchReply::RemoteError),
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return respond_fetch(&mut context, FetchReply::NotFound)
        }
        Err(_) => return respond_fetch(&mut context, FetchReply::RemoteError),
    };
    let len = match file.metadata().await {
        Ok(metadata) => metadata.len(),
        Err(_) => return respond_fetch(&mut context, FetchReply::RemoteError),
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return respond_fetch(&mut context, FetchReply::RemoteError);
    };
    let mut metadata = vec![0u8; name.len().saturating_add(8)];
    let metadata_len = match write_file_metadata(name.as_bytes(), &mut metadata) {
        Ok(len) => len,
        Err(_) => return respond_fetch(&mut context, FetchReply::RemoteError),
    };
    metadata.truncate(metadata_len);
    let handle = context.state.handle.clone();
    let compression = context.state.fetch.compression;
    let link_id = context.respond_token().link_id;
    let (progress, _) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let _ = handle
            .send_resource_with_options(link_id, len, file, &metadata, compression, progress)
            .await;
    });
    respond_fetch(&mut context, FetchReply::Found)
}

fn respond_fetch(
    context: &mut RequestContext<'_, ListenerState>,
    reply: FetchReply,
) -> Result<(), Decline> {
    let mut encoded = [0u8; 2];
    let len = write_fetch_reply(reply, &mut encoded).map_err(|_| Decline::Ignore)?;
    context.respond(&encoded[..len])
}

pub async fn run(args: RncpArgs) -> Result<(), RncpError> {
    if args.version {
        println!(
            "prnsd cp {} (RNS 1.4.2 compatibility)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args.listen || args.print_identity {
        return listen(args).await;
    }
    if args.fetch {
        if args.file.is_none() || args.destination.is_none() {
            print!("{}", crate::cli::cp_help());
            return Ok(());
        }
        return fetch(args).await;
    }
    if args.file.is_some() && args.destination.is_some() {
        return send(args).await;
    }
    print!("{}", crate::cli::cp_help());
    Ok(())
}

async fn send(args: RncpArgs) -> Result<(), RncpError> {
    let configuration =
        LoadedConfiguration::load(args.config.as_deref()).map_err(RncpError::Configuration)?;
    let secret = load_identity(&configuration, args.identity.as_deref())?;
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret).identity_hash();
    let file_path = canonical_file(args.file.as_deref().unwrap_or_default())?;
    let destination = args
        .destination
        .ok_or(RncpError::Arguments("destination is required"))?
        .destination();
    let file = tokio::fs::File::open(&file_path).await.map_err(|source| {
        RncpError::Io(CpIoError::Io {
            path: file_path.clone(),
            source,
        })
    })?;
    let len = file
        .metadata()
        .await
        .map_err(|source| {
            RncpError::Io(CpIoError::Io {
                path: file_path.clone(),
                source,
            })
        })?
        .len();
    let name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(RncpError::Arguments("file name is not valid UTF-8"))?;
    let metadata = encode_metadata(name)?;
    let timeout = args.timeout.get();
    let session = UtilityNodeSession::connect(
        &configuration,
        UtilityNodeIdentity::Private(secret),
        timeout,
    )
    .await
    .map_err(RncpError::Session)?;
    session
        .run(move |client| async move {
            client
                .ensure_path(destination, timeout)
                .await
                .map_err(RncpError::Path)?;
            let link = client
                .handle()
                .establish_link(destination)
                .await
                .map_err(RncpError::Link)?;
            client
                .handle()
                .identify(link, identity)
                .await
                .map_err(RncpError::Identify)?;
            let compression = if args.no_compress {
                SegmentCompression::Never
            } else {
                SegmentCompression::AUTO
            };
            let (progress, mut updates) = mpsc::unbounded_channel();
            let started = Instant::now();
            let transfer = client.handle().send_resource_with_options(
                link,
                len,
                file,
                &metadata,
                compression,
                progress,
            );
            tokio::pin!(transfer);
            loop {
                tokio::select! {
                    result = &mut transfer => {
                        result.map_err(RncpError::SendResource)?;
                        break;
                    }
                    Some(progress) = updates.recv() => {
                        if !args.silent {
                            print_progress("Transferring file", progress, started, args.phy_rates);
                        }
                    }
                }
            }
            client.handle().close_link(link);
            if !args.silent {
                println!(
                    "\n{} copied to {}",
                    file_path.display(),
                    pretty_hash(destination.as_bytes())
                );
            } else {
                println!(
                    "{} copied to {}",
                    file_path.display(),
                    pretty_hash(destination.as_bytes())
                );
            }
            Ok(())
        })
        .await
        .map_err(RncpError::NodeStopped)?
}

async fn fetch(args: RncpArgs) -> Result<(), RncpError> {
    let configuration =
        LoadedConfiguration::load(args.config.as_deref()).map_err(RncpError::Configuration)?;
    let secret = load_identity(&configuration, args.identity.as_deref())?;
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret).identity_hash();
    let destination = args
        .destination
        .ok_or(RncpError::Arguments("destination is required"))?
        .destination();
    let requested = args.file.clone().unwrap_or_default();
    let save = canonical_directory(args.save.as_deref().unwrap_or_else(|| Path::new(".")))
        .map_err(RncpError::Io)?;
    let target = ReceiveTarget::create(&save).map_err(RncpError::Io)?;
    let mut request = vec![0u8; requested.len().saturating_add(5)];
    let request_len = write_fetch_path(&requested, &mut request).map_err(RncpError::Codec)?;
    request.truncate(request_len);
    let timeout = args.timeout.get();
    let session = UtilityNodeSession::connect(
        &configuration,
        UtilityNodeIdentity::Private(secret),
        timeout,
    )
    .await
    .map_err(RncpError::Session)?;
    session
        .run(move |client| async move {
            client
                .ensure_path(destination, timeout)
                .await
                .map_err(RncpError::Path)?;
            let link = client
                .handle()
                .establish_link(destination)
                .await
                .map_err(RncpError::Link)?;
            client
                .handle()
                .identify(link, identity)
                .await
                .map_err(RncpError::Identify)?;
            let receiver = client
                .handle()
                .prepare_resource_receiver(link)
                .await
                .map_err(RncpError::ReceiveResource)?;
            client
                .handle()
                .set_link_resource_strategy(link, ResourceStrategy::AcceptIf)
                .await
                .map_err(RncpError::ResourceStrategy)?;
            let mut offers = client.handle().admit_resource_offers(
                link,
                ResourceOfferAdmission {
                    peer: ResourceAdmissionPeer::Any,
                    max_uncompressed_bytes: u64::MAX,
                    accept_compressed: true,
                },
            );
            let response = client
                .handle()
                .request(link, RequestPathHash::of(FETCH_PATH), &request)
                .await
                .map_err(RncpError::Request)?;
            match parse_fetch_reply(&response.0).map_err(RncpError::Codec)? {
                FetchReply::Found => {}
                FetchReply::NotFound => return Err(RncpError::FetchNotFound(requested)),
                FetchReply::NotAllowed => return Err(RncpError::FetchNotAllowed(requested)),
                FetchReply::RemoteError => return Err(RncpError::FetchRemoteError),
            }
            let mut target = target;
            let started = Instant::now();
            let receipt = {
                let receive = receiver.receive(&mut target.file);
                tokio::pin!(receive);
                loop {
                    tokio::select! {
                        result = &mut receive => break result.map_err(RncpError::ReceiveResource)?,
                        offer = offers.recv() => {
                            if let Some(offer) = offer {
                                if !args.silent {
                                    print_offer_progress("Transferring file", offer.uncompressed_data_bytes, offer.sealed_transfer_bytes as u64, started, args.phy_rates);
                                }
                            }
                        }
                    }
                }
            };
            client.handle().deny_resource_offers(link);
            client.handle().close_link(link);
            let metadata = receipt.metadata.ok_or(RncpError::MissingMetadata)?;
            let published = target
                .publish(&metadata, args.overwrite)
                .await
                .map_err(RncpError::Io)?;
            if !args.silent {
                println!("\n{} fetched from {}", published.display(), pretty_hash(destination.as_bytes()));
            } else {
                println!("{} fetched from {}", published.display(), pretty_hash(destination.as_bytes()));
            }
            Ok(())
        })
        .await
        .map_err(RncpError::NodeStopped)?
}

async fn listen(mut args: RncpArgs) -> Result<(), RncpError> {
    let configuration =
        LoadedConfiguration::load(args.config.as_deref()).map_err(RncpError::Configuration)?;
    let secret = load_identity(&configuration, args.identity.as_deref())?;
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret).identity_hash();
    let destination = personal_rns::routing::announce::derive_single_destination_hash(
        &identity,
        APP_NAME,
        &[RECEIVE_ASPECT],
    )
    .map_err(RncpError::Destination)?;
    if args.print_identity {
        println!("Identity     : {}", pretty_hash(identity.as_bytes()));
        println!("Listening on : {}", pretty_hash(destination.as_bytes()));
        return Ok(());
    }
    load_allowed_identities(&mut args.allowed)?;
    if args.allowed.is_empty() && !args.no_auth {
        eprintln!("prnsd cp: no allowed identities configured; no files will be accepted");
    }
    let save = canonical_directory(args.save.as_deref().unwrap_or_else(|| Path::new(".")))
        .map_err(RncpError::Io)?;
    let jail = args
        .jail
        .as_deref()
        .map(canonical_directory)
        .transpose()
        .map_err(RncpError::Io)?;
    let compression = if args.no_compress {
        SegmentCompression::Never
    } else {
        SegmentCompression::AUTO
    };
    let fetch = Arc::new(FetchPlan { jail, compression });
    if args.allow_fetch {
        if args.no_auth {
            listen_with_routes(
                args,
                configuration,
                secret,
                destination,
                save,
                fetch,
                || personal_rns::request_endpoints![PublicFetch],
            )
            .await
        } else {
            listen_with_routes(
                args,
                configuration,
                secret,
                destination,
                save,
                fetch,
                || personal_rns::request_endpoints![AuthenticatedFetch],
            )
            .await
        }
    } else {
        listen_with_routes(
            args,
            configuration,
            secret,
            destination,
            save,
            fetch,
            || personal_rns::request_endpoints![],
        )
        .await
    }
}

async fn listen_with_routes<R, F>(
    args: RncpArgs,
    configuration: LoadedConfiguration,
    secret: personal_rns::identity::Zeroizing<
        [u8; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    >,
    destination: DestinationHash,
    save: PathBuf,
    fetch: Arc<FetchPlan>,
    make_request_endpoints: F,
) -> Result<(), RncpError>
where
    R: RequestEndpointSet<ListenerState>,
    F: FnOnce() -> R,
{
    let (events, receiver) = mpsc::unbounded_channel();
    let listener_allowed: Arc<[IdentityHash]> = args.allowed.clone().into();
    let listener_no_auth = args.no_auth;
    let mut node = PrnsNode::new_with_handle(move |handle| personal_rns::runtime::PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            app_name: APP_NAME,
            aspects: &[RECEIVE_ASPECT],
            identity: secret,
            announce_app_data: &[],
            proof: ProofStrategy::ProveNone,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptIf,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::Yes,
        }],
        app_state: ListenerState {
            handle,
            fetch,
            events,
            allowed: listener_allowed,
            no_auth: listener_no_auth,
        },
        storage: GrowableHeap,
        request_endpoints: make_request_endpoints(),
        interfaces: personal_rns::runtime::ManuallyAttached,
        on_event: listener_event,
        persistence: NoPersistence,
    });
    if args.allow_fetch && !args.no_auth {
        for identity in &args.allowed {
            node.allow_requester(&destination, FETCH_PATH, *identity)
                .map_err(RncpError::RequestAcl)?;
        }
    }
    let handle = node.handle();
    let bus = configuration
        .local_bus_client_intent()
        .map_err(RncpError::Configuration)?;
    connect_existing_shared_instance(&handle, bus)
        .await
        .map_err(RncpError::SharedInstance)?;
    println!("cp listening on {}", pretty_hash(destination.as_bytes()));
    let announce_interval = args.announce;
    let serving = serve_links(handle.clone(), receiver, args, save);
    let announcing = announce_loop(handle, destination, announce_interval);
    tokio::select! {
        result = node.run() => match result {
            Ok(()) => Err(RncpError::ListenerStopped),
            Err(error) => Err(RncpError::ListenerPanicked(error)),
        },
        result = serving => result,
        result = announcing => result,
    }
}

fn listener_event(event: PrnsEvent<'_>, state: &ListenerState) {
    match event {
        PrnsEvent::Diagnostic(personal_rns::runtime::Diagnostic::LinkEstablished(established)) => {
            let peer = if state.no_auth {
                ResourceAdmissionPeer::Any
            } else {
                ResourceAdmissionPeer::AuthenticatedOneOf(state.allowed.clone())
            };
            let _ = state.handle.admit_resource_offers(
                established.link_id,
                ResourceOfferAdmission {
                    peer,
                    max_uncompressed_bytes: u64::MAX,
                    accept_compressed: true,
                },
            );
            let _ = state
                .events
                .send(ListenerEvent::Established(established.link_id));
        }
        PrnsEvent::Diagnostic(personal_rns::runtime::Diagnostic::PeerIdentified {
            link_id,
            identity,
        }) => {
            let _ = state
                .events
                .send(ListenerEvent::Identified(link_id, identity));
        }
        PrnsEvent::Diagnostic(personal_rns::runtime::Diagnostic::LinkClosed {
            link_id, ..
        }) => {
            let _ = state.events.send(ListenerEvent::Closed(link_id));
        }
        _ => {}
    }
}

async fn serve_links(
    handle: PrnsNodeHandle,
    mut events: mpsc::UnboundedReceiver<ListenerEvent>,
    args: RncpArgs,
    save: PathBuf,
) -> Result<(), RncpError> {
    let mut links: HashMap<LinkId, LinkState> = HashMap::new();
    let (internal, mut internal_events) = mpsc::unbounded_channel();
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(event) => handle_listener_event(event, &handle, &args, &mut links, &save, &internal),
                None => return Err(RncpError::ListenerStopped),
            },
            event = internal_events.recv() => match event {
                Some(event) => handle_listener_event(event, &handle, &args, &mut links, &save, &internal),
                None => return Err(RncpError::ListenerStopped),
            }
        }
    }
}

fn handle_listener_event(
    event: ListenerEvent,
    handle: &PrnsNodeHandle,
    args: &RncpArgs,
    links: &mut HashMap<LinkId, LinkState>,
    save: &Path,
    events: &mpsc::UnboundedSender<ListenerEvent>,
) {
    match event {
        ListenerEvent::Established(link_id) => {
            links.insert(
                link_id,
                LinkState {
                    identity: None,
                    ready: None,
                },
            );
            tokio::spawn(receive_on_link(
                handle.clone(),
                link_id,
                save.to_owned(),
                args.overwrite,
                args.phy_rates,
                events.clone(),
            ));
        }
        ListenerEvent::Identified(link_id, identity) => {
            if !args.no_auth && !args.allowed.contains(&identity) {
                handle.close_link(link_id);
                return;
            }
            if let Some(link) = links.get_mut(&link_id) {
                link.identity = Some(identity);
                activate_receiver(handle, link_id, link, args.no_auth);
            }
        }
        ListenerEvent::Closed(link_id) => {
            handle.deny_resource_offers(link_id);
            links.remove(&link_id);
        }
        ListenerEvent::ReceiverReady { link_id, reply } => {
            if let Some(link) = links.get_mut(&link_id) {
                link.ready = Some(reply);
                activate_receiver(handle, link_id, link, args.no_auth);
            }
        }
    }
}

fn activate_receiver(
    handle: &PrnsNodeHandle,
    link_id: LinkId,
    link: &mut LinkState,
    no_auth: bool,
) {
    let peer = if no_auth {
        ResourceAdmissionPeer::Any
    } else if let Some(identity) = link.identity {
        ResourceAdmissionPeer::Authenticated(identity)
    } else {
        return;
    };
    let Some(reply) = link.ready.take() else {
        return;
    };
    let monitor = handle.admit_resource_offers(
        link_id,
        ResourceOfferAdmission {
            peer,
            max_uncompressed_bytes: u64::MAX,
            accept_compressed: true,
        },
    );
    let _ = reply.send(monitor);
}

async fn receive_on_link(
    handle: PrnsNodeHandle,
    link_id: LinkId,
    save: PathBuf,
    overwrite: bool,
    phy_rates: bool,
    events: mpsc::UnboundedSender<ListenerEvent>,
) {
    loop {
        let Ok(mut target) = ReceiveTarget::create(&save) else {
            handle.close_link(link_id);
            return;
        };
        let Ok(receiver) = handle.prepare_resource_receiver(link_id).await else {
            return;
        };
        let (reply, ready) = oneshot::channel();
        if events
            .send(ListenerEvent::ReceiverReady { link_id, reply })
            .is_err()
        {
            return;
        }
        let Ok(mut offers) = ready.await else {
            return;
        };
        let started = Instant::now();
        let receipt = {
            let receive = receiver.receive(&mut target.file);
            tokio::pin!(receive);
            loop {
                tokio::select! {
                    result = &mut receive => match result {
                        Ok(receipt) => break receipt,
                        Err(_) => return,
                    },
                    offer = offers.recv() => {
                        if let Some(offer) = offer {
                            print_offer_progress("Receiving file", offer.uncompressed_data_bytes, offer.sealed_transfer_bytes as u64, started, phy_rates);
                        }
                    }
                }
            }
        };
        handle.deny_resource_offers(link_id);
        let Some(metadata) = receipt.metadata else {
            continue;
        };
        match target.publish(&metadata, overwrite).await {
            Ok(path) => println!("\nSaved received file to {}", path.display()),
            Err(error) => eprintln!("prnsd cp: could not save received file: {error}"),
        }
    }
}

async fn announce_loop(
    handle: PrnsNodeHandle,
    destination: DestinationHash,
    interval_seconds: i64,
) -> Result<(), RncpError> {
    loop {
        handle
            .announce_now(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })
            .await
            .map_err(RncpError::Announce)?;
        if interval_seconds <= 0 {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval_seconds as u64)).await;
    }
}

fn load_identity(
    configuration: &LoadedConfiguration,
    explicit: Option<&Path>,
) -> Result<personal_rns::identity::vault::IdentitySecretKey, RncpError> {
    let path = match explicit {
        Some(path) => expand_user_path(path).map_err(RncpError::Io)?,
        None => configuration
            .discovered
            .dir
            .join("storage")
            .join("identities")
            .join(APP_NAME),
    };
    load_or_create_identity_secret(&path).map_err(|source| RncpError::Identity { path, source })
}

fn load_allowed_identities(allowed: &mut Vec<IdentityHash>) -> Result<(), RncpError> {
    let candidates = [
        PathBuf::from("/etc/rncp/allowed_identities"),
        expand_user_path(Path::new("~/.config/rncp/allowed_identities")).map_err(RncpError::Io)?,
        expand_user_path(Path::new("~/.rncp/allowed_identities")).map_err(RncpError::Io)?,
    ];
    let Some(path) = candidates.into_iter().find(|path| path.is_file()) else {
        return Ok(());
    };
    let text = std::fs::read_to_string(&path).map_err(|source| {
        RncpError::Io(CpIoError::Io {
            path: path.clone(),
            source,
        })
    })?;
    for line in text.lines().map(str::trim).filter(|line| line.len() == 32) {
        let identity = super::arguments::parse_identity_hash(line)
            .map_err(|_| RncpError::AllowedIdentity(path.clone()))?;
        if !allowed.contains(&identity) {
            allowed.push(identity);
        }
    }
    Ok(())
}

fn canonical_file(path: &str) -> Result<PathBuf, RncpError> {
    let path = expand_user_path(Path::new(path)).map_err(RncpError::Io)?;
    let path = std::fs::canonicalize(&path).map_err(|source| {
        RncpError::Io(CpIoError::Io {
            path: path.clone(),
            source,
        })
    })?;
    if !path.is_file() {
        return Err(RncpError::Io(CpIoError::NotAFile(path)));
    }
    Ok(path)
}

fn encode_metadata(name: &str) -> Result<Vec<u8>, RncpError> {
    let mut metadata = vec![0u8; name.len().saturating_add(8)];
    let len = write_file_metadata(name.as_bytes(), &mut metadata).map_err(RncpError::Codec)?;
    metadata.truncate(len);
    Ok(metadata)
}

fn print_progress(
    label: &str,
    progress: personal_rns::runtime::ResourceProgress,
    started: Instant,
    phy_rates: bool,
) {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let percent = if progress.total_bytes == 0 {
        100.0
    } else {
        progress.transferred_bytes as f64 * 100.0 / progress.total_bytes as f64
    };
    let physical = if phy_rates {
        format!(
            " ({}ps at physical layer)",
            size_string(progress.physical_transferred_bytes as f64 / elapsed, true,)
        )
    } else {
        String::new()
    };
    print!(
        "\r\x1b[2K{label} {percent:.1}% - {} of {} - {}ps{physical}",
        size_string(progress.transferred_bytes as f64, false),
        size_string(progress.total_bytes as f64, false),
        size_string(progress.transferred_bytes as f64 / elapsed, true),
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn print_offer_progress(
    label: &str,
    logical: u64,
    physical: u64,
    started: Instant,
    phy_rates: bool,
) {
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let physical = if phy_rates {
        format!(
            " ({}ps at physical layer)",
            size_string(physical as f64 / elapsed, true)
        )
    } else {
        String::new()
    };
    print!(
        "\r\x1b[2K{label} - {} - {}ps{physical}",
        size_string(logical as f64, false),
        size_string(logical as f64 / elapsed, true),
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn size_string(mut value: f64, bits: bool) -> String {
    if bits {
        value *= 8.0;
    }
    let suffix = if bits { "b" } else { "B" };
    for unit in ["", "K", "M", "G", "T", "P", "E", "Z"] {
        if value.abs() < 1000.0 {
            return if unit.is_empty() {
                format!("{value:.0} {unit}{suffix}")
            } else {
                format!("{value:.2} {unit}{suffix}")
            };
        }
        value /= 1000.0;
    }
    format!("{value:.2}Y{suffix}")
}

fn pretty_hash(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("<{hex}>")
}

#[derive(Debug)]
pub enum RncpError {
    Arguments(&'static str),
    Configuration(UtilityConfigurationError),
    Identity {
        path: PathBuf,
        source: IdentitySecretFileError,
    },
    Destination(personal_rns::routing::announce::ExpandNameError),
    Session(UtilityNodeSessionError),
    SharedInstance(ExistingSharedInstanceUnavailable),
    NodeStopped(UtilityNodeStopped),
    ListenerStopped,
    ListenerPanicked(NodeRunError),
    Path(UtilityPathError),
    Link(SendError<EstablishLinkFailure>),
    Identify(SendError<IdentifyFailure>),
    ResourceStrategy(SendError<SetResourceStrategyFailure>),
    Request(SendError<SendRequestFailure>),
    SendResource(ResourceSendError),
    ReceiveResource(ResourceReceiveError),
    RequestAcl(RequestHandlerError),
    Announce(AnnounceNowError),
    Codec(personal_rns::rncp::RncpCodecError),
    Io(CpIoError),
    AllowedIdentity(PathBuf),
    FetchNotFound(String),
    FetchNotAllowed(String),
    FetchRemoteError,
    MissingMetadata,
}

impl fmt::Display for RncpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(message) => formatter.write_str(message),
            Self::Configuration(source) => source.fmt(formatter),
            Self::Identity { path, source } => {
                write!(
                    formatter,
                    "could not load identity {}: {source}",
                    path.display()
                )
            }
            Self::Destination(source) => write!(formatter, "invalid RNCP destination: {source:?}"),
            Self::Session(source) => source.fmt(formatter),
            Self::SharedInstance(source) => source.fmt(formatter),
            Self::NodeStopped(source) => source.fmt(formatter),
            Self::ListenerStopped => formatter.write_str("RNCP listener stopped"),
            Self::ListenerPanicked(source) => write!(formatter, "RNCP listener stopped: {source}"),
            Self::Path(source) => source.fmt(formatter),
            Self::Link(source) => write!(formatter, "link establishment failed: {source:?}"),
            Self::Identify(source) => write!(formatter, "link identification failed: {source:?}"),
            Self::ResourceStrategy(source) => {
                write!(formatter, "resource admission failed: {source:?}")
            }
            Self::Request(source) => write!(formatter, "fetch request failed: {source:?}"),
            Self::SendResource(source) => write!(formatter, "resource send failed: {source:?}"),
            Self::ReceiveResource(source) => {
                write!(formatter, "resource receive failed: {source:?}")
            }
            Self::RequestAcl(source) => {
                write!(formatter, "could not configure fetch ACL: {source:?}")
            }
            Self::Announce(source) => write!(formatter, "RNCP announce failed: {source:?}"),
            Self::Codec(source) => write!(formatter, "invalid RNCP value: {source:?}"),
            Self::Io(source) => source.fmt(formatter),
            Self::AllowedIdentity(path) => {
                write!(formatter, "invalid identity in {}", path.display())
            }
            Self::FetchNotFound(path) => write!(
                formatter,
                "fetch failed; {path} was not found on the remote"
            ),
            Self::FetchNotAllowed(path) => {
                write!(formatter, "fetching {path} was not allowed by the remote")
            }
            Self::FetchRemoteError => formatter.write_str("fetch failed on the remote system"),
            Self::MissingMetadata => {
                formatter.write_str("received resource has no RNCP filename metadata")
            }
        }
    }
}

impl std::error::Error for RncpError {}
