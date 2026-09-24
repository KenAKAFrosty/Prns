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

fn remote_control_with_administration() -> AssembledRemoteControl {
    let mut engine = EngineState::<GrowableHeap>::default();
    let capabilities = crate::remote_control::RemoteControlCapabilities::from_requests(
        crate::remote_control::RemoteControlRequestSet::all(),
    )
    .unwrap();
    crate::runtime::configure_remote_control_service(
        &mut engine,
        super::super::node_facade::test_remote_control_service_with_capabilities(capabilities),
    )
    .expect("Remote Control fits growable storage")
}

fn controller(fill: u8) -> crate::remote_control::RemoteControlControllerIdentity {
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::{IdentityPublicKeys, IdentitySigner};

    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
        [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
    ));
    crate::remote_control::RemoteControlControllerIdentity::new(IdentityPublicKeys {
        encryption: identity.encryption_public_key(),
        signing: identity.signing_public_key(),
    })
}

fn authorize_controller_request(
    remote_control: &AssembledRemoteControl,
    administrator: crate::remote_control::RemoteControlControllerIdentity,
    operator: crate::remote_control::RemoteControlControllerIdentity,
) -> RunnerRequest {
    use crate::remote_control::{
        RemoteControlRequest, RemoteControlRequestKind, RemoteControlRequestSet,
    };

    let mut data = std::vec![0; RemoteControlRequest::MAX_ENCODED_LEN];
    let encoded_len = RemoteControlRequest::AuthorizeController {
        controller: operator,
        permitted_requests: RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
    }
    .write_into(data.as_mut_slice())
    .unwrap();
    data.truncate(encoded_len);
    RunnerRequest {
        destination: remote_control.target_endpoint().unwrap().destination_hash(),
        link_id: LinkId::new([0x81; 16]),
        request_id: RequestId([0x82; 16]),
        requester: Some(administrator.identity_hash()),
        path_hash: remote_control.request_endpoint_id().unwrap(),
        requested_at: InstantMillis(83),
        rtt: RttMillis::new(84),
        data,
    }
}

struct ControllerRequestEndpointSet;

impl RequestEndpointSet<crate::runtime::NoRemoteControlHostControls>
    for ControllerRequestEndpointSet
{
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[(
        "/controller",
        RequestEndpointPolicy::AllowRemoteControlControllers,
    )];

    async fn dispatch(
        _context: RequestContext<'_, crate::runtime::NoRemoteControlHostControls>,
        _node: &impl crate::runtime::PrnsNodeApi,
        _path_hash: RequestPathHash,
    ) -> Result<(), Decline> {
        Err(Decline::Ignore)
    }
}

#[test]
fn controller_routes_follow_the_live_grant_table() {
    use crate::remote_control::{
        RemoteControlControllerAuthority, RemoteControlControllerGrant, RemoteControlRequestSet,
    };

    let mut remote_control = remote_control();
    let controller = controller(0x41);
    let request = || RunnerRequest {
        destination: DestinationHash::new([0x5a; 16]),
        link_id: LinkId::new([1; 16]),
        request_id: RequestId([2; 16]),
        requester: Some(controller.identity_hash()),
        path_hash: RequestPathHash::of("/controller"),
        requested_at: InstantMillis(3),
        rtt: RttMillis::new(4),
        data: std::vec::Vec::new(),
    };
    assert!(matches!(
        prepare_request::<crate::runtime::NoRemoteControlHostControls, ControllerRequestEndpointSet>(
            &mut remote_control,
            request()
        ),
        PreparedRunnerRequest {
            route: PreparedRequestRoute::Declined(Decline::Ignore),
            ..
        }
    ));

    remote_control
        .set_controller_grant(
            RemoteControlControllerGrant::new(
                controller,
                RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::all_operator(),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        prepare_request::<crate::runtime::NoRemoteControlHostControls, ControllerRequestEndpointSet>(
            &mut remote_control,
            request()
        ),
        PreparedRunnerRequest {
            route: PreparedRequestRoute::Application,
            ..
        }
    ));
}

#[tokio::test]
async fn revocation_while_waiting_for_a_response_lane_remains_silent() {
    use crate::remote_control::{
        FixedRemoteControlControllerGrantTable, RemoteControlControllerAuthority,
        RemoteControlControllerGrant, RemoteControlRequest, RemoteControlRequestSet,
    };
    let (commands, mut receiver) = mpsc::unbounded_channel();
    let handle = PrnsNodeHandle::over(commands);
    let watches = InterfaceWatchRegistry::default();
    let mut remote_control = remote_control_with_administration();
    let controller = controller(0x56);
    remote_control
        .set_controller_grant(
            RemoteControlControllerGrant::new(
                controller,
                RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::only(RemoteControlRequestKind::WatchInterfaces),
            )
            .unwrap(),
        )
        .unwrap();
    let mut data = vec![0; RemoteControlRequest::MAX_ENCODED_LEN];
    let len = RemoteControlRequest::WatchInterfaces {
        stream_id: StreamId::new(3).unwrap(),
    }
    .write_into(&mut data)
    .unwrap();
    data.truncate(len);
    let destination = remote_control.target_endpoint().unwrap().destination_hash();
    let path_hash = remote_control.request_endpoint_id().unwrap();
    let prepared =
        prepare_request::<crate::runtime::NoRemoteControlHostControls, PongRequestEndpointSet>(
            &mut remote_control,
            RunnerRequest {
                destination,
                link_id: LinkId::new([0x57; 16]),
                request_id: RequestId([0x58; 16]),
                requester: Some(controller.identity_hash()),
                path_hash,
                requested_at: InstantMillis(59),
                rtt: RttMillis::new(60),
                data,
            },
        )
        .reserve_interface_watch(&handle, &watches);
    assert!(matches!(
        prepared.route,
        PreparedRequestRoute::InterfaceWatch { .. }
    ));
    let lane = Arc::new(Mutex::new(()));
    let guard = lane.lock().await;
    let dispatch = dispatch_guarded::<
        crate::runtime::NoRemoteControlHostControls,
        crate::runtime::NoRemoteControlHostControls,
        PongRequestEndpointSet,
    >(
        &crate::runtime::NoRemoteControlHostControls,
        &crate::runtime::NoRemoteControlHostControls,
        &handle,
        prepared,
        lane.clone(),
        &watches,
    );
    tokio::pin!(dispatch);
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(dispatch.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    watches.reconcile_grants(&FixedRemoteControlControllerGrantTable::<8>::default());
    drop(guard);
    dispatch.await;
    tokio::task::yield_now().await;
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test(start_paused = true)]
async fn watch_heartbeats_continue_while_the_request_router_is_blocked() {
    use crate::engine::{DeliveryEvidence, DeliveryProof, PacketReceiptDelivered};
    use crate::remote_control::RemoteControlStreamEvent;
    use crate::routing::dedup::PacketHash;
    use crate::routing::links::channel::byte_stream::{parse, StreamId};

    let (commands, mut receiver) = mpsc::unbounded_channel();
    let handle = PrnsNodeHandle::over(commands);
    let watches = InterfaceWatchRegistry::default();
    watches
        .reserve(
            handle,
            LinkId::new([7; 16]),
            StreamId::new(3).unwrap(),
            IdentityHash::new([8; 16]),
        )
        .unwrap()
        .start();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let router = async {
        blocked.await.unwrap();
        Ok(())
    };
    let driven = run_with_watches(router, &watches);
    tokio::pin!(driven);
    for expected in [
        RemoteControlStreamEvent::ResyncRequired { sequence: 1 },
        RemoteControlStreamEvent::Heartbeat { sequence: 2 },
    ] {
        if expected.sequence() == 2 {
            tokio::time::advance(std::time::Duration::from_secs(5)).await;
        }
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(driven.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        let HostCommand::AwaitedEngine { issued, completion } = receiver.try_recv().unwrap() else {
            panic!("watch event while request handling is blocked");
        };
        let PrnsCommand::SendToChannel(send) = issued.command else {
            panic!("watch channel frame");
        };
        let event =
            RemoteControlStreamEvent::parse(parse(send.body.as_slice()).unwrap().payload).unwrap();
        assert_eq!(event, expected);
        completion
            .send(Settlement::SendToChannel(Ok(PacketReceiptDelivered {
                rtt: RttMillis::new(0),
                evidence: DeliveryEvidence::Proof(DeliveryProof::Implicit(PacketHash::new(
                    [0; 32],
                ))),
            })))
            .unwrap();
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(driven.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
    }
    release.send(()).unwrap();
    assert_eq!(driven.await, Ok(()));
}

#[tokio::test]
async fn admitted_watch_responds_before_its_initial_resync_frame() {
    use crate::remote_control::{
        RemoteControlControllerAuthority, RemoteControlControllerGrant, RemoteControlRequest,
        RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlResponse,
        RemoteControlStreamEvent, REMOTE_CONTROL_STREAM_EVENT_LEN,
    };
    use crate::routing::links::channel::byte_stream::{parse, StreamId};

    let (commands, mut command_rx) = mpsc::unbounded_channel();
    let handle = PrnsNodeHandle::over(commands);
    let mut remote_control = remote_control_with_administration();
    let controller = controller(0x56);
    remote_control
        .set_controller_grant(
            RemoteControlControllerGrant::new(
                controller,
                RemoteControlControllerAuthority::Operator,
                RemoteControlRequestSet::only(RemoteControlRequestKind::WatchInterfaces),
            )
            .unwrap(),
        )
        .unwrap();
    let stream_id = StreamId::new(0x234).unwrap();
    let mut data = vec![0; RemoteControlRequest::MAX_ENCODED_LEN];
    let len = RemoteControlRequest::WatchInterfaces { stream_id }
        .write_into(&mut data)
        .unwrap();
    data.truncate(len);
    let request = RunnerRequest {
        destination: remote_control.target_endpoint().unwrap().destination_hash(),
        link_id: LinkId::new([0x57; 16]),
        request_id: RequestId([0x58; 16]),
        requester: Some(controller.identity_hash()),
        path_hash: remote_control.request_endpoint_id().unwrap(),
        requested_at: InstantMillis(59),
        rtt: RttMillis::new(60),
        data,
    };
    let prepared = prepare_request::<
        crate::runtime::NoRemoteControlHostControls,
        PongRequestEndpointSet,
    >(&mut remote_control, request);
    let watches = InterfaceWatchRegistry::default();
    let prepared = prepared.reserve_interface_watch(&handle, &watches);
    let dispatch = dispatch_guarded::<
        crate::runtime::NoRemoteControlHostControls,
        crate::runtime::NoRemoteControlHostControls,
        PongRequestEndpointSet,
    >(
        &crate::runtime::NoRemoteControlHostControls,
        &crate::runtime::NoRemoteControlHostControls,
        &handle,
        prepared,
        Arc::new(Mutex::new(())),
        &watches,
    );
    let exercise = async {
        let Some(HostCommand::RespondAny(response)) = command_rx.recv().await else {
            panic!("watch acceptance response");
        };
        assert_eq!(
            RemoteControlResponse::parse(response.packed.as_slice()),
            Ok(RemoteControlResponse::WatchInterfaces { stream_id }),
        );
        assert!(command_rx.try_recv().is_err());
        response
            .completion
            .unwrap()
            .send(Settlement::Respond(Ok(())))
            .unwrap();
        let Some(HostCommand::AwaitedEngine { issued, completion }) =
            tokio::time::timeout(std::time::Duration::from_secs(1), command_rx.recv())
                .await
                .unwrap()
        else {
            panic!("initial watch stream frame");
        };
        let PrnsCommand::SendToChannel(send) = issued.command else {
            panic!("watch frame uses the link channel");
        };
        let frame = parse(send.body.as_slice()).unwrap();
        assert_eq!(frame.header.stream_id, stream_id);
        assert!(!frame.header.eof);
        assert_eq!(frame.payload.len(), REMOTE_CONTROL_STREAM_EVENT_LEN);
        assert_eq!(
            RemoteControlStreamEvent::parse(frame.payload),
            Ok(RemoteControlStreamEvent::ResyncRequired { sequence: 1 }),
        );
        drop(completion);
    };
    tokio::select! {
        () = async { tokio::join!(dispatch, exercise); } => {},
        () = async { loop { watches.next_completion().await; } } => unreachable!(),
    }
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

impl RequestEndpointSet<crate::runtime::NoRemoteControlHostControls>
    for PanickingRequestEndpointSet
{
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[];

    async fn dispatch(
        _context: RequestContext<'_, crate::runtime::NoRemoteControlHostControls>,
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
    let mut remote_control = remote_control();
    let link_id = LinkId::new([0x44; 16]);
    let interface_watches = InterfaceWatchRegistry::default();
    dispatch_guarded::<
        crate::runtime::NoRemoteControlHostControls,
        crate::runtime::NoRemoteControlHostControls,
        PanickingRequestEndpointSet,
    >(
        &crate::runtime::NoRemoteControlHostControls,
        &crate::runtime::NoRemoteControlHostControls,
        &handle,
        prepare_request::<crate::runtime::NoRemoteControlHostControls, PanickingRequestEndpointSet>(
            &mut remote_control,
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
        &interface_watches,
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

impl RequestEndpointSet<crate::runtime::NoRemoteControlHostControls> for PongRequestEndpointSet {
    const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] = &[];

    async fn dispatch(
        mut context: RequestContext<'_, crate::runtime::NoRemoteControlHostControls>,
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
    let mut remote_control = remote_control();
    let interface_watches = InterfaceWatchRegistry::default();
    let dispatched = dispatch_guarded::<
        crate::runtime::NoRemoteControlHostControls,
        crate::runtime::NoRemoteControlHostControls,
        PongRequestEndpointSet,
    >(
        &crate::runtime::NoRemoteControlHostControls,
        &crate::runtime::NoRemoteControlHostControls,
        &handle,
        prepare_request::<crate::runtime::NoRemoteControlHostControls, PongRequestEndpointSet>(
            &mut remote_control,
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
        &interface_watches,
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
        drive_response_settling_to(RespondFailure::Rejected(RespondRejection::NoSuchLink)).await;
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
async fn router_applies_ready_remote_control_controller_grants_before_a_ready_request() {
    use crate::remote_control::{
        RemoteControlDescription, RemoteControlRequest, RemoteControlRequestKind,
        RemoteControlRequestSet, RemoteControlResponse, RevokeRemoteControlControllerOutcome,
        SetRemoteControlControllerGrantOutcome,
    };
    use crate::runtime::RemoteControlControllerGrantControl;

    let (commands, mut command_rx) = mpsc::unbounded_channel();
    let (handle, mut controller_grants) =
        PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
    let (request_tx, request_rx) = mpsc::channel(1);
    let mut remote_control = remote_control();
    let destination = remote_control.target_endpoint().unwrap().destination_hash();
    let path_hash = remote_control.request_endpoint_id().unwrap();
    let grant =
        super::super::node_facade::test_remote_control_grant(RemoteControlRequestKind::Describe);
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
        outcome = &mut setting => panic!("unsettled controller_grants change returned: {outcome:?}"),
        () = tokio::task::yield_now() => {}
    }

    let (_target_access_sender, mut target_accesses) =
        super::super::remote_control_target_accesses::remote_control_target_access_lane();
    let (_pairing_persistence_sender, mut pairing_persistence) =
        super::super::remote_control_pairing_persistence::remote_control_pairing_persistence_lane();
    let persistence_directory = std::env::temp_dir().join(format!(
        "prns-remote-control-router-grants-{}",
        std::process::id()
    ));
    let _removed = std::fs::remove_dir_all(&persistence_directory);
    let persistence_worker = super::super::NodePersistence::custom_dir(&persistence_directory)
        .unwrap()
        .worker(handle.clone());
    let authorization_persistence = persistence_worker.remote_control_authorization_persistence();

    {
        let interface_watches = InterfaceWatchRegistry::default();
        let router = run_router::<
            crate::runtime::NoRemoteControlHostControls,
            crate::runtime::NoRemoteControlHostControls,
            PongRequestEndpointSet,
        >(
            &crate::runtime::NoRemoteControlHostControls,
            &crate::runtime::NoRemoteControlHostControls,
            &mut remote_control,
            request_rx,
            &interface_watches,
            RemoteControlAuthorizationRuntime {
                controller_grants: &mut controller_grants,
                target_accesses: &mut target_accesses,
                pairing_persistence: &mut pairing_persistence,
                persistence: Some(&authorization_persistence),
            },
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
            result = &mut router => panic!("router returned while its lanes remained open: {result:?}"),
        }
    }
    drop(persistence_worker);
    std::fs::remove_dir_all(persistence_directory).unwrap();
}

#[tokio::test]
async fn remote_administrator_grant_survives_when_its_response_fails() {
    use crate::remote_control::{
        RemoteControlAuthorizeControllerOutcome, RemoteControlControllerAuthority,
        RemoteControlControllerGrant, RemoteControlControllerGrantTable, RemoteControlRequestKind,
        RemoteControlRequestSet, RemoteControlResponse,
    };

    let (commands, mut command_rx) = mpsc::unbounded_channel();
    let (handle, mut controller_grants) =
        PrnsNodeHandle::over_with_remote_control_controller_grant_lane(commands);
    let (request_tx, request_rx) = mpsc::channel(1);
    let mut remote_control = remote_control_with_administration();
    let administrator_identity =
        *super::super::node_facade::test_remote_control_grant(RemoteControlRequestKind::Describe)
            .controller();
    let administrator = RemoteControlControllerGrant::new(
        administrator_identity,
        RemoteControlControllerAuthority::Administrator,
        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
    )
    .unwrap();
    remote_control.set_controller_grant(administrator).unwrap();
    let operator = controller(0x46);
    request_tx
        .send(authorize_controller_request(
            &remote_control,
            administrator_identity,
            operator,
        ))
        .await
        .unwrap();

    let (_target_access_sender, mut target_accesses) =
        super::super::remote_control_target_accesses::remote_control_target_access_lane();
    let (_pairing_persistence_sender, mut pairing_persistence) =
        super::super::remote_control_pairing_persistence::remote_control_pairing_persistence_lane();
    let persistence_directory = std::env::temp_dir().join(format!(
        "prns-remote-control-response-rollback-{}",
        std::process::id()
    ));
    let _removed = std::fs::remove_dir_all(&persistence_directory);
    let persistence_worker = super::super::NodePersistence::custom_dir(&persistence_directory)
        .unwrap()
        .worker(handle.clone());
    let authorization_persistence = persistence_worker.remote_control_authorization_persistence();

    {
        let interface_watches = InterfaceWatchRegistry::default();
        let router = run_router::<
            crate::runtime::NoRemoteControlHostControls,
            crate::runtime::NoRemoteControlHostControls,
            PongRequestEndpointSet,
        >(
            &crate::runtime::NoRemoteControlHostControls,
            &crate::runtime::NoRemoteControlHostControls,
            &mut remote_control,
            request_rx,
            &interface_watches,
            RemoteControlAuthorizationRuntime {
                controller_grants: &mut controller_grants,
                target_accesses: &mut target_accesses,
                pairing_persistence: &mut pairing_persistence,
                persistence: Some(&authorization_persistence),
            },
            handle.clone(),
        );
        let exercise = async {
            let Some(HostCommand::RespondAny(response)) = command_rx.recv().await else {
                panic!("Remote Control response command")
            };
            assert_eq!(
                RemoteControlResponse::parse(response.packed.as_slice()),
                Ok(RemoteControlResponse::AuthorizeController(
                    RemoteControlAuthorizeControllerOutcome::Applied,
                )),
            );
            response
                .completion
                .unwrap()
                .send(Settlement::Respond(Err(RespondFailure::WriteFailed)))
                .unwrap();
            assert!(matches!(
                command_rx.recv().await,
                Some(HostCommand::Engine(IssuedCommand {
                    command: PrnsCommand::CloseLink(close),
                    ..
                })) if close.link_id == LinkId::new([0x81; 16])
            ));
            handle
                .snapshot_remote_control_controller_grants()
                .await
                .unwrap();
        };
        tokio::pin!(router);
        tokio::select! {
            biased;
            () = exercise => {}
            result = &mut router => panic!("router returned while its lanes remained open: {result:?}"),
        }
    }

    let expected_operator = RemoteControlControllerGrant::new(
        operator,
        RemoteControlControllerAuthority::Operator,
        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
    )
    .unwrap();
    let mut expected = [administrator, expected_operator];
    expected.sort_by_key(|grant| *grant.controller().identity_hash().as_bytes());
    assert_eq!(
        remote_control
            .controller_grants()
            .unwrap()
            .grants_in_identity_hash_order(),
        &expected
    );
    for _ in 0..2 {
        use crate::persistence::{
            read_remote_control_controller_grants_snapshot,
            remote_control_controller_grants_snapshot_capacity, FileStore, PersistedStore,
            SnapshotRegion,
        };
        let store = FileStore::new(&persistence_directory);
        let mut bytes = vec![0; remote_control_controller_grants_snapshot_capacity(2)];
        let loaded = store
            .load(SnapshotRegion::RemoteControlControllerGrants, &mut bytes)
            .unwrap()
            .unwrap();
        assert_eq!(
            read_remote_control_controller_grants_snapshot(loaded)
                .unwrap()
                .collect::<Vec<_>>(),
            expected
        );
    }
    drop(persistence_worker);
    std::fs::remove_dir_all(persistence_directory).unwrap();
}
