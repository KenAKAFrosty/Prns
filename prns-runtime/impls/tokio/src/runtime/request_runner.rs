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
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use prns_runtime::runtime::placement::{
    admit_remote_control_request, dispatch_admitted_remote_control_request,
    AdmittedRemoteControlRequest,
};

use super::node_facade::{PrnsNodeHandle, ResponseSendError};
use super::remote_control_access::RemoteControlAccessReceiver;
use super::request_endpoints::{dispatch_request, Decline, InboundRequest, RequestEndpointSet};
use super::request_endpoints::{ResponseCapacityExceeded, ResponseSink};
use super::AssembledRemoteControl;

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
    RemoteControl(AdmittedRemoteControlRequest),
    Declined(Decline),
}

struct PreparedRunnerRequest {
    request: RunnerRequest,
    route: PreparedRequestRoute,
}

fn prepare_request(
    remote_control: &AssembledRemoteControl,
    request: RunnerRequest,
) -> PreparedRunnerRequest {
    let route = if let Some((access, available_requests, self_announcement)) =
        remote_control.request_configuration(request.destination, request.path_hash)
    {
        match admit_remote_control_request(
            access,
            available_requests,
            self_announcement,
            &request.inbound(),
        ) {
            Ok(admission) => PreparedRequestRoute::RemoteControl(admission),
            Err(decline) => PreparedRequestRoute::Declined(decline),
        }
    } else {
        PreparedRequestRoute::Application
    };
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

pub(super) async fn run_router<St, R: RequestEndpointSet<St>>(
    state: &St,
    remote_control: &mut AssembledRemoteControl,
    mut requests: mpsc::Receiver<RunnerRequest>,
    remote_control_access: &mut RemoteControlAccessReceiver,
    commands: PrnsNodeHandle,
) {
    let mut in_flight = FuturesUnordered::new();
    let mut response_lanes: std::collections::HashMap<LinkId, Weak<Mutex<()>>> =
        std::collections::HashMap::new();
    loop {
        let accepting = in_flight.len() < MAX_IN_FLIGHT;
        tokio::select! {
            biased;
            Some(()) = in_flight.next(), if !in_flight.is_empty() => {}
            Some(command) = remote_control_access.receive() => {
                command.apply(remote_control);
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
                    let request = prepare_request(remote_control, request);
                    in_flight.push(dispatch_guarded::<St, R>(
                        state,
                        &commands,
                        request,
                        response_lane,
                    ));
                }
                None => break,
            },
        }
    }
}

async fn dispatch_guarded<St, R: RequestEndpointSet<St>>(
    state: &St,
    commands: &PrnsNodeHandle,
    request: PreparedRunnerRequest,
    response_lane: Arc<Mutex<()>>,
) {
    let link_id = request.request.link_id;
    if AssertUnwindSafe(dispatch::<St, R>(state, commands, request, response_lane))
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
async fn dispatch<St, R: RequestEndpointSet<St>>(
    state: &St,
    commands: &PrnsNodeHandle,
    request: PreparedRunnerRequest,
    response_lane: Arc<Mutex<()>>,
) {
    let PreparedRunnerRequest { request, route } = request;
    let link_id = request.link_id;
    let inbound = request.inbound();
    let responder = inbound.respond_token();
    let mut body = RunnerResponse::Buffered(std::vec::Vec::new());
    let dispatched = match route {
        PreparedRequestRoute::RemoteControl(admission) => {
            dispatch_admitted_remote_control_request(state, commands, inbound, &mut body, admission)
                .await
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
                let link_already_gone = matches!(
                    error,
                    ResponseSendError::Rejected(
                        RespondFailure::Rejected(RespondRejection::NoSuchLink)
                            | RespondFailure::Resource(SendResourceFailure::Rejected(
                                SendResourceRejection::NoSuchLink,
                            )),
                    )
                );
                if link_already_gone {
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
        Err(Decline::Ignore) => {}
        Err(Decline::CloseLink) => {
            commands.close_link(responder.link_id);
        }
        Err(Decline::ResponseTooLarge) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineState, IssuedCommand, PrnsCommand, Settlement};
    use crate::manifold::driver::HostCommand;
    use crate::routing::request_handlers::RequestPathHash;
    use crate::runtime::request_endpoints::{RequestContext, RequestEndpointPolicy};
    use crate::storage::GrowableHeap;

    fn remote_control() -> AssembledRemoteControl {
        let mut engine = EngineState::<GrowableHeap>::default();
        crate::runtime::configure_remote_control_service(
            &mut engine,
            super::super::node_facade::test_remote_control_service(),
        )
        .expect("RemoteControl fits growable storage")
    }

    #[test]
    fn static_file_sink_preserves_filename_and_borrowed_bytes() {
        static FILE: [u8; 32] = [0x42; 32];
        let mut response = RunnerResponse::Buffered(std::vec::Vec::new());
        ResponseSink::put_static_file(&mut response, "source.zip", &FILE).unwrap();
        let RunnerResponse::StaticFile { name, bytes } = response else {
            panic!("static file response");
        };
        assert_eq!(name, "source.zip");
        assert_eq!(bytes.as_ptr(), FILE.as_ptr());
    }

    #[test]
    fn open_file_sink_retains_the_handle_without_reading_it() {
        let source = std::fs::File::open("Cargo.toml").unwrap();
        let mut response = RunnerResponse::Buffered(std::vec::Vec::new());
        ResponseSink::put_open_file(&mut response, "source.zip", source, 42).unwrap();
        let RunnerResponse::OpenFile { name, byte_len, .. } = response else {
            panic!("open file response");
        };
        assert_eq!(name, "source.zip");
        assert_eq!(byte_len, 42);
    }

    struct PanickingRequestEndpointSet;

    impl RequestEndpointSet<()> for PanickingRequestEndpointSet {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[];

        async fn dispatch(
            _context: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
            _path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            std::panic::panic_any("request handler")
        }
    }

    #[tokio::test]
    async fn a_panicking_request_handler_closes_its_link() {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let handle = PrnsNodeHandle::over(commands);
        let remote_control = remote_control();
        let link_id = LinkId::new([0x44; 16]);
        dispatch_guarded::<(), PanickingRequestEndpointSet>(
            &(),
            &handle,
            prepare_request(
                &remote_control,
                RunnerRequest {
                    destination: DestinationHash::new([0x33; 16]),
                    link_id,
                    request_id: RequestId([0x55; 16]),
                    requester: None,
                    path_hash: RequestPathHash::new([0x66; 16]),
                    requested_at: InstantMillis(700),
                    rtt: RttMillis::new(80),
                    data: std::vec::Vec::new(),
                },
            ),
            Arc::new(Mutex::new(())),
        )
        .await;

        assert!(matches!(
            command_rx.recv().await,
            Some(HostCommand::Engine(IssuedCommand {
                command: PrnsCommand::CloseLink(close),
                ..
            })) if close.link_id == link_id
        ));
    }

    struct PongRequestEndpointSet;

    impl RequestEndpointSet<()> for PongRequestEndpointSet {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[];

        async fn dispatch(
            mut context: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
            _path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            context.respond("pong")
        }
    }

    async fn drive_response_settling_to(
        failure: RespondFailure,
    ) -> mpsc::UnboundedReceiver<HostCommand> {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let handle = PrnsNodeHandle::over(commands);
        let remote_control = remote_control();
        let dispatched = dispatch_guarded::<(), PongRequestEndpointSet>(
            &(),
            &handle,
            prepare_request(
                &remote_control,
                RunnerRequest {
                    destination: DestinationHash::new([0x33; 16]),
                    link_id: LinkId::new([0x44; 16]),
                    request_id: RequestId([0x55; 16]),
                    requester: None,
                    path_hash: RequestPathHash::new([0x66; 16]),
                    requested_at: InstantMillis(700),
                    rtt: RttMillis::new(80),
                    data: std::vec::Vec::new(),
                },
            ),
            Arc::new(Mutex::new(())),
        );
        let settled = async {
            let Some(HostCommand::RespondAny(respond)) = command_rx.recv().await else {
                panic!("respond command");
            };
            respond
                .completion
                .unwrap()
                .send(Settlement::Respond(Err(failure)))
                .unwrap();
            command_rx
        };
        let ((), command_rx) = tokio::join!(dispatched, settled);
        command_rx
    }

    #[tokio::test]
    async fn a_failed_response_closes_its_link() {
        let mut command_rx = drive_response_settling_to(RespondFailure::WriteFailed).await;
        assert!(matches!(
            command_rx.recv().await,
            Some(HostCommand::Engine(IssuedCommand {
                command: PrnsCommand::CloseLink(close),
                ..
            })) if close.link_id == LinkId::new([0x44; 16])
        ));
    }

    #[tokio::test]
    async fn a_response_rejected_for_a_vanished_link_does_not_close_it_again() {
        let mut command_rx =
            drive_response_settling_to(RespondFailure::Rejected(RespondRejection::NoSuchLink))
                .await;
        assert!(command_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_resource_response_rejected_for_a_vanished_link_does_not_close_it_again() {
        let mut command_rx = drive_response_settling_to(RespondFailure::Resource(
            SendResourceFailure::Rejected(SendResourceRejection::NoSuchLink),
        ))
        .await;
        assert!(command_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn router_applies_ready_remote_control_access_before_a_ready_request() {
        use crate::remote_control::{
            RemoteControlDescription, RemoteControlRequest, RemoteControlRequestKind,
            RemoteControlRequestSet, RemoteControlResponse, RevokeRemoteControlControllerOutcome,
            SetRemoteControlControllerGrantOutcome,
        };
        use crate::runtime::RemoteControlAccessControl;

        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (handle, mut access) = PrnsNodeHandle::over_with_remote_control_access(commands);
        let (request_tx, request_rx) = mpsc::channel(1);
        let mut remote_control = remote_control();
        let destination = remote_control.target_endpoint().unwrap().destination_hash();
        let path_hash = remote_control.request_endpoint_id().unwrap();
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let mut data = std::vec![0; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlRequest::Describe
            .write_into(data.as_mut_slice())
            .expect("Describe fits its maximum encoded length");
        data.truncate(encoded_len);
        request_tx
            .send(RunnerRequest {
                destination,
                link_id: LinkId::new([0x71; 16]),
                request_id: RequestId([0x72; 16]),
                requester: Some(grant.controller().identity_hash()),
                path_hash,
                requested_at: InstantMillis(73),
                rtt: RttMillis::new(74),
                data,
            })
            .await
            .expect("request lane remains open");

        let setting = handle.set_remote_control_controller_grant(grant);
        tokio::pin!(setting);
        tokio::select! {
            biased;
            outcome = &mut setting => panic!("unsettled access change returned: {outcome:?}"),
            () = tokio::task::yield_now() => {}
        }

        let router = run_router::<(), PongRequestEndpointSet>(
            &(),
            &mut remote_control,
            request_rx,
            &mut access,
            handle.clone(),
        );
        let exercise = async {
            assert_eq!(
                setting.await,
                Ok(SetRemoteControlControllerGrantOutcome::Added),
            );
            let Some(HostCommand::RespondAny(response)) = command_rx.recv().await else {
                panic!("RemoteControl response command")
            };
            let expected = RemoteControlDescription::try_from(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            ))
            .expect("Describe is available");
            assert_eq!(
                RemoteControlResponse::parse(response.packed.as_slice()),
                Ok(RemoteControlResponse::Describe(expected)),
            );
            let Some(completion) = response.completion else {
                panic!("settled RemoteControl response")
            };
            assert!(completion.send(Settlement::Respond(Ok(()))).is_ok());
            assert_eq!(
                handle
                    .revoke_remote_control_controller(*grant.controller())
                    .await,
                Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }),
            );
        };
        tokio::pin!(router);
        tokio::select! {
            biased;
            () = exercise => {}
            () = &mut router => panic!("router returned while its lanes remained open"),
        }
    }
}
