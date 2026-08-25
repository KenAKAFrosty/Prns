use personal_rns::runtime::NoPersistence;
use std::path::PathBuf;
use std::sync::Arc;

use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, RatchetPolicy};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::identity::{IdentityHash, IdentitySigner};
use personal_rns::rnx::{
    ExecutionRequestRef, APP_NAME, COMMAND_PATH, EXECUTE_ASPECT, MAX_EXECUTION_REQUEST_BYTES,
};
use personal_rns::routing::announce::derive_single_destination_hash;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::rnx::{
    HeapRnxOutput, RnxAuthorization, RnxCommandHandler, RnxCompletion, RnxOutput,
    RnxRequestEndpoint,
};
use personal_rns::runtime::{
    Diagnostic, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle, ProcessCommands,
    ResourceAdmissionPeer, ResourceOfferAdmission, ServeMyRequestEndpoints,
};
use personal_rns::shared_instance::connect_existing_shared_instance;
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;
use tokio::sync::Semaphore;

use crate::utilities::configuration::LoadedConfiguration;

use super::identity::{home_directory, load_identity, pretty_hash};
use super::{RnxArgs, RnxError};

const MAX_CONCURRENT_COMMANDS: usize = 8;

struct ListenerState {
    handle: PrnsNodeHandle,
    destination: DestinationHash,
    allowed: Arc<[IdentityHash]>,
    no_auth: bool,
    execution_slots: Semaphore,
}

struct RnxCommand;
struct PublicRnxCommand;
type RnxEndpoint = RnxRequestEndpoint<RnxCommand>;
type PublicRnxEndpoint = RnxRequestEndpoint<PublicRnxCommand>;

impl RnxCommandHandler<ListenerState> for RnxCommand {
    const AUTHORIZATION: RnxAuthorization = RnxAuthorization::AllowList(&[]);
    type Output = HeapRnxOutput;

    fn destination(state: &ListenerState) -> DestinationHash {
        state.destination
    }

    async fn execute(
        state: &ListenerState,
        request: ExecutionRequestRef<'_>,
        output: &mut RnxOutput<'_>,
    ) -> RnxCompletion {
        execute_process(state, request, output).await
    }
}

impl RnxCommandHandler<ListenerState> for PublicRnxCommand {
    const AUTHORIZATION: RnxAuthorization = RnxAuthorization::Public;
    type Output = HeapRnxOutput;

    fn destination(state: &ListenerState) -> DestinationHash {
        state.destination
    }

    async fn execute(
        state: &ListenerState,
        request: ExecutionRequestRef<'_>,
        output: &mut RnxOutput<'_>,
    ) -> RnxCompletion {
        execute_process(state, request, output).await
    }
}

async fn execute_process(
    state: &ListenerState,
    request: ExecutionRequestRef<'_>,
    output: &mut RnxOutput<'_>,
) -> RnxCompletion {
    let Ok(permit) = state.execution_slots.acquire().await else {
        return ProcessCommands::not_executed_now();
    };
    let result = ProcessCommands::execute(request, output).await;
    drop(permit);
    result
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
        listen_with_endpoints(args, configuration, secret, destination, || {
            personal_rns::request_endpoints![PublicRnxEndpoint]
        })
        .await
    } else {
        listen_with_endpoints(args, configuration, secret, destination, || {
            personal_rns::request_endpoints![RnxEndpoint]
        })
        .await
    }
}

async fn listen_with_endpoints<R, F>(
    args: RnxArgs,
    configuration: LoadedConfiguration,
    secret: IdentitySecretKey,
    destination: DestinationHash,
    make_request_endpoints: F,
) -> Result<(), RnxError>
where
    R: RequestEndpointSet<ListenerState>,
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
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::Yes,
        }],
        app_state: ListenerState {
            handle,
            destination,
            allowed,
            no_auth,
            execution_slots: Semaphore::new(MAX_CONCURRENT_COMMANDS),
        },
        storage: GrowableHeap,
        request_endpoints: make_request_endpoints(),
        interfaces: personal_rns::runtime::ManuallyAttached,
        on_event: listener_event,
        persistence: NoPersistence,
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
        result = node.run() => match result {
            Ok(()) => Err(RnxError::ListenerStopped),
            Err(error) => Err(RnxError::ListenerPanicked(error)),
        },
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
                    max_uncompressed_bytes: MAX_EXECUTION_REQUEST_BYTES as u64,
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
