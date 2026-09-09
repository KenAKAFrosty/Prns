use core::future::Future;
use core::time::Duration;

use personal_rns::engine::{
    EstablishLinkFailure, EstablishLinkRejection, SendRequestFailure, SendRequestRejection,
    WriteEstablishLinkRejection,
};
use personal_rns::identity::IdentityHash;
use personal_rns::interfaces::InterfaceId;
use personal_rns::node_introspection::{InterfaceTimingSnapshot, NodeIntrospection};
use personal_rns::prelude::{
    ConnectRemoteControlTargetError, PrnsNodeHandle, RemoteControlError,
    RemoteControlTargetAccessControl, RemoteControlTargetHandle, RemoteControlTargetOperationError,
    SendError,
};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::RequestPathError;
use personal_rns::wire::DestinationHash;

use crate::contract::{
    DescribeRemoteControlTargetInput, DevelopmentNodeOperation, DevelopmentNodeOperationKind,
    RemoteControlAnnounceFailureStage as AnnounceStage,
    RemoteControlAnnounceStatus as AnnounceStatus,
    RemoteControlAnnounceUnknownReason as UnknownReason, RemoteControlDescribeFailureStage,
    RemoteControlDescribeOutcome, RemoteControlTargetSnapshot,
};
use crate::pairing::request_kinds;
use crate::snapshot::SnapshotStore;

pub async fn refresh_targets(
    handle: &PrnsNodeHandle,
    snapshots: &SnapshotStore,
) -> Result<Vec<RemoteControlTargetSnapshot>, String> {
    let inventory = handle
        .remote_control_target_inventory()
        .await
        .map_err(|_| "The upstream target inventory is unavailable.".to_owned())?;
    let mut targets = Vec::with_capacity(inventory.len());
    for authorized in inventory.targets() {
        let resolved = handle
            .resolve_remote_control_target(authorized.identity_hash())
            .await
            .map_err(|_| "A persisted target could not be resolved.".to_owned())?;
        targets.push(target_snapshot(&resolved));
    }
    snapshots.update(|snapshot| snapshot.paired_targets.clone_from(&targets));
    Ok(targets)
}

pub async fn describe(
    handle: &PrnsNodeHandle,
    snapshots: &SnapshotStore,
    input: DescribeRemoteControlTargetInput,
) -> RemoteControlDescribeOutcome {
    let Some(target_identity) = identity_hash(&input.target_identity_fingerprint) else {
        return failed(
            RemoteControlDescribeFailureStage::Input,
            "A target identity fingerprint must contain exactly 16 bytes.",
        );
    };
    snapshots.update(|snapshot| {
        snapshot.active_operation = Some(DevelopmentNodeOperation {
            kind: DevelopmentNodeOperationKind::Describe,
            started_at_millis: monotonic_millis(),
        });
    });

    let resolved = match handle.resolve_remote_control_target(target_identity).await {
        Ok(resolved) => resolved,
        Err(_) => {
            clear_operation(snapshots);
            return failed(
                RemoteControlDescribeFailureStage::Inventory,
                "The selected target is not present in the upstream authorization inventory.",
            );
        }
    };
    let target = target_snapshot(&resolved);
    let connected = match connect_reachable_target(
        handle,
        target_identity,
        resolved.endpoint().destination_hash(),
    )
    .await
    {
        Ok(connected) => connected,
        Err(error) => {
            return fail_describe_reachable_connect(snapshots, error);
        }
    };
    let _link = CloseTargetLink {
        handle,
        id: connected.connection().link_id(),
    };
    let result = connected.describe().await;
    match result {
        Ok((description, rtt)) => {
            let available_requests = request_kinds(description.available_requests());
            let _ = refresh_targets(handle, snapshots).await;
            clear_operation(snapshots);
            RemoteControlDescribeOutcome::Described {
                target,
                available_requests,
                rtt_millis: rtt.millis(),
                snapshot: Box::new(snapshots.read()),
            }
        }
        Err(error) => {
            clear_operation(snapshots);
            failed_operation(error)
        }
    }
}

/// Execute once through the authorized public target handle. An absent response
/// after submission cannot prove that the target did not announce.
pub async fn announce_self(
    handle: &PrnsNodeHandle,
    target_identity_fingerprint: &[u8],
) -> AnnounceStatus {
    let Some(identity) = identity_hash(target_identity_fingerprint) else {
        return AnnounceStatus::Failed {
            stage: AnnounceStage::Input,
        };
    };
    let resolved = match handle.resolve_remote_control_target(identity).await {
        Ok(resolved) => resolved,
        Err(_) => {
            return AnnounceStatus::Failed {
                stage: AnnounceStage::Inventory,
            };
        }
    };
    let connected =
        match connect_reachable_target(handle, identity, resolved.endpoint().destination_hash())
            .await
        {
            Ok(connected) => connected,
            Err(error) => return announce_reachable_connect_failure(error),
        };
    let _link = CloseTargetLink {
        handle,
        id: connected.connection().link_id(),
    };
    // Recheck the live target's capabilities: persisted permissions alone do not
    // establish that the target still supports or authorizes this operation.
    let status = match connected.describe().await {
        Ok((description, _))
            if description
                .available_requests()
                .supports(personal_rns::remote_control::RemoteControlRequestKind::AnnounceSelf) =>
        {
            match connected.announce_self().await {
                Ok(rtt) => AnnounceStatus::Announced {
                    rtt_millis: rtt.millis(),
                },
                Err(error) => announce_exchange_failure(error),
            }
        }
        Ok(_) => AnnounceStatus::Failed {
            stage: AnnounceStage::Permission,
        },
        Err(RemoteControlTargetOperationError::NotPermitted(_)) => AnnounceStatus::Failed {
            stage: AnnounceStage::Permission,
        },
        Err(_) => AnnounceStatus::Failed {
            stage: AnnounceStage::Request,
        },
    };
    status
}

// The public target handle requires explicit close. Own that obligation across
// cancellation as well as normal completion of an app-owned connected session.
// An upstream Link still being established/identified has not reached this
// scope; cancelling its waiter does not imply immediate engine cancellation.
struct CloseTargetLink<'a> {
    handle: &'a PrnsNodeHandle,
    id: LinkId,
}

impl Drop for CloseTargetLink<'_> {
    fn drop(&mut self) {
        let _closed = self.handle.close_link(self.id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetReadiness {
    RouteReady,
    DiscoveryReady,
    Waiting,
}

const TRANSPORT_READINESS_WAIT: Duration = Duration::from_secs(5);
const TRANSPORT_READINESS_POLL: Duration = Duration::from_millis(100);

fn target_readiness(
    route_interface: Option<InterfaceId>,
    interfaces: &[InterfaceTimingSnapshot],
) -> TargetReadiness {
    let mut discovery_ready = false;
    for interface in interfaces {
        if !interface.connection.is_online() || !interface.capabilities.allows_transmit() {
            continue;
        }
        if Some(interface.id) == route_interface {
            return TargetReadiness::RouteReady;
        }
        discovery_ready = true;
    }
    if discovery_ready {
        TargetReadiness::DiscoveryReady
    } else {
        TargetReadiness::Waiting
    }
}

async fn observe_target_readiness(
    handle: &PrnsNodeHandle,
    destination: DestinationHash,
) -> TargetReadiness {
    let route = handle.route(destination).await;
    // Current app transports (Bluetooth/TCP) publish status. Unlike the folded
    // UI inventory, this excludes discovery supervisors without a data lane.
    // It is not a universal predicate for custom interfaces without status.
    target_readiness(
        route.map(|route| route.interface),
        &handle.interface_timing_inventory(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReachabilityFailure {
    Route,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReachableTargetConnectionError<ConnectionError> {
    Reachability(ReachabilityFailure),
    Connection(ConnectionError),
}

async fn connect_reachable_target<'a>(
    handle: &'a PrnsNodeHandle,
    target: IdentityHash,
    destination: DestinationHash,
) -> Result<
    RemoteControlTargetHandle<'a>,
    ReachableTargetConnectionError<ConnectRemoteControlTargetError>,
> {
    // Stored routes can outlive their interfaces. Wait for live app transport
    // status before sending anything. Discovery remains one engine-owned path
    // request, not a retry of the application operation.
    connect_reachable_target_with(
        destination,
        |destination| observe_target_readiness(handle, destination),
        |destination| async move {
            handle
                .request_path(destination)
                .await
                .map(|_| ())
                .map_err(classify_request_path_error)
        },
        || async move { handle.connect_remote_control_target(target).await },
    )
    .await
}

async fn connect_reachable_target_with<
    Connection,
    ConnectionError,
    Observe,
    ObserveFuture,
    RequestPath,
    RequestPathFuture,
    Connect,
    ConnectFuture,
>(
    destination: DestinationHash,
    mut observe: Observe,
    request_path: RequestPath,
    connect: Connect,
) -> Result<Connection, ReachableTargetConnectionError<ConnectionError>>
where
    Observe: FnMut(DestinationHash) -> ObserveFuture,
    ObserveFuture: Future<Output = TargetReadiness>,
    RequestPath: FnOnce(DestinationHash) -> RequestPathFuture,
    RequestPathFuture: Future<Output = Result<(), ReachabilityFailure>>,
    Connect: FnOnce() -> ConnectFuture,
    ConnectFuture: Future<Output = Result<Connection, ConnectionError>>,
{
    // This is a bounded condition wait, not a delay after granting permission.
    // Describe's admission-time deadline/caller/Stop also owns and can drop this
    // entire future. An announcement has this finite preflight bound too.
    let ready = tokio::time::timeout(TRANSPORT_READINESS_WAIT, async {
        loop {
            let ready = observe(destination).await;
            if ready != TargetReadiness::Waiting {
                return ready;
            }
            tokio::time::sleep(TRANSPORT_READINESS_POLL).await;
        }
    })
    .await
    .map_err(|_| ReachableTargetConnectionError::Reachability(ReachabilityFailure::Route))?;
    if ready == TargetReadiness::DiscoveryReady {
        request_path(destination)
            .await
            .map_err(ReachableTargetConnectionError::Reachability)?;
        // A matching announcement can settle discovery just before its lane
        // departs. Recheck once; do not connect blindly or replay discovery.
        if observe(destination).await != TargetReadiness::RouteReady {
            return Err(ReachableTargetConnectionError::Reachability(
                ReachabilityFailure::Route,
            ));
        }
    }
    connect()
        .await
        .map_err(ReachableTargetConnectionError::Connection)
}

const fn classify_request_path_error(error: RequestPathError) -> ReachabilityFailure {
    match error {
        RequestPathError::NodeStopped => ReachabilityFailure::Node,
        RequestPathError::EntropyUnavailable | RequestPathError::Failed(_) => {
            ReachabilityFailure::Route
        }
    }
}

fn fail_describe_reachable_connect(
    snapshots: &SnapshotStore,
    error: ReachableTargetConnectionError<ConnectRemoteControlTargetError>,
) -> RemoteControlDescribeOutcome {
    clear_operation(snapshots);
    match error {
        ReachableTargetConnectionError::Reachability(ReachabilityFailure::Route) => failed(
            RemoteControlDescribeFailureStage::Route,
            "Prns could not discover a route to the selected target.",
        ),
        ReachableTargetConnectionError::Reachability(ReachabilityFailure::Node) => failed(
            RemoteControlDescribeFailureStage::Node,
            "The Prns node stopped while discovering the selected target.",
        ),
        ReachableTargetConnectionError::Connection(error) => failed_connect(error),
    }
}

fn announce_reachable_connect_failure(
    error: ReachableTargetConnectionError<ConnectRemoteControlTargetError>,
) -> AnnounceStatus {
    match error {
        ReachableTargetConnectionError::Reachability(ReachabilityFailure::Route) => {
            AnnounceStatus::Failed {
                stage: AnnounceStage::Route,
            }
        }
        ReachableTargetConnectionError::Reachability(ReachabilityFailure::Node) => {
            AnnounceStatus::Failed {
                stage: AnnounceStage::Node,
            }
        }
        ReachableTargetConnectionError::Connection(error) => announce_connect_failure(error),
    }
}

fn announce_connect_failure(error: ConnectRemoteControlTargetError) -> AnnounceStatus {
    let stage = match error {
        ConnectRemoteControlTargetError::Resolve(_) => AnnounceStage::Inventory,
        ConnectRemoteControlTargetError::EstablishLink(error) => {
            match classify_establish_link_failure(&error) {
                EstablishLinkFailureClass::Route => AnnounceStage::Route,
                EstablishLinkFailureClass::Link => AnnounceStage::Link,
                EstablishLinkFailureClass::Node => AnnounceStage::Node,
            }
        }
        ConnectRemoteControlTargetError::Identify(_) => AnnounceStage::Identification,
    };
    AnnounceStatus::Failed { stage }
}

fn announce_exchange_failure(error: RemoteControlTargetOperationError) -> AnnounceStatus {
    use personal_rns::runtime::RemoteControlAnnounceSelfFailure;
    match error {
        RemoteControlTargetOperationError::NotPermitted(_) => AnnounceStatus::Failed {
            stage: AnnounceStage::Permission,
        },
        RemoteControlTargetOperationError::Exchange(RemoteControlError::AnnounceSelf(failure)) => {
            match failure {
                RemoteControlAnnounceSelfFailure::Unavailable => AnnounceStatus::Unavailable,
                RemoteControlAnnounceSelfFailure::Rejected => AnnounceStatus::Rejected,
                RemoteControlAnnounceSelfFailure::WriteFailed => AnnounceStatus::WriteFailed,
            }
        }
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::Failed(failure),
        )) => match failure {
            SendRequestFailure::Rejected(_) => AnnounceStatus::Failed {
                stage: AnnounceStage::Link,
            },
            SendRequestFailure::Timeout => AnnounceStatus::OutcomeUnknown {
                reason: UnknownReason::Timeout,
            },
            SendRequestFailure::LinkClosed => AnnounceStatus::OutcomeUnknown {
                reason: UnknownReason::ConnectionLost,
            },
            SendRequestFailure::ResponseTooLarge
            | SendRequestFailure::ResponseTransferFailed(_)
            | SendRequestFailure::ResourceCapacity => AnnounceStatus::OutcomeUnknown {
                reason: UnknownReason::ResponseInvalid,
            },
            SendRequestFailure::WriteFailed | SendRequestFailure::Culled => {
                AnnounceStatus::OutcomeUnknown {
                    reason: UnknownReason::DeliveryUnconfirmed,
                }
            }
        },
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::NodeStopped,
        )) => AnnounceStatus::OutcomeUnknown {
            reason: UnknownReason::NodeStopped,
        },
        RemoteControlTargetOperationError::Exchange(
            RemoteControlError::Response(_) | RemoteControlError::UnexpectedResponse { .. },
        ) => AnnounceStatus::OutcomeUnknown {
            reason: UnknownReason::ResponseInvalid,
        },
        RemoteControlTargetOperationError::Exchange(
            RemoteControlError::Encode(_)
            | RemoteControlError::Remote(_)
            | RemoteControlError::Request(SendError::Busy | SendError::PayloadTooLarge),
        ) => AnnounceStatus::Failed {
            stage: AnnounceStage::Request,
        },
    }
}

pub fn complete_announcement(
    snapshots: &SnapshotStore,
    operation_id: &u64,
    status: AnnounceStatus,
) {
    snapshots.update(|snapshot| {
        if let Some(operation) = &mut snapshot.last_announcement {
            if &operation.operation_id == operation_id {
                operation.status = status;
                if snapshot.active_operation.as_ref().is_some_and(|operation| {
                    operation.kind == DevelopmentNodeOperationKind::AnnounceSelf
                }) {
                    snapshot.active_operation = None;
                }
            }
        }
    });
}

fn target_snapshot(
    resolved: &personal_rns::runtime::ResolvedRemoteControlTarget,
) -> RemoteControlTargetSnapshot {
    RemoteControlTargetSnapshot {
        target_identity_fingerprint: resolved.target().as_bytes().to_vec(),
        destination: resolved.endpoint().destination_hash().as_bytes().to_vec(),
        controller_identity_fingerprint: resolved.controller().identity_hash().as_bytes().to_vec(),
        permitted_requests: request_kinds(resolved.permitted_requests()),
    }
}

fn identity_hash(bytes: &[u8]) -> Option<IdentityHash> {
    let value = <[u8; 16]>::try_from(bytes).ok()?;
    Some(IdentityHash::new(value))
}

fn failed_connect(error: ConnectRemoteControlTargetError) -> RemoteControlDescribeOutcome {
    match error {
        ConnectRemoteControlTargetError::Resolve(_) => failed(
            RemoteControlDescribeFailureStage::Inventory,
            "The selected target authorization could not be resolved.",
        ),
        ConnectRemoteControlTargetError::EstablishLink(error) => {
            match classify_establish_link_failure(&error) {
                EstablishLinkFailureClass::Route => failed(
                    RemoteControlDescribeFailureStage::Route,
                    "Prns has no current route to the selected target.",
                ),
                EstablishLinkFailureClass::Link => failed(
                    RemoteControlDescribeFailureStage::Link,
                    "Prns could not establish a Link to the selected target.",
                ),
                EstablishLinkFailureClass::Node => failed(
                    RemoteControlDescribeFailureStage::Node,
                    "The Prns node stopped while opening the target Link.",
                ),
            }
        }
        ConnectRemoteControlTargetError::Identify(_) => failed(
            RemoteControlDescribeFailureStage::Identification,
            "Prns could not identify the controller on the target Link.",
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EstablishLinkFailureClass {
    Route,
    Link,
    Node,
}

pub(crate) fn classify_establish_link_failure(
    error: &SendError<EstablishLinkFailure>,
) -> EstablishLinkFailureClass {
    match error {
        SendError::Failed(EstablishLinkFailure::Rejected(
            EstablishLinkRejection::NoRouteToDestination
            | EstablishLinkRejection::NotDirectlyReachable,
        ))
        | SendError::Failed(EstablishLinkFailure::WriteFailed(
            WriteEstablishLinkRejection::RouteVanished,
        )) => EstablishLinkFailureClass::Route,
        SendError::NodeStopped => EstablishLinkFailureClass::Node,
        SendError::PayloadTooLarge
        | SendError::Busy
        | SendError::Failed(EstablishLinkFailure::WriteFailed(
            WriteEstablishLinkRejection::Serialize
            | WriteEstablishLinkRejection::LinkTableFull
            | WriteEstablishLinkRejection::DuplicateLinkId,
        ))
        | SendError::Failed(EstablishLinkFailure::Timeout) => EstablishLinkFailureClass::Link,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendRequestFailureClass {
    Timeout,
    Link,
    Request,
}

pub(crate) const fn classify_send_request_failure(
    failure: SendRequestFailure,
) -> SendRequestFailureClass {
    match failure {
        SendRequestFailure::Timeout => SendRequestFailureClass::Timeout,
        SendRequestFailure::Rejected(
            SendRequestRejection::NoSuchLink | SendRequestRejection::LinkNotActive,
        )
        | SendRequestFailure::LinkClosed
        | SendRequestFailure::ResponseTransferFailed(
            prns_core::routing::links::resources::ResourceFailureCause::LinkVanished,
        ) => SendRequestFailureClass::Link,
        SendRequestFailure::WriteFailed
        | SendRequestFailure::Culled
        | SendRequestFailure::ResponseTooLarge
        | SendRequestFailure::ResponseTransferFailed(_)
        | SendRequestFailure::ResourceCapacity => SendRequestFailureClass::Request,
    }
}

fn failed_operation(error: RemoteControlTargetOperationError) -> RemoteControlDescribeOutcome {
    match error {
        RemoteControlTargetOperationError::NotPermitted(_) => failed(
            RemoteControlDescribeFailureStage::Permission,
            "The persisted target grant does not permit Describe.",
        ),
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::Failed(failure),
        )) => match classify_send_request_failure(failure) {
            SendRequestFailureClass::Timeout => failed(
                RemoteControlDescribeFailureStage::Timeout,
                "The node did not respond before the request timed out.",
            ),
            SendRequestFailureClass::Link => failed(
                RemoteControlDescribeFailureStage::Link,
                "The connection to the node closed before the request completed.",
            ),
            SendRequestFailureClass::Request => failed(
                RemoteControlDescribeFailureStage::Request,
                "The node could not complete the Describe request.",
            ),
        },
        RemoteControlTargetOperationError::Exchange(RemoteControlError::Request(
            SendError::NodeStopped,
        )) => failed(
            RemoteControlDescribeFailureStage::Node,
            "The Prns node stopped during Describe.",
        ),
        RemoteControlTargetOperationError::Exchange(_) => failed(
            RemoteControlDescribeFailureStage::Request,
            "The upstream RemoteControl Describe exchange failed.",
        ),
    }
}

fn failed(stage: RemoteControlDescribeFailureStage, detail: &str) -> RemoteControlDescribeOutcome {
    RemoteControlDescribeOutcome::Failed {
        stage,
        detail: detail.to_owned(),
    }
}

fn clear_operation(snapshots: &SnapshotStore) {
    snapshots.update(|snapshot| snapshot.active_operation = None);
}

fn monotonic_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_the_actual_routes_live_transmitting_interface() {
        use personal_rns::interfaces::{
            BitrateBps, ConnectionState, EgressCapability, IngressCapability,
            InterfaceCapabilities, TransportCapability,
        };
        let id = InterfaceId::new([0x21; 8]);
        let other = InterfaceId::new([0x22; 8]);
        let mut interface = InterfaceTimingSnapshot {
            id,
            bitrate: BitrateBps::guess(1_000_000),
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::NoTransport),
            },
            connection: ConnectionState::Connected,
        };
        // A discovery supervisor alone has no descriptor-bearing data interface.
        assert_eq!(target_readiness(Some(id), &[]), TargetReadiness::Waiting);
        assert_eq!(target_readiness(None, &[]), TargetReadiness::Waiting);
        for state in [
            ConnectionState::Initializing,
            ConnectionState::Reconnecting,
            ConnectionState::Failed,
            ConnectionState::Disconnected,
            ConnectionState::Disabled,
            ConnectionState::Unknown,
        ] {
            interface.connection = state;
            assert_eq!(
                target_readiness(Some(id), &[interface]),
                TargetReadiness::Waiting
            );
        }
        for state in [ConnectionState::Connected, ConnectionState::Degraded] {
            interface.connection = state;
            assert_eq!(
                target_readiness(Some(id), &[interface]),
                TargetReadiness::RouteReady
            );
            assert_eq!(
                target_readiness(Some(other), &[interface]),
                TargetReadiness::DiscoveryReady
            );
            assert_eq!(
                target_readiness(None, &[interface]),
                TargetReadiness::DiscoveryReady
            );
        }
        interface.capabilities.egress = EgressCapability::Disabled;
        assert_eq!(
            target_readiness(Some(id), &[interface]),
            TargetReadiness::Waiting
        );
    }

    #[tokio::test(start_paused = true)]
    async fn readiness_wait_sends_nothing_until_the_route_returns() {
        let readiness = core::cell::Cell::new(TargetReadiness::Waiting);
        let paths = core::cell::Cell::new(0);
        let connections = core::cell::Cell::new(0);
        let mut pending = Box::pin(connect_reachable_target_with(
            DestinationHash::new([0x61; 16]),
            |_| core::future::ready(readiness.get()),
            |_| {
                paths.set(paths.get() + 1);
                core::future::ready(Ok(()))
            },
            || {
                connections.set(connections.get() + 1);
                core::future::ready(Ok::<_, ()>(7))
            },
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(250), &mut pending)
                .await
                .is_err()
        );
        assert_eq!((paths.get(), connections.get()), (0, 0));
        readiness.set(TargetReadiness::RouteReady);
        assert_eq!(pending.await, Ok(7));
        assert_eq!((paths.get(), connections.get()), (0, 1));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_readiness_wait_cannot_send_when_the_interface_returns() {
        let readiness = core::cell::Cell::new(TargetReadiness::Waiting);
        let paths = core::cell::Cell::new(0);
        let connections = core::cell::Cell::new(0);
        let mut pending = Box::pin(connect_reachable_target_with(
            DestinationHash::new([0x62; 16]),
            |_| core::future::ready(readiness.get()),
            |_| {
                paths.set(paths.get() + 1);
                core::future::ready(Ok(()))
            },
            || {
                connections.set(connections.get() + 1);
                core::future::ready(Ok::<_, ()>(7))
            },
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(250), &mut pending)
                .await
                .is_err()
        );
        drop(pending);
        readiness.set(TargetReadiness::RouteReady);
        tokio::time::advance(TRANSPORT_READINESS_WAIT).await;
        assert_eq!((paths.get(), connections.get()), (0, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_transport_has_a_finite_preflight_without_network_emission() {
        let paths = core::cell::Cell::new(0);
        let connections = core::cell::Cell::new(0);
        let started = tokio::time::Instant::now();
        let result = connect_reachable_target_with(
            DestinationHash::new([0x63; 16]),
            |_| core::future::ready(TargetReadiness::Waiting),
            |_| {
                paths.set(paths.get() + 1);
                core::future::ready(Ok(()))
            },
            || {
                connections.set(connections.get() + 1);
                core::future::ready(Ok::<_, ()>(7))
            },
        )
        .await;
        assert_eq!(
            result,
            Err(ReachableTargetConnectionError::Reachability(
                ReachabilityFailure::Route
            ))
        );
        assert_eq!(started.elapsed(), TRANSPORT_READINESS_WAIT);
        assert_eq!((paths.get(), connections.get()), (0, 0));
    }

    #[tokio::test]
    async fn discovery_settlement_does_not_use_a_route_whose_interface_has_departed() {
        let paths = core::cell::Cell::new(0);
        let connections = core::cell::Cell::new(0);
        let result = connect_reachable_target_with(
            DestinationHash::new([0x64; 16]),
            |_| core::future::ready(TargetReadiness::DiscoveryReady),
            |_| {
                paths.set(paths.get() + 1);
                core::future::ready(Ok(()))
            },
            || {
                connections.set(connections.get() + 1);
                core::future::ready(Ok::<_, ()>(7))
            },
        )
        .await;
        assert_eq!(
            result,
            Err(ReachableTargetConnectionError::Reachability(
                ReachabilityFailure::Route
            ))
        );
        assert_eq!((paths.get(), connections.get()), (1, 0));
    }

    #[test]
    fn announcement_settlement_preserves_remote_outcomes_and_ambiguous_delivery() {
        use personal_rns::runtime::RemoteControlAnnounceSelfFailure as Failure;
        for (failure, expected) in [
            (Failure::Unavailable, AnnounceStatus::Unavailable),
            (Failure::Rejected, AnnounceStatus::Rejected),
            (Failure::WriteFailed, AnnounceStatus::WriteFailed),
        ] {
            assert_eq!(
                announce_exchange_failure(RemoteControlTargetOperationError::Exchange(
                    RemoteControlError::AnnounceSelf(failure)
                )),
                expected
            );
        }
        assert_eq!(
            announce_exchange_failure(RemoteControlTargetOperationError::Exchange(
                RemoteControlError::Request(SendError::Failed(SendRequestFailure::Timeout))
            )),
            AnnounceStatus::OutcomeUnknown {
                reason: UnknownReason::Timeout
            }
        );
        assert_eq!(
            announce_exchange_failure(RemoteControlTargetOperationError::Exchange(
                RemoteControlError::Request(SendError::Failed(SendRequestFailure::LinkClosed))
            )),
            AnnounceStatus::OutcomeUnknown {
                reason: UnknownReason::ConnectionLost
            }
        );
        assert_eq!(
            announce_exchange_failure(RemoteControlTargetOperationError::Exchange(
                RemoteControlError::Request(SendError::Busy)
            )),
            AnnounceStatus::Failed {
                stage: AnnounceStage::Request
            }
        );
    }

    #[test]
    fn older_completion_cannot_replace_a_newer_native_operation() {
        let snapshots = SnapshotStore::new();
        snapshots.update(|snapshot| {
            snapshot.last_announcement = Some(crate::contract::RemoteControlAnnounceOperation {
                operation_id: 2,
                target_identity_fingerprint: vec![1; 16],
                status: AnnounceStatus::Pending,
            });
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::AnnounceSelf,
                started_at_millis: 0,
            });
        });
        complete_announcement(&snapshots, &1, AnnounceStatus::Announced { rtt_millis: 3 });
        assert_eq!(
            snapshots
                .read()
                .last_announcement
                .map(|operation| operation.status),
            Some(AnnounceStatus::Pending)
        );
        assert!(snapshots.read().active_operation.is_some());
        complete_announcement(&snapshots, &2, AnnounceStatus::Announced { rtt_millis: 4 });
        assert!(snapshots.read().active_operation.is_none());
    }

    #[test]
    fn identity_input_is_exactly_sized() {
        assert!(identity_hash(&[0; 15]).is_none());
        assert!(identity_hash(&[0; 16]).is_some());
        assert!(identity_hash(&[0; 17]).is_none());
    }

    #[test]
    fn establish_link_failures_distinguish_route_from_link_failures() {
        let route_failures = [
            SendError::Failed(EstablishLinkFailure::Rejected(
                EstablishLinkRejection::NoRouteToDestination,
            )),
            SendError::Failed(EstablishLinkFailure::Rejected(
                EstablishLinkRejection::NotDirectlyReachable,
            )),
            SendError::Failed(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::RouteVanished,
            )),
        ];
        for failure in route_failures {
            assert_eq!(
                classify_establish_link_failure(&failure),
                EstablishLinkFailureClass::Route
            );
            assert!(matches!(
                failed_connect(ConnectRemoteControlTargetError::EstablishLink(failure)),
                RemoteControlDescribeOutcome::Failed {
                    stage: RemoteControlDescribeFailureStage::Route,
                    ..
                }
            ));
        }

        let link_failures = [
            SendError::Failed(EstablishLinkFailure::Timeout),
            SendError::Failed(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::Serialize,
            )),
            SendError::Failed(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::LinkTableFull,
            )),
            SendError::Failed(EstablishLinkFailure::WriteFailed(
                WriteEstablishLinkRejection::DuplicateLinkId,
            )),
            SendError::Busy,
            SendError::PayloadTooLarge,
        ];
        for failure in link_failures {
            assert_eq!(
                classify_establish_link_failure(&failure),
                EstablishLinkFailureClass::Link
            );
            assert!(matches!(
                failed_connect(ConnectRemoteControlTargetError::EstablishLink(failure)),
                RemoteControlDescribeOutcome::Failed {
                    stage: RemoteControlDescribeFailureStage::Link,
                    ..
                }
            ));
        }

        let node_stopped = SendError::NodeStopped;
        assert_eq!(
            classify_establish_link_failure(&node_stopped),
            EstablishLinkFailureClass::Node
        );
        assert!(matches!(
            failed_connect(ConnectRemoteControlTargetError::EstablishLink(node_stopped)),
            RemoteControlDescribeOutcome::Failed {
                stage: RemoteControlDescribeFailureStage::Node,
                ..
            }
        ));
    }

    #[test]
    fn request_failures_distinguish_timeout_link_and_request_failures() {
        assert_eq!(
            classify_send_request_failure(SendRequestFailure::Timeout),
            SendRequestFailureClass::Timeout
        );
        for failure in [
            SendRequestFailure::Rejected(SendRequestRejection::NoSuchLink),
            SendRequestFailure::Rejected(SendRequestRejection::LinkNotActive),
            SendRequestFailure::LinkClosed,
            SendRequestFailure::ResponseTransferFailed(
                prns_core::routing::links::resources::ResourceFailureCause::LinkVanished,
            ),
        ] {
            assert_eq!(
                classify_send_request_failure(failure),
                SendRequestFailureClass::Link
            );
        }
        for failure in [
            SendRequestFailure::WriteFailed,
            SendRequestFailure::Culled,
            SendRequestFailure::ResponseTooLarge,
            SendRequestFailure::ResourceCapacity,
        ] {
            assert_eq!(
                classify_send_request_failure(failure),
                SendRequestFailureClass::Request
            );
        }
    }

    #[tokio::test]
    async fn cached_route_skips_discovery_before_connecting() {
        let destination = DestinationHash::new([0x42; 16]);
        let path_requests = core::cell::Cell::new(0_u8);
        let connections = core::cell::Cell::new(0_u8);

        let result = connect_reachable_target_with(
            destination,
            |observed| {
                assert_eq!(observed, destination);
                core::future::ready(TargetReadiness::RouteReady)
            },
            |observed| {
                assert_eq!(observed, destination);
                path_requests.set(path_requests.get() + 1);
                core::future::ready(Ok(()))
            },
            || {
                connections.set(connections.get() + 1);
                core::future::ready(Result::<_, ()>::Ok(7_u8))
            },
        )
        .await;

        assert_eq!(result, Ok(7));
        assert_eq!(path_requests.get(), 0);
        assert_eq!(connections.get(), 1);
    }

    #[tokio::test]
    async fn missing_route_requests_the_authorized_destination_before_connecting() {
        let destination = DestinationHash::new([0x43; 16]);
        let phase = core::cell::Cell::new(0_u8);
        let requested = core::cell::Cell::new(None);

        let result = connect_reachable_target_with(
            destination,
            |observed| {
                assert_eq!(observed, destination);
                core::future::ready(if phase.get() == 0 {
                    phase.set(1);
                    TargetReadiness::DiscoveryReady
                } else {
                    assert_eq!(phase.replace(3), 2);
                    TargetReadiness::RouteReady
                })
            },
            |observed| {
                assert_eq!(phase.replace(2), 1);
                requested.set(Some(observed));
                core::future::ready(Ok(()))
            },
            || {
                assert_eq!(phase.replace(4), 3);
                core::future::ready(Result::<_, ()>::Ok(8_u8))
            },
        )
        .await;

        assert_eq!(result, Ok(8));
        assert_eq!(requested.get(), Some(destination));
        assert_eq!(phase.get(), 4);
    }

    #[tokio::test]
    async fn failed_discovery_prevents_connection() {
        let destination = DestinationHash::new([0x44; 16]);
        let connections = core::cell::Cell::new(0_u8);

        let result = connect_reachable_target_with(
            destination,
            |_| core::future::ready(TargetReadiness::DiscoveryReady),
            |observed| {
                assert_eq!(observed, destination);
                core::future::ready(Err(ReachabilityFailure::Route))
            },
            || {
                connections.set(connections.get() + 1);
                core::future::ready(Result::<(), ()>::Ok(()))
            },
        )
        .await;

        assert_eq!(
            result,
            Err(ReachableTargetConnectionError::Reachability(
                ReachabilityFailure::Route
            ))
        );
        assert_eq!(connections.get(), 0);
    }

    #[tokio::test]
    async fn pending_discovery_holds_the_operation_and_node_failure_clears_it() {
        let destination = DestinationHash::new([0x45; 16]);
        let snapshots = SnapshotStore::new();
        snapshots.update(|snapshot| {
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Describe,
                started_at_millis: 1,
            });
        });
        let connections = core::cell::Cell::new(0_u8);
        let (settle, settled) = tokio::sync::oneshot::channel();
        let pending = connect_reachable_target_with(
            destination,
            |_| core::future::ready(TargetReadiness::DiscoveryReady),
            move |observed| async move {
                assert_eq!(observed, destination);
                settled.await.expect("the discovery test settles")
            },
            || {
                connections.set(connections.get() + 1);
                core::future::ready(Result::<u8, ConnectRemoteControlTargetError>::Ok(9))
            },
        );
        tokio::pin!(pending);

        tokio::select! {
            result = &mut pending => panic!("discovery settled before its engine result: {result:?}"),
            () = tokio::task::yield_now() => {}
        }
        assert_eq!(connections.get(), 0);
        assert!(matches!(
            snapshots.read().active_operation,
            Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Describe,
                ..
            })
        ));

        settle
            .send(Err(ReachabilityFailure::Node))
            .expect("the pending discovery receives its settlement");
        let error = pending.await.expect_err("node failure prevents connection");
        let outcome = fail_describe_reachable_connect(&snapshots, error);
        assert!(matches!(
            outcome,
            RemoteControlDescribeOutcome::Failed {
                stage: RemoteControlDescribeFailureStage::Node,
                ..
            }
        ));
        assert_eq!(connections.get(), 0);
        assert!(snapshots.read().active_operation.is_none());
    }

    #[test]
    fn request_path_failures_preserve_route_and_node_stages() {
        assert_eq!(
            classify_request_path_error(RequestPathError::NodeStopped),
            ReachabilityFailure::Node
        );
        assert_eq!(
            classify_request_path_error(RequestPathError::EntropyUnavailable),
            ReachabilityFailure::Route
        );
        assert_eq!(
            classify_request_path_error(RequestPathError::Failed(
                personal_rns::engine::RequestPathFailure::Timeout
            )),
            ReachabilityFailure::Route
        );
    }

    #[test]
    fn describe_discovery_failure_clears_the_active_operation() {
        let snapshots = SnapshotStore::new();
        snapshots.update(|snapshot| {
            snapshot.active_operation = Some(DevelopmentNodeOperation {
                kind: DevelopmentNodeOperationKind::Describe,
                started_at_millis: 1,
            });
        });

        let outcome = fail_describe_reachable_connect(
            &snapshots,
            ReachableTargetConnectionError::Reachability(ReachabilityFailure::Route),
        );

        assert!(matches!(
            outcome,
            RemoteControlDescribeOutcome::Failed {
                stage: RemoteControlDescribeFailureStage::Route,
                ..
            }
        ));
        assert!(snapshots.read().active_operation.is_none());
    }

    #[cfg(feature = "host-test")]
    mod host {
        use core::time::Duration;

        use personal_rns::engine::{
            EgressTarget, OpenRemoteControlPairing, OpenRemoteControlPairingFailure,
            OpenRemoteControlPairingRejection, RemoteControlPairingOpened,
            RemoteControlTargetPairingApproval,
        };
        use personal_rns::identity::vault::IdentitySecretKey;
        use personal_rns::prelude::*;
        use personal_rns::remote_control::{
            RemoteControlInitialControllerGrants, RemoteControlPairingAttemptTimeout,
            RemoteControlPairingExpiresAfter, RemoteControlPairingPermissions,
            RemoteControlPairingPublicAppDataBytes, RemoteControlSelfAnnouncement,
            RemoteControlService,
        };
        use personal_rns::runtime::{
            DropRouteOutcome, NodePersistence, RemoteControlPairingControlError, RoutingControl,
        };
        use personal_rns::units::DurationMillis;

        use super::*;
        use crate::contract::{DevelopmentNodeRuntime, RemoteControlRequestKind};

        const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
        const PAIRING_WINDOW: DurationMillis = DurationMillis(60_000);
        const PAIRING_ATTEMPT_TIMEOUT: DurationMillis = DurationMillis(30_000);

        struct PersistenceDirectories {
            _temporary: tempfile::TempDir,
            target: std::path::PathBuf,
            controller: std::path::PathBuf,
        }

        impl PersistenceDirectories {
            fn new() -> Self {
                let temporary = tempfile::tempdir().expect("temporary persistence directory");
                Self {
                    target: temporary.path().join("target"),
                    controller: temporary.path().join("controller"),
                    _temporary: temporary,
                }
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn restored_inventory_discovers_describes_and_announces_over_upstream_tcp() {
            let persistence = PersistenceDirectories::new();
            pair_once(&persistence).await;
            describe_after_restart(&persistence).await;
        }

        async fn pair_once(persistence: &PersistenceDirectories) {
            let target_identity_secrets = identity_secrets(0xA1, 0xA2);
            let controller_identity_secrets = identity_secrets(0xB1, 0xB2);

            let (target_confirmation_tx, mut target_confirmation_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let (target_persisted_tx, mut target_persisted_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let target = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: service(target_identity_secrets),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| match event {
                    PrnsEvent::Message(
                        Message::RemoteControlTargetPairingConfirmationRequired(pairing),
                    ) => {
                        let _ignored = target_confirmation_tx.send(pairing);
                    }
                    PrnsEvent::Message(
                        Message::RemoteControlTargetPairingAuthorizationPersisted { attempt_id },
                    ) => {
                        let _ignored = target_persisted_tx.send(attempt_id);
                    }
                    PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
                },
                interfaces: ManuallyAttached,
                persistence: NodePersistence::custom_dir(&persistence.target)
                    .expect("target persistence opens"),
            });
            let target_handle = target.handle();
            let server = TcpServer::bind("127.0.0.1:0")
                .await
                .expect("target TCP server binds");
            let server_address = server
                .local_addr()
                .expect("target TCP server has an address")
                .to_string();
            let _server = target_handle.supervise(server);

            let (availability_tx, mut availability_rx) = tokio::sync::mpsc::unbounded_channel();
            let (controller_confirmation_tx, mut controller_confirmation_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let (controller_persisted_tx, mut controller_persisted_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let controller = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: service(controller_identity_secrets),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| match event {
                    PrnsEvent::Message(Message::RemoteControlPairingAvailable(pairing)) => {
                        let _ignored =
                            availability_tx.send((pairing.endpoint(), pairing.expires_at()));
                    }
                    PrnsEvent::Message(
                        Message::RemoteControlControllerPairingConfirmationRequired(pairing),
                    ) => {
                        let _ignored = controller_confirmation_tx.send(pairing);
                    }
                    PrnsEvent::Message(
                        Message::RemoteControlControllerPairingAuthorizationPersisted {
                            attempt_id,
                        },
                    ) => {
                        let _ignored = controller_persisted_tx.send(attempt_id);
                    }
                    PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
                },
                interfaces: move |node: &PrnsNodeHandle| {
                    node.attach(TcpClientInterface::new(server_address));
                },
                persistence: NodePersistence::custom_dir(&persistence.controller)
                    .expect("controller persistence opens"),
            });
            let controller_handle = controller.handle();

            let exchange = async {
                wait_for_direct_connection(&target_handle, &controller_handle).await;
                let opened = open_pairing_when_attached(
                    &target_handle,
                    OpenRemoteControlPairing {
                        target: EgressTarget::AllInterfaces,
                        expires_after: RemoteControlPairingExpiresAfter::try_from(PAIRING_WINDOW)
                            .expect("pairing window is valid"),
                        attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(
                            PAIRING_ATTEMPT_TIMEOUT,
                        )
                        .expect("attempt timeout is valid"),
                        permissions: RemoteControlPairingPermissions::try_from(
                            RemoteControlRequestSet::all(),
                        )
                        .expect("permissions are not empty"),
                        public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(
                            b"native composition host test".as_slice(),
                        )
                        .expect("public application data fits"),
                    },
                )
                .await;
                let (endpoint, expires_at) = availability_rx
                    .recv()
                    .await
                    .expect("controller observes pairing availability");
                let _offered = controller_handle
                    .initiate_remote_control_controller_pairing(
                        InitiateRemoteControlControllerPairing {
                            endpoint,
                            invitation_code: opened.invitation_code,
                            expires_at,
                        },
                    )
                    .await
                    .expect("controller receives a pairing offer");
                let target_confirmation = target_confirmation_rx
                    .recv()
                    .await
                    .expect("target asks for confirmation");
                let controller_confirmation = controller_confirmation_rx
                    .recv()
                    .await
                    .expect("controller asks for confirmation");
                let attempt_id = controller_confirmation.confirmation().attempt_id();
                assert_eq!(
                    target_handle
                        .approve_remote_control_target_pairing(target_confirmation.approval())
                        .await,
                    Ok(RemoteControlTargetPairingApproval::AwaitingControllerCommit { attempt_id }),
                );
                controller_handle
                    .approve_remote_control_controller_pairing(controller_confirmation.approval())
                    .await
                    .expect("controller completes pairing");
                assert_eq!(
                    target_persisted_rx
                        .recv()
                        .await
                        .expect("target persists its controller grant"),
                    attempt_id,
                );
                assert_eq!(
                    controller_persisted_rx
                        .recv()
                        .await
                        .expect("controller persists its target access"),
                    attempt_id,
                );
            };

            tokio::select! {
                result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
                    result.expect("pairing completes within the bounded window");
                }
                result = target.run() => panic!("target stopped while pairing: {result:?}"),
                result = controller.run() => panic!("controller stopped while pairing: {result:?}"),
            }
        }

        async fn describe_after_restart(persistence: &PersistenceDirectories) {
            let target_identity_secrets = identity_secrets(0xA1, 0xA2);
            let target_identity_hash = target_identity_secrets
                .identities()
                .target()
                .identity_hash();
            let target_endpoint = target_identity_secrets.identities().target().endpoint();
            let controller_identity_secrets = identity_secrets(0xB1, 0xB2);

            let (target_restored_tx, mut target_restored_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let self_destination = announcement_destination();
            let self_destination_hash = self_destination
                .destination_hash()
                .expect("announcement destination is valid");
            let target = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: service(target_identity_secrets),
                pre_configured_destinations: [self_destination],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| {
                    if matches!(
                        event,
                        PrnsEvent::Diagnostic(Diagnostic::PersistenceRestored { .. })
                    ) {
                        let _ignored = target_restored_tx.send(());
                    }
                },
                interfaces: ManuallyAttached,
                persistence: NodePersistence::custom_dir(&persistence.target)
                    .expect("restarted target persistence opens"),
            });
            let target_handle = target.handle();
            let server = TcpServer::bind("127.0.0.1:0")
                .await
                .expect("restarted target TCP server binds");
            let server_address = server
                .local_addr()
                .expect("restarted target TCP server has an address")
                .to_string();
            let _server = target_handle.supervise(server);

            let (target_announce_tx, mut target_announce_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let (controller_restored_tx, mut controller_restored_rx) =
                tokio::sync::mpsc::unbounded_channel();
            let controller = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: service(controller_identity_secrets),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| match event {
                    PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                        let _ignored = target_announce_tx.send(destination);
                    }
                    PrnsEvent::Diagnostic(Diagnostic::PersistenceRestored { .. }) => {
                        let _ignored = controller_restored_tx.send(());
                    }
                    PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
                },
                interfaces: move |node: &PrnsNodeHandle| {
                    node.attach(TcpClientInterface::new(server_address));
                },
                persistence: NodePersistence::custom_dir(&persistence.controller)
                    .expect("restarted controller persistence opens"),
            });
            let controller_handle = controller.handle();
            let snapshots = SnapshotStore::new();
            snapshots.set_runtime(DevelopmentNodeRuntime::Running);

            let exchange = async {
                target_restored_rx
                    .recv()
                    .await
                    .expect("restarted target restores its controller grant");
                controller_restored_rx
                    .recv()
                    .await
                    .expect("restarted controller restores its target access");
                wait_for_direct_connection(&target_handle, &controller_handle).await;
                assert!(
                    controller_handle
                        .route(target_endpoint.destination_hash())
                        .await
                        .is_none(),
                    "the restarted controller begins without a cached target route"
                );

                let targets = refresh_targets(&controller_handle, &snapshots)
                    .await
                    .expect("application projection restores the upstream inventory");
                assert_eq!(targets.len(), 1);
                assert_eq!(
                    targets[0].target_identity_fingerprint,
                    target_identity_hash.as_bytes(),
                );
                assert_eq!(
                    targets[0].destination,
                    target_endpoint.destination_hash().as_bytes(),
                );

                let outcome = describe(
                    &controller_handle,
                    &snapshots,
                    DescribeRemoteControlTargetInput {
                        target_identity_fingerprint: target_identity_hash.as_bytes().to_vec(),
                    },
                )
                .await;
                let RemoteControlDescribeOutcome::Described {
                    target,
                    available_requests,
                    snapshot,
                    ..
                } = outcome
                else {
                    panic!("application Describe failed after restart: {outcome:?}");
                };
                assert_eq!(
                    target_announce_rx
                        .recv()
                        .await
                        .expect("the route request returns the stable target announcement"),
                    target_endpoint.destination_hash(),
                );
                assert_eq!(
                    target.target_identity_fingerprint,
                    target_identity_hash.as_bytes(),
                );
                assert_eq!(
                    available_requests,
                    vec![
                        RemoteControlRequestKind::Describe,
                        RemoteControlRequestKind::AnnounceSelf
                    ]
                );
                assert_eq!(snapshot.paired_targets, targets);
                assert!(snapshot.active_operation.is_none());

                assert_eq!(
                    controller_handle
                        .drop_route(target_endpoint.destination_hash())
                        .await,
                    Ok(DropRouteOutcome::Dropped),
                );
                assert!(controller_handle
                    .route(target_endpoint.destination_hash())
                    .await
                    .is_none());
                let announced =
                    announce_self(&controller_handle, target_identity_hash.as_bytes()).await;
                assert!(matches!(announced, AnnounceStatus::Announced { .. }));
                assert_eq!(
                    target_announce_rx
                        .recv()
                        .await
                        .expect("AnnounceSelf reacquires the stable target route"),
                    target_endpoint.destination_hash(),
                );
                assert_eq!(
                    target_announce_rx
                        .recv()
                        .await
                        .expect("controller observes the requested announcement"),
                    self_destination_hash
                );
            };

            tokio::select! {
                result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
                    result.expect("restored Describe completes within the bounded window");
                }
                result = target.run() => panic!("restarted target stopped: {result:?}"),
                result = controller.run() => panic!("restarted controller stopped: {result:?}"),
            }
        }

        fn identity_secrets(
            controller_fill: u8,
            target_fill: u8,
        ) -> RemoteControlNodeIdentitySecrets {
            RemoteControlNodeIdentitySecrets::new(
                RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
                    [controller_fill; IDENTITY_SECRET_KEY_LEN],
                )),
                RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
                    [target_fill; IDENTITY_SECRET_KEY_LEN],
                )),
            )
            .expect("controller and target identities are distinct")
        }

        fn service(
            identity_secrets: RemoteControlNodeIdentitySecrets,
        ) -> RemoteControlService<'static> {
            let destination = announcement_destination()
                .destination_hash()
                .expect("announcement destination is valid");
            RemoteControlService::new(
                identity_secrets,
                RemoteControlInitialControllerGrants::Nobody,
                RemoteControlSelfAnnouncement::Destination(destination),
            )
        }

        fn announcement_destination() -> PreConfiguredDestination<'static> {
            PreConfiguredDestination::Single {
                app_name: "prns-app-test",
                aspects: &["node"],
                identity: Zeroizing::new([0xC4; IDENTITY_SECRET_KEY_LEN]),
                announce_app_data: b"Announcement fixture",
                proof: ProofStrategy::ProveNone,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: Default::default(),
                request_endpoints: ServeMyRequestEndpoints::No,
            }
        }

        async fn wait_for_direct_connection(target: &PrnsNodeHandle, controller: &PrnsNodeHandle) {
            loop {
                let target_connected = target
                    .interfaces()
                    .iter()
                    .any(|interface| interface.connection.is_online());
                let controller_connected = controller
                    .interfaces()
                    .iter()
                    .any(|interface| interface.connection.is_online());
                if target_connected && controller_connected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        }

        async fn open_pairing_when_attached(
            target: &PrnsNodeHandle,
            open: OpenRemoteControlPairing,
        ) -> RemoteControlPairingOpened {
            loop {
                match target.open_remote_control_pairing(open.clone()).await {
                    Ok(opened) => return opened,
                    Err(RemoteControlPairingControlError::Failed(
                        OpenRemoteControlPairingFailure::Rejected(
                            OpenRemoteControlPairingRejection::NoTransmittingInterfaces,
                        ),
                    )) => tokio::task::yield_now().await,
                    Err(error) => panic!("target could not open pairing: {error:?}"),
                }
            }
        }
    }
}
