use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Weak};

use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use crate::engine::{
    InstantMillis, RespondFailure, RespondRejection, SendResourceFailure, SendResourceRejection,
};
use crate::identity::IdentityHash;
use crate::remote_control::RemoteControlControllerGrantTable;
use crate::remote_control::{
    RemoteControlProtocolError, RemoteControlRequestKind, RemoteControlResponse,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use prns_runtime::runtime::placement::{
    admit_remote_control_request, dispatch_verified_admitted_remote_control_request,
    verify_admitted_remote_control_request, VerifiedAdmittedRemoteControlRequest,
};

use super::interface_watch::{
    InterfaceWatchRegistry, WatchAdmission, WatchReservation, WatchReserveFailure,
};
use super::node_facade::{
    PrnsNodeHandle, RemoteControlAuthorizationPersistence, ResponseSendError,
};
use super::remote_control_controller_grants::RemoteControlControllerGrantReceiver;
use super::remote_control_pairing_persistence::{
    RemoteControlAuthorizationPersistenceFailure, RemoteControlPairingPersistenceReceiver,
};
use super::remote_control_target_accesses::RemoteControlTargetAccessReceiver;
use super::request_endpoints::{
    dispatch_request, Decline, InboundRequest, RequestEndpointPolicy, RequestEndpointSet,
};
use super::request_endpoints::{ResponseCapacityExceeded, ResponseSink};
use super::AssembledRemoteControl;
use crate::routing::links::channel::byte_stream::StreamId;

pub(super) const REQUEST_QUEUE_DEPTH: usize = 1024;
const MAX_IN_FLIGHT: usize = 256;

pub(super) struct RunnerRequest {
    pub destination: DestinationHash,
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub requester: Option<IdentityHash>,
    pub path_hash: RequestPathHash,
    pub requested_at: InstantMillis,
    pub rtt: RttMillis,
    pub data: std::vec::Vec<u8>,
}

pub(super) struct RemoteControlAuthorizationRuntime<'a> {
    pub controller_grants: &'a mut RemoteControlControllerGrantReceiver,
    pub target_accesses: &'a mut RemoteControlTargetAccessReceiver,
    pub pairing_persistence: &'a mut RemoteControlPairingPersistenceReceiver,
    pub persistence: Option<&'a RemoteControlAuthorizationPersistence>,
}

impl RunnerRequest {
    fn inbound(&self) -> InboundRequest<'_> {
        InboundRequest::new(
            self.destination,
            self.link_id,
            self.request_id,
            self.requester,
            self.requested_at,
            self.rtt,
            &self.data,
        )
    }
}

enum PreparedRequestRoute {
    Application,
    RemoteControl(Box<VerifiedAdmittedRemoteControlRequest>),
    InterfaceWatch {
        stream_id: StreamId,
        reservation: Result<WatchReservation, WatchReserveFailure>,
    },
    Declined(Decline),
}

struct PreparedRunnerRequest {
    request: RunnerRequest,
    route: PreparedRequestRoute,
}

impl PreparedRunnerRequest {
    /// Reserve synchronously with admission, before a response-lane wait can yield.
    /// Grant revocation and link closure can then cancel pending and active watches alike.
    fn reserve_interface_watch(
        mut self,
        commands: &PrnsNodeHandle,
        watches: &InterfaceWatchRegistry,
    ) -> Self {
        if let PreparedRequestRoute::RemoteControl(verified) = &self.route {
            if let Some(stream_id) = verified.watch_interfaces_stream() {
                self.route = match self.request.requester {
                    Some(controller) => PreparedRequestRoute::InterfaceWatch {
                        stream_id,
                        reservation: watches.reserve(
                            commands.clone(),
                            self.request.link_id,
                            stream_id,
                            controller,
                        ),
                    },
                    None => PreparedRequestRoute::Declined(Decline::Ignore),
                };
            }
        }
        self
    }
}

fn prepare_request<St, R: RequestEndpointSet<St>>(
    remote_control: &mut AssembledRemoteControl,
    request: RunnerRequest,
) -> PreparedRunnerRequest {
    let route = if let Some((controller_grants, available_requests, self_announcement)) =
        remote_control.request_configuration_mut(request.destination, request.path_hash)
    {
        match admit_remote_control_request(
            controller_grants,
            available_requests,
            self_announcement,
            &request.inbound(),
        ) {
            Ok(admission) => {
                match verify_admitted_remote_control_request(admission, &request.inbound()) {
                    Ok(verified) => PreparedRequestRoute::RemoteControl(Box::new(verified)),
                    Err(decline) => PreparedRequestRoute::Declined(decline),
                }
            }
            Err(error) => {
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    target: "prns.runtime",
                    event = "remote_control_admit_declined",
                    reason = ?error,
                    has_grant = request
                        .requester
                        .is_some_and(|controller| controller_grants.grant_for(&controller).is_some()),
                    controller = ?request.requester.map(|identity| *identity.as_bytes()),
                    destination = ?request.destination.as_bytes(),
                );
                PreparedRequestRoute::Declined(error.into())
            }
        }
    } else if R::REGISTRATIONS.iter().any(|(path, policy)| {
        RequestPathHash::of(path) == request.path_hash
            && *policy == RequestEndpointPolicy::AllowRemoteControlControllers
    }) {
        let authorized = request.requester.is_some_and(|requester| {
            remote_control
                .controller_grants()
                .is_some_and(|grants| grants.contains_controller(&requester))
        });
        match authorized {
            true => PreparedRequestRoute::Application,
            false => PreparedRequestRoute::Declined(Decline::Ignore),
        }
    } else {
        PreparedRequestRoute::Application
    };
    #[cfg(feature = "tracing")]
    tracing::debug!(
        target: "prns.runtime",
        event = "request_routed",
        route = match &route {
            PreparedRequestRoute::RemoteControl(_) | PreparedRequestRoute::InterfaceWatch { .. } => "remote_control",
            PreparedRequestRoute::Application => "application",
            PreparedRequestRoute::Declined(_) => "declined",
        },
        destination = ?request.destination.as_bytes(),
        path_hash = ?request.path_hash,
        link_id = ?request.link_id.as_bytes(),
    );
    PreparedRunnerRequest { request, route }
}

enum RunnerResponse {
    Buffered(std::vec::Vec<u8>),
    StaticFile {
        name: &'static str,
        bytes: &'static [u8],
    },
    OpenBytes {
        file: std::fs::File,
        byte_len: u64,
    },
    OpenFile {
        name: std::string::String,
        file: std::fs::File,
        byte_len: u64,
    },
}

impl ResponseSink for RunnerResponse {
    fn put_packed(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            Self::Buffered(body) => ResponseSink::put_packed(body, bytes),
            Self::StaticFile { .. } | Self::OpenBytes { .. } | Self::OpenFile { .. } => {
                Err(ResponseCapacityExceeded)
            }
        }
    }

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            Self::Buffered(body) => ResponseSink::put_bytes(body, bytes),
            Self::StaticFile { .. } | Self::OpenBytes { .. } | Self::OpenFile { .. } => {
                Err(ResponseCapacityExceeded)
            }
        }
    }

    fn put_static_file(
        &mut self,
        name: &'static str,
        bytes: &'static [u8],
    ) -> Result<(), ResponseCapacityExceeded> {
        match self {
            Self::Buffered(body) if body.is_empty() => {
                *self = Self::StaticFile { name, bytes };
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }

    fn put_open_bytes(
        &mut self,
        file: std::fs::File,
        byte_len: u64,
    ) -> Result<(), ResponseCapacityExceeded> {
        match self {
            Self::Buffered(body) if body.is_empty() => {
                *self = Self::OpenBytes { file, byte_len };
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }

    fn put_open_file(
        &mut self,
        name: &str,
        file: std::fs::File,
        byte_len: u64,
    ) -> Result<(), ResponseCapacityExceeded> {
        match self {
            Self::Buffered(body) if body.is_empty() => {
                *self = Self::OpenFile {
                    name: name.to_owned(),
                    file,
                    byte_len,
                };
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }
}

pub(super) async fn run_router<St, C, R: RequestEndpointSet<St>>(
    state: &St,
    controls: &C,
    remote_control: &mut AssembledRemoteControl,
    mut requests: mpsc::Receiver<RunnerRequest>,
    interface_watches: &InterfaceWatchRegistry,
    authorization: RemoteControlAuthorizationRuntime<'_>,
    commands: PrnsNodeHandle,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure>
where
    C: prns_runtime::runtime::RemoteControlHostControls
        + prns_runtime::runtime::RemoteControlAppMessages<St>,
{
    run_with_watches(
        async {
            let mut in_flight = FuturesUnordered::new();
            let mut response_lanes: std::collections::HashMap<LinkId, Weak<Mutex<()>>> =
                std::collections::HashMap::new();
            loop {
                let accepting = in_flight.len() < MAX_IN_FLIGHT;
                tokio::select! {
                    biased;
                    Some(()) = in_flight.next(), if !in_flight.is_empty() => {}
                    Some(command) = authorization.controller_grants.receive() => {
                        command.apply(remote_control, authorization.persistence).await?;
                        if let Some(grants) = remote_control.controller_grants() {
                            interface_watches.reconcile_grants(grants);
                        } else {
                            interface_watches.cancel_all();
                        }
                    }
                    Some(command) = authorization.target_accesses.receive() => {
                        command.apply(remote_control);
                    }
                    Some(command) = authorization.pairing_persistence.receive() => {
                        command
                            .apply(remote_control, authorization.persistence, &commands)
                            .await?;
                        if let Some(grants) = remote_control.controller_grants() {
                            interface_watches.reconcile_grants(grants);
                        } else {
                            interface_watches.cancel_all();
                        }
                    }
                    request = requests.recv(), if accepting => match request {
                        Some(request) => {
                            response_lanes.retain(|_, lane| lane.strong_count() > 0);
                            let response_lane = response_lanes
                                .get(&request.link_id)
                                .and_then(Weak::upgrade)
                                .unwrap_or_else(|| {
                                    let lane = Arc::new(Mutex::new(()));
                                    response_lanes.insert(request.link_id, Arc::downgrade(&lane));
                                    lane
                                });
                            let request = prepare_request::<St, R>(remote_control, request)
                                .reserve_interface_watch(&commands, interface_watches);
                            in_flight.push(dispatch_guarded::<St, C, R>(
                                state,
                                controls,
                                &commands,
                                request,
                                response_lane,
                                interface_watches,
                            ));
                        }
                        None => return Ok(()),
                    },
                }
            }
        },
        interface_watches,
    )
    .await
}

async fn run_with_watches(
    router: impl std::future::Future<Output = Result<(), RemoteControlAuthorizationPersistenceFailure>>,
    watches: &InterfaceWatchRegistry,
) -> Result<(), RemoteControlAuthorizationPersistenceFailure> {
    tokio::select! {
        biased;
        result = router => result,
        never = watches.run() => match never {},
    }
}

async fn dispatch_guarded<St, C, R: RequestEndpointSet<St>>(
    state: &St,
    controls: &C,
    commands: &PrnsNodeHandle,
    request: PreparedRunnerRequest,
    response_lane: Arc<Mutex<()>>,
    interface_watches: &InterfaceWatchRegistry,
) where
    C: prns_runtime::runtime::RemoteControlHostControls
        + prns_runtime::runtime::RemoteControlAppMessages<St>,
{
    let link_id = request.request.link_id;
    if AssertUnwindSafe(dispatch::<St, C, R>(
        state,
        controls,
        commands,
        request,
        response_lane,
        interface_watches,
    ))
    .catch_unwind()
    .await
    .is_err()
    {
        commands.close_link(link_id);
    }
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        name = "prns.respond",
        level = "debug",
        skip_all,
        fields(
            bytes = request.request.data.len(),
            link_id = ?request.request.link_id.as_bytes(),
            path_hash = ?request.request.path_hash,
        )
    )
)]
async fn dispatch<St, C, R: RequestEndpointSet<St>>(
    state: &St,
    controls: &C,
    commands: &PrnsNodeHandle,
    request: PreparedRunnerRequest,
    response_lane: Arc<Mutex<()>>,
    interface_watches: &InterfaceWatchRegistry,
) where
    C: prns_runtime::runtime::RemoteControlHostControls
        + prns_runtime::runtime::RemoteControlAppMessages<St>,
{
    let PreparedRunnerRequest { request, route } = request;
    let link_id = request.link_id;
    let inbound = request.inbound();
    let responder = inbound.respond_token();
    let mut body = RunnerResponse::Buffered(std::vec::Vec::new());
    let dispatched = match route {
        PreparedRequestRoute::RemoteControl(verified) => {
            let verified = *verified;
            if let Some(grant) = verified.authorize_controller_grant() {
                let _response_guard = response_lane.lock().await;
                commands
                    .authorize_remote_control_controller_and_respond(grant, responder)
                    .await;
                return;
            }
            if let Some(controller) = verified.revoke_controller_grant() {
                let _response_guard = response_lane.lock().await;
                commands
                    .revoke_remote_control_controller_and_respond(controller, responder)
                    .await;
                return;
            }
            dispatch_verified_admitted_remote_control_request(
                state, controls, commands, inbound, &mut body, verified,
            )
            .await
        }
        PreparedRequestRoute::InterfaceWatch {
            stream_id,
            reservation,
        } => {
            let _response_guard = response_lane.lock().await;
            if let Ok(reserved) = &reservation {
                if matches!(
                    interface_watches.admission(reserved),
                    WatchAdmission::Withdrawn
                ) {
                    return;
                }
            }
            let response = match &reservation {
                Ok(_) => RemoteControlResponse::WatchInterfaces { stream_id },
                Err(_) => RemoteControlResponse::ProtocolError(RemoteControlProtocolError::Busy {
                    request: RemoteControlRequestKind::WatchInterfaces,
                }),
            };
            let mut encoded = [0; RemoteControlResponse::MAX_ENCODED_LEN];
            let Ok(len) = response.write_into(&mut encoded) else {
                if let Ok(reserved) = &reservation {
                    interface_watches.cancel(reserved);
                }
                return;
            };
            let Some(bytes) = encoded.get(..len) else {
                if let Ok(reserved) = &reservation {
                    interface_watches.cancel(reserved);
                }
                return;
            };
            let settled = commands
                .respond_owned_packed_settled(responder, bytes.to_vec())
                .await;
            match settled {
                Ok(_) => {
                    if let Ok(start) = reservation {
                        start.start();
                    }
                }
                Err(error) => {
                    if let Ok(reserved) = &reservation {
                        interface_watches.cancel(reserved);
                    }
                    if matches!(
                        response_failure_cleanup(&error),
                        ResponseFailureCleanup::CloseLink
                    ) {
                        commands.close_link(link_id);
                    }
                }
            }
            return;
        }
        PreparedRequestRoute::Application => {
            dispatch_request::<St, R>(state, commands, request.path_hash, inbound, &mut body).await
        }
        PreparedRequestRoute::Declined(decline) => Err(decline),
    };
    match dispatched {
        Ok(()) => {
            let _response_guard = response_lane.lock().await;
            let result = match body {
                RunnerResponse::Buffered(body) => {
                    commands.respond_owned_packed_settled(responder, body).await
                }
                RunnerResponse::StaticFile { name, bytes } => {
                    commands
                        .respond_static_file_settled(responder, name, bytes)
                        .await
                }
                RunnerResponse::OpenBytes { file, byte_len } => {
                    commands
                        .respond_bytes_streaming(
                            responder,
                            byte_len,
                            tokio::fs::File::from_std(file),
                        )
                        .await
                }
                RunnerResponse::OpenFile {
                    name,
                    file,
                    byte_len,
                } => {
                    commands
                        .respond_open_file_settled(responder, &name, file, byte_len)
                        .await
                }
            };
            if let Err(error) = result {
                if matches!(
                    response_failure_cleanup(&error),
                    ResponseFailureCleanup::AlreadyClosed
                ) {
                    #[cfg(feature = "tracing")]
                    tracing::debug!(
                        target: "prns.runtime",
                        event = "request_response_link_gone",
                        link_id = ?link_id.as_bytes(),
                    );
                } else {
                    #[cfg(feature = "tracing")]
                    tracing::warn!(
                        target: "prns.runtime",
                        event = "request_response_failed",
                        error = ?error,
                        link_id = ?link_id.as_bytes(),
                    );
                    commands.close_link(link_id);
                }
            }
        }
        Err(Decline::Ignore) => {
            #[cfg(feature = "tracing")]
            tracing::debug!(
                target: "prns.runtime",
                event = "request_declined",
                reason = "ignore",
                path_hash = ?request.path_hash,
                link_id = ?link_id.as_bytes(),
            );
        }
        Err(Decline::CloseLink) => {
            commands.close_link(responder.link_id);
        }
        Err(Decline::ResponseTooLarge) => {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                target: "prns.runtime",
                event = "request_response_too_large",
                link_id = ?link_id.as_bytes(),
            );
        }
    }
}

enum ResponseFailureCleanup {
    AlreadyClosed,
    CloseLink,
}

fn response_failure_cleanup(error: &ResponseSendError) -> ResponseFailureCleanup {
    match error {
        ResponseSendError::Rejected(
            RespondFailure::Rejected(RespondRejection::NoSuchLink)
            | RespondFailure::Resource(SendResourceFailure::Rejected(
                SendResourceRejection::NoSuchLink,
            )),
        ) => ResponseFailureCleanup::AlreadyClosed,
        _ => ResponseFailureCleanup::CloseLink,
    }
}

#[cfg(test)]
mod tests;
