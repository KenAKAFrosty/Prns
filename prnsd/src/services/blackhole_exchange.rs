use std::fmt;
use std::time::Duration;

use personal_rns::config::BlackholeExchangePlan;
use personal_rns::engine::RatchetPolicy;
use personal_rns::engine::{EstablishLinkFailure, SendRequestFailure};
use personal_rns::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_management::{RnsBlackholeDecodeError, RnsBlackholeTable};
use personal_rns::manifold::tokio::TokioHost;
use personal_rns::manifold::Host;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{BlackholeIdentityOutcome, LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy, RequestEndpointSet,
};
use personal_rns::runtime::{
    ConfigurePreconfiguredDestinationError, IdentityBlackholeControl,
    IdentityBlackholeControlError, IdentityBlackholeSource, PreConfiguredDestination, PrnsEvent,
    PrnsNode, PrnsNodeHandle, RegisterRequestEndpointError, SendError, ServeMyRequestEndpoints,
};
use personal_rns::shared_instance::{RnsBlackholeFileError, RnsBlackholeFiles};
use personal_rns::storage::StorageLayout;
use personal_rns::wire::DestinationHash;

use super::request_state::DaemonRequestState;

pub const LIST_PATH: &str = "/list";
const INITIAL_WAIT: Duration = Duration::from_secs(20);
const JOB_INTERVAL: Duration = Duration::from_secs(60);

pub struct ListRoute;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationError {
    Destination(ConfigurePreconfiguredDestinationError),
    Route(RegisterRequestEndpointError),
}

#[derive(Debug)]
enum BlackholeUpdateError {
    Link(SendError<EstablishLinkFailure>),
    Request(SendError<SendRequestFailure>),
    Decode(RnsBlackholeDecodeError),
    Apply {
        identity: IdentityHash,
        source: IdentityBlackholeControlError,
    },
    Persist(RnsBlackholeFileError),
}

impl fmt::Display for BlackholeUpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Link(source) => write!(formatter, "link establishment failed: {source:?}"),
            Self::Request(source) => write!(formatter, "list request failed: {source:?}"),
            Self::Decode(source) => write!(formatter, "invalid blackhole list: {source}"),
            Self::Apply { identity, source } => write!(
                formatter,
                "blackhole table rejected identity {:?}: {source:?}",
                identity.as_bytes()
            ),
            Self::Persist(source) => {
                write!(formatter, "could not persist blackhole list: {source}")
            }
        }
    }
}

impl RequestEndpoint<DaemonRequestState> for ListRoute {
    const ENDPOINT_ID: &'static str = LIST_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, DaemonRequestState>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let entries = context
            .state
            .handle()
            .blackholed_identities()
            .await
            .map_err(|_| Decline::Ignore)?;
        let response = RnsBlackholeTable::from_entries(entries)
            .encode_message_pack()
            .map_err(|_| Decline::Ignore)?;
        context.respond(response)
    }
}

pub fn activate<R, F, S>(
    node: &mut PrnsNode<DaemonRequestState, R, F, S>,
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
) -> Result<DestinationHash, ActivationError>
where
    R: RequestEndpointSet<DaemonRequestState>,
    F: FnMut(PrnsEvent<'_>, &DaemonRequestState),
    S: StorageLayout,
{
    let destination = node
        .register_preconfigured_destination(PreConfiguredDestination::Single {
            app_name: "rnstransport",
            aspects: &["info", "blackhole"],
            identity,
            announce_app_data: &[],
            proof: ProofStrategy::ProveNone,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        })
        .map_err(ActivationError::Destination)?;
    node.register_request_route::<ListRoute>(&destination)
        .map_err(ActivationError::Route)?;
    Ok(destination)
}

pub struct BlackholeUpdateTask(tokio::task::JoinHandle<()>);

impl BlackholeUpdateTask {
    pub async fn shutdown(self) {
        self.0.abort();
        let _ = self.0.await;
    }
}

pub fn spawn_updater(
    handle: PrnsNodeHandle,
    clock: TokioHost,
    files: RnsBlackholeFiles,
    plan: &BlackholeExchangePlan,
) -> Option<BlackholeUpdateTask> {
    let sources = plan.sources().to_vec();
    if sources.is_empty() {
        return None;
    }
    let update_interval = plan.update_interval().duration();
    Some(BlackholeUpdateTask(tokio::spawn(async move {
        tokio::time::sleep(INITIAL_WAIT).await;
        let mut last_updates = vec![None; sources.len()];
        loop {
            let now = tokio::time::Instant::now();
            for (source, last_update) in sources.iter().zip(&mut last_updates) {
                if !update_is_due(now, *last_update, update_interval) {
                    continue;
                }
                match destination_for(*source) {
                    Ok(destination) => {
                        if handle.route(destination).await.is_none()
                            && handle.request_path(destination).await.is_err()
                        {
                            tracing::debug!(
                                event = "blackhole_update_path_unavailable",
                                source = ?source.as_bytes(),
                            );
                            continue;
                        }
                        *last_update = Some(tokio::time::Instant::now());
                        if let Err(error) =
                            update_source(&handle, &clock, &files, *source, destination).await
                        {
                            tracing::warn!(
                                event = "blackhole_update_failed",
                                source = ?source.as_bytes(),
                                error = %error,
                            );
                        }
                    }
                    Err(error) => tracing::warn!(
                        event = "blackhole_update_destination_failed",
                        source = ?source.as_bytes(),
                        error = ?error,
                    ),
                }
            }
            tokio::time::sleep(JOB_INTERVAL).await;
        }
    })))
}

fn update_is_due(
    now: tokio::time::Instant,
    last_update: Option<tokio::time::Instant>,
    update_interval: Duration,
) -> bool {
    match last_update {
        None => true,
        Some(last) => last
            .checked_add(update_interval)
            .is_some_and(|next| now > next),
    }
}

fn destination_for(source: IdentityHash) -> Result<DestinationHash, ExpandNameError> {
    derive_single_destination_hash(&source, "rnstransport", &["info", "blackhole"])
}

async fn update_source(
    handle: &PrnsNodeHandle,
    clock: &TokioHost,
    files: &RnsBlackholeFiles,
    source: IdentityHash,
    destination: DestinationHash,
) -> Result<(), BlackholeUpdateError> {
    let link = handle
        .establish_link(destination)
        .await
        .map_err(BlackholeUpdateError::Link)?;
    let response = handle
        .request(link, RequestPathHash::of(LIST_PATH), &[])
        .await;
    handle.close_link(link);
    let (response, _) = response.map_err(BlackholeUpdateError::Request)?;
    let entries = RnsBlackholeTable::decode_published_table(&response, clock.now())
        .map(RnsBlackholeTable::into_entries)
        .map_err(BlackholeUpdateError::Decode)?;
    let mut added = 0usize;
    for entry in &entries {
        let outcome = handle
            .blackhole_identity(personal_rns::routing::BlackholedIdentity {
                identity: entry.identity,
                source: entry.source,
                expiry: entry.expiry,
                reason: entry.reason.as_deref(),
            })
            .await
            .map_err(|source| BlackholeUpdateError::Apply {
                identity: entry.identity,
                source,
            })?;
        if outcome == BlackholeIdentityOutcome::Added {
            added = added.saturating_add(1);
        }
    }
    if added != 0 {
        files
            .store_source(source, entries)
            .map_err(BlackholeUpdateError::Persist)?;
        tracing::debug!(
            event = "blackhole_update_applied",
            source = ?source.as_bytes(),
            added,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_destination_matches_the_stock_name() {
        let source = IdentityHash::new([0x42; 16]);
        assert_eq!(
            destination_for(source),
            Ok(DestinationHash::new([
                0x4e, 0x85, 0x66, 0xf8, 0x2e, 0xaa, 0xcf, 0xc3, 0x69, 0xc4, 0x37, 0x8c, 0x31, 0xec,
                0x7f, 0x1e,
            ]))
        );
    }

    #[test]
    fn source_schedule_uses_the_stock_strict_interval_boundary() {
        let last = tokio::time::Instant::now();
        let interval = Duration::from_secs(120);
        assert!(update_is_due(last, None, interval));
        assert!(!update_is_due(last + interval, Some(last), interval));
        assert!(update_is_due(
            last + interval + Duration::from_nanos(1),
            Some(last),
            interval
        ));
    }
}
