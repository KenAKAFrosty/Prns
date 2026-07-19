use std::path::PathBuf;
use std::sync::Arc;

use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, RatchetPolicy};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::identity::{IdentityHash, IdentitySigner};
use personal_rns::rnx::{
    decode_execution_request, encode_execution_result, APP_NAME, COMMAND_PATH, EXECUTE_ASPECT,
    MAX_EXECUTION_REQUEST_BYTES,
};
use personal_rns::routing::announce::derive_single_destination_hash;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_router::{
    Decline, RequestContext, RequestRoute, RoutePolicy, RouteSet,
};
use personal_rns::runtime::{
    Diagnostic, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    RequestHandlerRegistration, ResourceAdmissionPeer, ResourceOfferAdmission,
};
use personal_rns::shared_instance::connect_existing_shared_instance;
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;
use tokio::sync::Semaphore;

use crate::utilities::configuration::LoadedConfiguration;

use super::execution;
use super::identity::{home_directory, load_identity, pretty_hash};
use super::{RnxArgs, RnxError};

const MAX_CONCURRENT_COMMANDS: usize = 8;

struct ListenerState {
    handle: PrnsNodeHandle,
    allowed: Arc<[IdentityHash]>,
    no_auth: bool,
    execution_slots: Semaphore,
}

struct AuthenticatedCommand;
struct PublicCommand;

impl RequestRoute<ListenerState> for AuthenticatedCommand {
    const PATH: &'static str = COMMAND_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowList(&[]);

    async fn handle(context: RequestContext<'_, ListenerState>) -> Result<(), Decline> {
        handle_command(context).await
    }
}

impl RequestRoute<ListenerState> for PublicCommand {
    const PATH: &'static str = COMMAND_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;

    async fn handle(context: RequestContext<'_, ListenerState>) -> Result<(), Decline> {
        handle_command(context).await
    }
}

async fn handle_command(mut context: RequestContext<'_, ListenerState>) -> Result<(), Decline> {
    let request = decode_execution_request(context.data).map_err(|_| Decline::Ignore)?;
    let permit = context
        .state
        .execution_slots
        .acquire()
        .await
        .map_err(|_| Decline::Ignore)?;
    let result = execution::execute(request).await;
    drop(permit);
    let response = encode_execution_result(&result).map_err(|_| Decline::Ignore)?;
    context.respond(&response)
}

pub(super) async fn run(mut args: RnxArgs) -> Result<(), RnxError> {
    let configuration =
        LoadedConfiguration::load(args.config.as_deref()).map_err(RnxError::Configuration)?;
    let secret = load_identity(&configuration, args.identity.as_deref())?;
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret).identity_hash();
    let destination = derive_single_destination_hash(&identity, APP_NAME, &[EXECUTE_ASPECT])
        .map_err(RnxError::Destination)?;
    if args.print_identity {
        println!("Identity     : {}", pretty_hash(identity.as_bytes()));
        println!("Listening on : {}", pretty_hash(destination.as_bytes()));
        return Ok(());
    }
    load_allowed_identities(&mut args.allowed)?;
    if args.allowed.is_empty() && !args.no_auth {
        eprintln!("prnsd x: no allowed identities configured; no commands will be accepted");
    }
    if args.no_auth {
        listen_with_routes(args, configuration, secret, destination, || {
            personal_rns::routes![PublicCommand]
        })
        .await
    } else {
        listen_with_routes(args, configuration, secret, destination, || {
            personal_rns::routes![AuthenticatedCommand]
        })
        .await
    }
}

async fn listen_with_routes<R, F>(
    args: RnxArgs,
    configuration: LoadedConfiguration,
    secret: IdentitySecretKey,
    destination: DestinationHash,
    make_routes: F,
) -> Result<(), RnxError>
where
    R: RouteSet<ListenerState>,
    F: FnOnce() -> R,
{
    let allowed: Arc<[IdentityHash]> = args.allowed.clone().into();
    let no_auth = args.no_auth;
    let mut node = PrnsNode::new_with_handle(move |handle| personal_rns::runtime::PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            app_name: APP_NAME,
            aspects: &[EXECUTE_ASPECT],
            identity: secret,
            announce_app_data: &[],
            proof: ProofStrategy::ProveNone,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptIf,
            request_handlers: RequestHandlerRegistration::NodeRouteSet,
        }],
        app_state: ListenerState {
            handle,
            allowed,
            no_auth,
            execution_slots: Semaphore::new(MAX_CONCURRENT_COMMANDS),
        },
        storage: GrowableHeap,
        routes: make_routes(),
        interfaces: personal_rns::runtime::Manual,
        on_event: listener_event,
    });
    if !args.no_auth {
        for identity in &args.allowed {
            node.allow_requester(&destination, COMMAND_PATH, *identity)
                .map_err(RnxError::RequestAcl)?;
        }
    }
    let handle = node.handle();
    let bus = configuration
        .local_bus_client_intent()
        .map_err(RnxError::Configuration)?;
    connect_existing_shared_instance(&handle, bus)
        .await
        .map_err(RnxError::SharedInstance)?;
    println!("x listening on {}", pretty_hash(destination.as_bytes()));
    let serving = async move {
        if !args.no_announce {
            handle
                .announce_now(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                })
                .await
                .map_err(RnxError::Announce)?;
        }
        std::future::pending::<Result<(), RnxError>>().await
    };
    tokio::select! {
        () = node.run() => Err(RnxError::ListenerStopped),
        result = serving => result,
    }
}

fn listener_event(event: PrnsEvent<'_>, state: &ListenerState) {
    match event {
        PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(established)) => {
            let peer = if state.no_auth {
                ResourceAdmissionPeer::Any
            } else {
                ResourceAdmissionPeer::AuthenticatedOneOf(state.allowed.clone())
            };
            let _ = state.handle.admit_resource_offers(
                established.link_id,
                ResourceOfferAdmission {
                    peer,
                    max_uncompressed_len: MAX_EXECUTION_REQUEST_BYTES as u64,
                    accept_compressed: true,
                },
            );
        }
        PrnsEvent::Diagnostic(Diagnostic::PeerIdentified { link_id, identity }) => {
            if !state.no_auth && !state.allowed.contains(&identity) {
                state.handle.close_link(link_id);
            }
        }
        PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, .. }) => {
            state.handle.deny_resource_offers(link_id);
        }
        _ => {}
    }
}

fn load_allowed_identities(allowed: &mut Vec<IdentityHash>) -> Result<(), RnxError> {
    let mut candidates = vec![PathBuf::from("/etc/rnx/allowed_identities")];
    if let Some(home) = home_directory() {
        candidates.push(home.join(".config/rnx/allowed_identities"));
        candidates.push(home.join(".rnx/allowed_identities"));
    }
    let Some(path) = candidates.into_iter().find(|path| path.is_file()) else {
        return Ok(());
    };
    let text = std::fs::read_to_string(&path).map_err(|source| RnxError::Io {
        path: path.clone(),
        source,
    })?;
    for line in text.lines().map(str::trim).filter(|line| line.len() == 32) {
        let identity = crate::utilities::arguments::parse_identity_hash(line)
            .map_err(|_| RnxError::AllowedIdentity(path.clone()))?;
        if !allowed.contains(&identity) {
            allowed.push(identity);
        }
    }
    Ok(())
}
