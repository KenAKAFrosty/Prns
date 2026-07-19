use std::fmt;
use std::future::Future;
use std::time::Duration;

use personal_rns::engine::PathFound;
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::routes;
use personal_rns::runtime::{
    Manual, NonRoutingIdentityError, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
    RequestPathError,
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

impl UtilityNodeSession {
    pub async fn connect(
        configuration: &LoadedConfiguration,
        identity: UtilityNodeIdentity,
        rpc_timeout: Duration,
    ) -> Result<Self, UtilityNodeSessionError> {
        let rpc = configuration
            .local_rpc_client(rpc_timeout)
            .map_err(UtilityNodeSessionError::Configuration)?;
        let bus = configuration
            .local_bus_client_intent()
            .map_err(UtilityNodeSessionError::Configuration)?;
        let node: UtilityNode = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: std::iter::empty(),
            app_state: (),
            storage: GrowableHeap,
            routes: routes![],
            interfaces: Manual,
            on_event: ignore_event,
        });
        let node = match identity {
            UtilityNodeIdentity::Anonymous => node,
            UtilityNodeIdentity::Private(identity) => node
                .with_non_routing_identity(identity)
                .map_err(UtilityNodeSessionError::IdentityConfiguration)?,
        };
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
            () = node.run() => Err(UtilityNodeStopped),
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

fn ignore_event(_event: PrnsEvent<'_>, _state: &()) {}

#[derive(Debug)]
pub struct UtilityNodeStopped;

impl fmt::Display for UtilityNodeStopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the utility node stopped before its operation completed")
    }
}

impl std::error::Error for UtilityNodeStopped {}

#[derive(Debug)]
pub enum UtilityNodeSessionError {
    Configuration(UtilityConfigurationError),
    IdentityConfiguration(NonRoutingIdentityError),
    SharedInstanceUnavailable(ExistingSharedInstanceUnavailable),
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
