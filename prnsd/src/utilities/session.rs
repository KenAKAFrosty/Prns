use personal_rns::runtime::NoPersistence;
use std::fmt;
use std::future::Future;
use std::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PathFound, RatchetPolicy,
};
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::node_introspection::{
    DestinationIdentityQuery, DestinationIdentitySnapshot, NodeIntrospection,
};
use personal_rns::request_endpoints;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    AnnounceNowError, ConfigurePreconfiguredDestinationError, ManuallyAttached, NodeRunError,
    NonRoutingIdentityError, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, RequestPathError, ServeMyRequestEndpoints,
};
use personal_rns::shared_instance::{
    connect_existing_shared_instance, ExistingSharedInstanceUnavailable, SharedInstanceRpcClient,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::units::HopCount;
use personal_rns::wire::DestinationHash;

use super::configuration::{LoadedConfiguration, UtilityConfigurationError};

type UtilityNode = PrnsNode<(), (), fn(PrnsEvent<'_>, &()), GrowableHeap>;

pub enum UtilityNodeIdentity {
    Anonymous,
    Private(IdentitySecretKey),
}

pub struct UtilityNodeSession {
    node: UtilityNode,
    client: UtilityNodeClient,
}

pub struct UtilityNodeClient {
    handle: PrnsNodeHandle,
    rpc: SharedInstanceRpcClient,
}

pub struct UtilityBusSession {
    node: UtilityNode,
    client: UtilityBusClient,
}

pub struct UtilityBusClient {
    handle: PrnsNodeHandle,
}

impl UtilityNodeSession {
    pub async fn connect(
        configuration: &LoadedConfiguration,
        identity: UtilityNodeIdentity,
        rpc_timeout: Duration,
    ) -> Result<Self, UtilityNodeSessionError> {
        let node = utility_node();
        let node = match identity {
            UtilityNodeIdentity::Anonymous => node,
            UtilityNodeIdentity::Private(identity) => node
                .with_non_routing_identity(identity)
                .map_err(UtilityNodeSessionError::IdentityConfiguration)?,
        };
        Self::attach(configuration, node, rpc_timeout).await
    }

    async fn attach(
        configuration: &LoadedConfiguration,
        node: UtilityNode,
        rpc_timeout: Duration,
    ) -> Result<Self, UtilityNodeSessionError> {
        let rpc = configuration
            .local_rpc_client(rpc_timeout)
            .map_err(UtilityNodeSessionError::Configuration)?;
        let bus = configuration
            .local_bus_client_intent()
            .map_err(UtilityNodeSessionError::Configuration)?;
        let handle = node.handle();
        connect_existing_shared_instance(&handle, bus)
            .await
            .map_err(UtilityNodeSessionError::SharedInstanceUnavailable)?;
        Ok(Self {
            node,
            client: UtilityNodeClient { handle, rpc },
        })
    }

    pub async fn run<T, F, Operation>(self, operation: F) -> Result<T, UtilityNodeStopped>
    where
        F: FnOnce(UtilityNodeClient) -> Operation,
        Operation: Future<Output = T>,
    {
        let Self { node, client } = self;
        tokio::select! {
            result = operation(client) => Ok(result),
            result = node.run() => Err(UtilityNodeStopped { failure: result.err() }),
        }
    }
}

impl UtilityNodeClient {
    pub fn handle(&self) -> &PrnsNodeHandle {
        &self.handle
    }

    pub fn rpc(&self) -> &SharedInstanceRpcClient {
        &self.rpc
    }

    pub async fn ensure_path(
        &self,
        destination: DestinationHash,
        timeout: Duration,
    ) -> Result<PathFound, UtilityPathError> {
        if let Some(route) = self.handle.route(destination).await {
            return Ok(PathFound {
                hops: HopCount(route.hops),
            });
        }
        tokio::time::timeout(timeout, self.handle.request_path(destination))
            .await
            .map_err(|_| UtilityPathError::Timeout { timeout })?
            .map_err(UtilityPathError::Request)
    }
}

impl UtilityBusSession {
    pub async fn connect(
        configuration: &LoadedConfiguration,
        identity: UtilityNodeIdentity,
    ) -> Result<Self, UtilityNodeSessionError> {
        let node = utility_node();
        let node = match identity {
            UtilityNodeIdentity::Anonymous => node,
            UtilityNodeIdentity::Private(identity) => node
                .with_non_routing_identity(identity)
                .map_err(UtilityNodeSessionError::IdentityConfiguration)?,
        };
        Self::attach(configuration, node).await
    }

    pub async fn connect_announcing(
        configuration: &LoadedConfiguration,
        identity: IdentitySecretKey,
        app_name: &str,
        aspects: &[&str],
    ) -> Result<(Self, DestinationHash), UtilityNodeSessionError> {
        let mut node = utility_node();
        let destination = node
            .register_preconfigured_destination(PreConfiguredDestination::Single {
                app_name,
                aspects,
                identity,
                announce_app_data: &[],
                proof: ProofStrategy::ProveNone,
                link_requests: LinkRequestPolicy::AcceptNone,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: Default::default(),
                request_endpoints: ServeMyRequestEndpoints::No,
            })
            .map_err(UtilityNodeSessionError::DestinationConfiguration)?;
        let session = Self::attach(configuration, node).await?;
        Ok((session, destination))
    }

    async fn attach(
        configuration: &LoadedConfiguration,
        node: UtilityNode,
    ) -> Result<Self, UtilityNodeSessionError> {
        let bus = configuration
            .local_bus_client_intent()
            .map_err(UtilityNodeSessionError::Configuration)?;
        let handle = node.handle();
        connect_existing_shared_instance(&handle, bus)
            .await
            .map_err(UtilityNodeSessionError::SharedInstanceUnavailable)?;
        Ok(Self {
            node,
            client: UtilityBusClient { handle },
        })
    }

    pub async fn run<T, F, Operation>(self, operation: F) -> Result<T, UtilityNodeStopped>
    where
        F: FnOnce(UtilityBusClient) -> Operation,
        Operation: Future<Output = T>,
    {
        let Self { node, client } = self;
        tokio::select! {
            result = operation(client) => Ok(result),
            result = node.run() => Err(UtilityNodeStopped { failure: result.err() }),
        }
    }
}

impl UtilityBusClient {
    pub fn handle(&self) -> &PrnsNodeHandle {
        &self.handle
    }

    pub async fn destination_identity(
        &self,
        query: DestinationIdentityQuery,
    ) -> Option<DestinationIdentitySnapshot> {
        self.handle.destination_identity(query).await
    }

    pub async fn announce(&self, destination: DestinationHash) -> Result<(), AnnounceNowError> {
        self.handle
            .announce_now(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })
            .await
    }
}

fn utility_node() -> UtilityNode {
    PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: std::iter::empty(),
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: ignore_event,
    })
}

fn ignore_event(_event: PrnsEvent<'_>, _state: &()) {}

#[derive(Debug)]
pub struct UtilityNodeStopped {
    failure: Option<NodeRunError>,
}

impl fmt::Display for UtilityNodeStopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.failure {
            Some(failure) => write!(
                formatter,
                "the utility node stopped before its operation completed: {failure}"
            ),
            None => formatter.write_str("the utility node stopped before its operation completed"),
        }
    }
}

impl std::error::Error for UtilityNodeStopped {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.failure
            .as_ref()
            .map(|failure| failure as &(dyn std::error::Error + 'static))
    }
}

#[derive(Debug)]
pub enum UtilityNodeSessionError {
    Configuration(UtilityConfigurationError),
    IdentityConfiguration(NonRoutingIdentityError),
    SharedInstanceUnavailable(ExistingSharedInstanceUnavailable),
    DestinationConfiguration(ConfigurePreconfiguredDestinationError),
}

impl fmt::Display for UtilityNodeSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(source) => source.fmt(formatter),
            Self::IdentityConfiguration(source) => {
                write!(
                    formatter,
                    "could not activate the utility identity: {source:?}"
                )
            }
            Self::SharedInstanceUnavailable(source) => write!(
                formatter,
                "{source}; start prnsd or a stock RNS shared instance first"
            ),
            Self::DestinationConfiguration(source) => {
                write!(
                    formatter,
                    "could not configure announce destination: {source:?}"
                )
            }
        }
    }
}

impl std::error::Error for UtilityNodeSessionError {}

#[derive(Debug)]
pub enum UtilityPathError {
    Request(RequestPathError),
    Timeout { timeout: Duration },
}

impl fmt::Display for UtilityPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(source) => write!(formatter, "path request failed: {source:?}"),
            Self::Timeout { timeout } => write!(
                formatter,
                "path request timed out after {:.3} seconds",
                timeout.as_secs_f64()
            ),
        }
    }
}

impl std::error::Error for UtilityPathError {}
