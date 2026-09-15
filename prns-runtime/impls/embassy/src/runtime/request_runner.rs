use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Receiver, Sender};
use heapless::Vec as HeaplessVec;

use crate::engine::{InstantMillis, Journaled, RespondData};
use crate::identity::IdentityHash;
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::RttMillis;
use crate::wire::DestinationHash;
use prns_runtime::runtime::placement::{
    admit_remote_control_request, dispatch_admitted_remote_control_request,
};

use super::node_facade::PrnsNodeHandle;
use super::remote_control_controller_grants::{
    RemoteControlControllerGrantCommand, RemoteControlControllerGrantCompletion,
};
use super::remote_control_pairing_authorizations::RemoteControlPairingAuthorizationTransactionState;
use super::remote_control_pairing_persistence::{
    RemoteControlAuthorizationStoreExchange, RemoteControlPairingPersistenceEvents,
    RemoteControlPairingPersistenceProgress, RemoteControlPairingPersistenceRequired,
};
use super::remote_control_target_accesses::{
    RemoteControlTargetAccessCommand, RemoteControlTargetAccessCompletion,
};
use super::request_endpoints::{
    dispatch_request, Decline, InboundRequest, RequestEndpointSet, ResponseCapacityExceeded,
    ResponseSink,
};
use super::AssembledRemoteControl;

#[allow(clippy::large_enum_variant)]
enum RunnerResponse {
    Buffered(RespondData),
    StaticBytes(&'static [u8]),
    #[cfg(feature = "large-static-responses")]
    StaticFile {
        name: &'static str,
        bytes: &'static [u8],
    },
}

impl ResponseSink for RunnerResponse {
    fn put_packed(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) => body
                .extend_from_slice(bytes)
                .map_err(|()| ResponseCapacityExceeded),
            RunnerResponse::StaticBytes(_) => Err(ResponseCapacityExceeded),
            #[cfg(feature = "large-static-responses")]
            RunnerResponse::StaticFile { .. } => Err(ResponseCapacityExceeded),
        }
    }

    fn put_bytes(&mut self, bytes: &[u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) => ResponseSink::put_bytes(body, bytes),
            RunnerResponse::StaticBytes(_) => Err(ResponseCapacityExceeded),
            #[cfg(feature = "large-static-responses")]
            RunnerResponse::StaticFile { .. } => Err(ResponseCapacityExceeded),
        }
    }

    fn put_static_bytes(&mut self, bytes: &'static [u8]) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) if body.is_empty() => {
                *self = RunnerResponse::StaticBytes(bytes);
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }

    #[cfg(feature = "large-static-responses")]
    fn put_static_file(
        &mut self,
        name: &'static str,
        bytes: &'static [u8],
    ) -> Result<(), ResponseCapacityExceeded> {
        match self {
            RunnerResponse::Buffered(body) if body.is_empty() => {
                *self = RunnerResponse::StaticFile { name, bytes };
                Ok(())
            }
            _ => Err(ResponseCapacityExceeded),
        }
    }
}

pub(super) struct RunnerRequest<const N: usize> {
    destination: DestinationHash,
    link_id: LinkId,
    request_id: RequestId,
    requester: Option<IdentityHash>,
    path_hash: RequestPathHash,
    requested_at: InstantMillis,
    rtt: RttMillis,
    data: HeaplessVec<u8, N>,
}

impl<const N: usize> RunnerRequest<N> {
    pub(super) fn copy_from(journaled: &Journaled<'_>) -> Option<Self> {
        let Journaled::RequestReceived {
            destination,
            link_id,
            request_id,
            requester,
            path_hash,
            requested_at,
            rtt,
            data,
        } = journaled
        else {
            return None;
        };
        Some(Self {
            destination: *destination,
            link_id: *link_id,
            request_id: *request_id,
            requester: *requester,
            path_hash: *path_hash,
            requested_at: *requested_at,
            rtt: *rtt,
            data: HeaplessVec::from_slice(data).ok()?,
        })
    }

    pub(super) fn try_enqueue<M, const CAP: usize>(
        journaled: &Journaled<'_>,
        sender: &Sender<'_, M, Self, CAP>,
    ) where
        M: RawMutex,
    {
        let Journaled::RequestReceived { data, .. } = journaled else {
            return;
        };
        #[cfg_attr(not(feature = "log"), allow(unused_variables))]
        let inbound_len = data.len();
        let Some(request) = Self::copy_from(journaled) else {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop copy_failed bytes={inbound_len}"
            );
            return;
        };
        if sender.try_send(request).is_err() {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop queue_full bytes={inbound_len}"
            );
        } else {
            #[cfg(feature = "log")]
            if let Journaled::RequestReceived {
                destination,
                link_id,
                requester,
                path_hash,
                data,
                ..
            } = journaled
            {
                log::info!(
                    target: "personal_hopspot_esp32",
                    "rc: enqueue dest={} link={} path={} ctrl={} kind={:?} bytes={}",
                    hex4(destination.as_bytes()),
                    hex4(link_id.as_bytes()),
                    hex4(path_hash.as_bytes()),
                    hash4(*requester),
                    data.get(1).copied(),
                    data.len()
                );
            }
        }
    }
}

pub(super) fn trace_journaled(journaled: &Journaled<'_>) {
    #[cfg(feature = "log")]
    match journaled {
        Journaled::LinkEstablished(established) => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: link up id={} rtt={}",
                hex4(established.link_id.as_bytes()),
                established.rtt_millis
            );
        }
        Journaled::PeerIdentified { link_id, identity } => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: identified link={} peer={}",
                hex4(link_id.as_bytes()),
                hex4(identity.as_bytes())
            );
        }
        Journaled::RequestReceived {
            destination,
            link_id,
            requester,
            path_hash,
            data,
            ..
        } => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: request dest={} link={} path={} ctrl={} kind={:?} bytes={}",
                hex4(destination.as_bytes()),
                hex4(link_id.as_bytes()),
                hex4(path_hash.as_bytes()),
                hash4(*requester),
                data.get(1).copied(),
                data.len()
            );
        }
        Journaled::LinkClosed { link_id, reason } => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: link close id={} reason={reason:?}",
                hex4(link_id.as_bytes())
            );
        }
        Journaled::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => {
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: link mismatch id={} attached={} arrived={}",
                hex4(link_id.as_bytes()),
                hex4(attached_interface.as_bytes()),
                hex4(arrived_on.as_bytes())
            );
        }
        _ => {}
    }
    #[cfg(not(feature = "log"))]
    let _ = journaled;
}

pub(super) async fn run_router<
    St,
    R,
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
    const REQUESTS: usize,
    const REQUEST_BYTES: usize,
>(
    state: &St,
    remote_control: &mut AssembledRemoteControl,
    requests: Receiver<'_, M, RunnerRequest<REQUEST_BYTES>, REQUESTS>,
    pairing_events: &RemoteControlPairingPersistenceEvents<M>,
    authorization_stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
) where
    R: RequestEndpointSet<St>,
    M: RawMutex,
    St: prns_runtime::runtime::RemoteControlHostControls,
{
    let mut authorization_transaction = RemoteControlPairingAuthorizationTransactionState::new();
    let mut pairing_persistence = RemoteControlPairingPersistenceProgress::new();
    #[cfg(feature = "log")]
    log::info!(target: "personal_hopspot_esp32", "rc: runner up");
    loop {
        match select4(
            next_pairing_persistence_input(
                &pairing_persistence,
                pairing_events,
                authorization_stores,
            ),
            commands.next_remote_control_controller_grant_command(),
            commands.next_remote_control_target_access_command(),
            requests.receive(),
        )
        .await
        {
            Either4::First(input) => {
                let result = match input {
                    RemoteControlPairingPersistenceInput::Required(required) => {
                        pairing_persistence
                            .accept_required(
                                required,
                                remote_control,
                                &mut authorization_transaction,
                                authorization_stores,
                                commands,
                            )
                            .await
                    }
                    RemoteControlPairingPersistenceInput::StoreCompleted(stored) => {
                        let Some(stores) = authorization_stores else {
                            continue;
                        };
                        pairing_persistence
                            .accept_store_completion(
                                stored,
                                remote_control,
                                &mut authorization_transaction,
                                stores,
                                commands,
                            )
                            .await
                    }
                };
                if let (Err(failure), Some(stores)) = (result, authorization_stores) {
                    stores.report_failure(failure).await;
                }
            }
            Either4::Second(RemoteControlControllerGrantCommand::SetControllerGrant {
                id,
                grant,
            }) => {
                let outcome = if authorization_transaction.is_active() {
                    Err(super::SetRemoteControlControllerGrantServiceError::TransactionInProgress)
                } else {
                    remote_control.set_controller_grant(grant)
                };
                let _settled = commands.settle_remote_control_controller_grant(
                    id,
                    RemoteControlControllerGrantCompletion::ControllerGrantSet(outcome),
                );
            }
            Either4::Second(RemoteControlControllerGrantCommand::RevokeController {
                id,
                controller,
            }) => {
                let outcome = if authorization_transaction.is_active() {
                    Err(super::RevokeRemoteControlControllerServiceError::TransactionInProgress)
                } else {
                    remote_control.revoke_controller(&controller)
                };
                let _settled = commands.settle_remote_control_controller_grant(
                    id,
                    RemoteControlControllerGrantCompletion::ControllerRevoked(outcome),
                );
            }
            Either4::Third(RemoteControlTargetAccessCommand::Inventory { id }) => {
                let outcome = if authorization_transaction.is_active() {
                    Err(super::RemoteControlTargetInventoryServiceError::TransactionInProgress)
                } else {
                    remote_control.target_inventory()
                };
                let _settled = commands.settle_remote_control_target_access(
                    id,
                    RemoteControlTargetAccessCompletion::Inventory(outcome),
                );
            }
            Either4::Third(RemoteControlTargetAccessCommand::ResolveTarget { id, target }) => {
                let outcome = if authorization_transaction.is_active() {
                    Err(super::ResolveRemoteControlTargetServiceError::TransactionInProgress)
                } else {
                    remote_control.resolve_target(&target)
                };
                let _settled = commands.settle_remote_control_target_access(
                    id,
                    RemoteControlTargetAccessCompletion::Resolved(outcome),
                );
            }
            Either4::Third(RemoteControlTargetAccessCommand::SetTargetAccess { id, access }) => {
                let outcome = if authorization_transaction.is_active() {
                    Err(super::SetRemoteControlTargetAccessServiceError::TransactionInProgress)
                } else {
                    remote_control.set_target_access(access)
                };
                let _settled = commands.settle_remote_control_target_access(
                    id,
                    RemoteControlTargetAccessCompletion::AccessSet(outcome),
                );
            }
            Either4::Third(RemoteControlTargetAccessCommand::ForgetTarget { id, target }) => {
                let outcome = if authorization_transaction.is_active() {
                    Err(super::ForgetRemoteControlTargetServiceError::TransactionInProgress)
                } else {
                    remote_control.forget_target_by_hash(target)
                };
                let _settled = commands.settle_remote_control_target_access(
                    id,
                    RemoteControlTargetAccessCompletion::Forgotten(outcome),
                );
            }
            Either4::Fourth(request) => {
                #[cfg(feature = "log")]
                log::info!(
                    target: "personal_hopspot_esp32",
                    "rc: dequeue kind={:?} bytes={} link={}",
                    request.data.get(1).copied(),
                    request.data.len(),
                    hex4(request.link_id.as_bytes())
                );
                dispatch::<
                    St,
                    R,
                    M,
                    COMMANDS,
                    COMPLETIONS,
                    REQUEST_COMPLETIONS,
                    RESPONSE_BYTES,
                    REQUEST_BYTES,
                >(state, remote_control, commands, request)
                .await;
            }
        }
    }
}

enum RemoteControlPairingPersistenceInput {
    Required(RemoteControlPairingPersistenceRequired),
    StoreCompleted(Result<(), super::embedded_persistence::EmbeddedPersistenceFailure>),
}

#[inline(never)]
async fn next_pairing_persistence_input<M: RawMutex>(
    progress: &RemoteControlPairingPersistenceProgress,
    events: &RemoteControlPairingPersistenceEvents<M>,
    stores: Option<&RemoteControlAuthorizationStoreExchange<M>>,
) -> RemoteControlPairingPersistenceInput {
    if progress.is_ready() {
        return RemoteControlPairingPersistenceInput::Required(events.receive().await);
    }
    if progress.is_waiting_for_store() {
        if let Some(stores) = stores {
            return RemoteControlPairingPersistenceInput::StoreCompleted(
                stores.next_completion().await,
            );
        }
    }
    core::future::pending().await
}

#[inline(never)]
async fn dispatch<
    St,
    R,
    M,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
    const REQUEST_BYTES: usize,
>(
    state: &St,
    remote_control: &mut AssembledRemoteControl,
    commands: PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    request: RunnerRequest<REQUEST_BYTES>,
) where
    R: RequestEndpointSet<St>,
    M: RawMutex,
    St: prns_runtime::runtime::RemoteControlHostControls,
{
    let inbound = InboundRequest::new(
        request.destination,
        request.link_id,
        request.request_id,
        request.requester,
        request.requested_at,
        request.rtt,
        &request.data,
    );
    let responder = inbound.respond_token();
    let mut body = RunnerResponse::Buffered(RespondData::new());
    #[cfg_attr(not(feature = "log"), allow(unused_variables))]
    let kind = request.data.get(1).copied();
    let dispatched = if let Some((controller_grants, available_requests, self_announcement)) =
        remote_control.request_configuration_mut(request.destination, request.path_hash)
    {
        match admit_remote_control_request(
            controller_grants,
            available_requests,
            self_announcement,
            &inbound,
        ) {
            Ok(admission) => {
                #[cfg(feature = "log")]
                log::info!(
                    target: "personal_hopspot_esp32",
                    "rc: admit ok kind={kind:?} ctrl={} bytes={}",
                    hash4(request.requester),
                    request.data.len()
                );
                dispatch_admitted_remote_control_request(
                    state, &commands, inbound, &mut body, admission,
                )
                .await
            }
            Err(reason) => {
                #[cfg(feature = "log")]
                log::info!(
                    target: "personal_hopspot_esp32",
                    "rc: admit {reason:?} kind={kind:?} ctrl={} bytes={}",
                    hash4(request.requester),
                    request.data.len()
                );
                Err(reason.into())
            }
        }
    } else {
        #[cfg(feature = "log")]
        log::info!(
            target: "personal_hopspot_esp32",
            "rc: not_control_dest kind={kind:?} ctrl={} bytes={}",
            hash4(request.requester),
            request.data.len()
        );
        dispatch_request::<St, R>(state, &commands, request.path_hash, inbound, &mut body).await
    };
    match dispatched {
        Ok(()) => {
            #[cfg_attr(not(feature = "log"), allow(unused_variables))]
            let reply_len = match &body {
                RunnerResponse::Buffered(body) => body.len(),
                RunnerResponse::StaticBytes(bytes) => bytes.len(),
                #[cfg(feature = "large-static-responses")]
                RunnerResponse::StaticFile { bytes, .. } => bytes.len(),
            };
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: reply queued kind={kind:?} bytes={reply_len}"
            );
            match body {
                RunnerResponse::Buffered(body) => {
                    commands.respond_owned_packed(responder, body);
                }
                RunnerResponse::StaticBytes(bytes) => {
                    commands.respond_static_bytes(responder, bytes);
                }
                #[cfg(feature = "large-static-responses")]
                RunnerResponse::StaticFile { name, bytes } => {
                    commands.respond_static_file(responder, name, bytes);
                }
            }
        }
        Err(Decline::Ignore) => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop Ignore kind={kind:?} ctrl={}",
                hash4(request.requester)
            );
        }
        Err(Decline::ResponseTooLarge) => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop ResponseTooLarge kind={kind:?} ctrl={}",
                hash4(request.requester)
            );
        }
        Err(Decline::CloseLink) => {
            #[cfg(feature = "log")]
            log::info!(
                target: "personal_hopspot_esp32",
                "rc: drop CloseLink kind={kind:?} ctrl={}",
                hash4(request.requester)
            );
            commands.close_link(responder.link_id);
        }
    }
}

#[cfg(feature = "log")]
fn hex4(bytes: &[u8]) -> Hex4<'_> {
    Hex4(bytes)
}

#[cfg(feature = "log")]
struct Hex4<'a>(&'a [u8]);

#[cfg(feature = "log")]
impl core::fmt::Display for Hex4<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            [a, b, c, d, ..] => write!(formatter, "{a:02x}{b:02x}{c:02x}{d:02x}"),
            _ => formatter.write_str("----"),
        }
    }
}

#[cfg(feature = "log")]
fn hash4(identity: Option<IdentityHash>) -> Hash4 {
    Hash4(identity)
}

#[cfg(feature = "log")]
struct Hash4(Option<IdentityHash>);

#[cfg(feature = "log")]
impl core::fmt::Display for Hash4 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            None => formatter.write_str("none"),
            Some(identity) => {
                let bytes = identity.as_bytes();
                write!(
                    formatter,
                    "{:02x}{:02x}{:02x}{:02x}",
                    bytes[0], bytes[1], bytes[2], bytes[3]
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineState, PrnsCommand};
    use crate::runtime::request_endpoints::{
        RequestContext, RequestEndpoint, RequestEndpointPolicy,
    };
    use embassy_futures::select::{select, Either};
    use embassy_futures::{block_on, join::join};
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use prns_core::storage::GrowableHeap;

    fn remote_control() -> AssembledRemoteControl {
        let mut engine = EngineState::<GrowableHeap>::default();
        crate::runtime::configure_remote_control_service(
            &mut engine,
            super::super::node_facade::test_remote_control_service(),
        )
        .expect("RemoteControl fits growable storage")
    }

    struct DestinationEcho;
    struct DestinationRoutes;

    impl RequestEndpoint<()> for DestinationEcho {
        const ENDPOINT_ID: &'static str = "/destination";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

        async fn handle(
            mut context: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
        ) -> Result<(), Decline> {
            let destination = context.destination;
            context.respond(destination.as_bytes())
        }
    }

    impl RequestEndpointSet<()> for DestinationRoutes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
            &[(DestinationEcho::ENDPOINT_ID, DestinationEcho::POLICY)];

        async fn dispatch(
            context: RequestContext<'_, ()>,
            node: &impl crate::runtime::PrnsNodeApi,
            path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            if path_hash == RequestPathHash::of(DestinationEcho::ENDPOINT_ID) {
                DestinationEcho::handle(context, node).await
            } else {
                Err(Decline::Ignore)
            }
        }
    }

    struct StaticPage;
    struct StaticRoutes;
    static PAGE: [u8; 1200] = [0x21; 1200];

    #[cfg(feature = "large-static-responses")]
    #[test]
    fn static_file_sink_preserves_filename_and_borrowed_bytes() {
        let mut response = RunnerResponse::Buffered(RespondData::new());
        ResponseSink::put_static_file(&mut response, "source.zip", &PAGE).unwrap();
        let RunnerResponse::StaticFile { name, bytes } = response else {
            panic!("static file response");
        };
        assert_eq!(name, "source.zip");
        assert_eq!(bytes.as_ptr(), PAGE.as_ptr());
    }

    impl RequestEndpoint<()> for StaticPage {
        const ENDPOINT_ID: &'static str = "/page";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

        async fn handle(
            mut context: RequestContext<'_, ()>,
            _node: &impl crate::runtime::PrnsNodeApi,
        ) -> Result<(), Decline> {
            context.respond_static_messagepack_bytes(&PAGE)
        }
    }

    impl RequestEndpointSet<()> for StaticRoutes {
        const REGISTRATIONS: &'static [(&'static str, RequestEndpointPolicy)] =
            &[(StaticPage::ENDPOINT_ID, StaticPage::POLICY)];

        async fn dispatch(
            context: RequestContext<'_, ()>,
            node: &impl crate::runtime::PrnsNodeApi,
            path_hash: RequestPathHash,
        ) -> Result<(), Decline> {
            if path_hash == RequestPathHash::of(StaticPage::ENDPOINT_ID) {
                StaticPage::handle(context, node).await
            } else {
                Err(Decline::Ignore)
            }
        }
    }

    #[test]
    fn dispatch_hands_a_borrowed_body_to_the_borrowed_lane() {
        type M = CriticalSectionRawMutex;
        let channel = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 1>::new();
        let handle = PrnsNodeHandle::new(channel.sender(), &completions);
        let mut remote_control = remote_control();
        let request = RunnerRequest {
            destination: DestinationHash::new([0x5A; 16]),
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            requester: None,
            path_hash: RequestPathHash::of("/page"),
            requested_at: InstantMillis(3),
            rtt: RttMillis::new(4),
            data: HeaplessVec::<u8, 16>::new(),
        };

        block_on(dispatch::<(), StaticRoutes, M, 1, 1, 0, 0, 16>(
            &(),
            &mut remote_control,
            handle,
            request,
        ));

        let Ok(issued) = channel.try_receive() else {
            panic!("response command");
        };
        let PrnsCommand::Respond(response) = issued.command else {
            panic!("respond command");
        };
        assert_eq!(response.link_id, LinkId::new([1; 16]));
        assert_eq!(response.request_id, RequestId([2; 16]));
        let crate::engine::RespondPayload::StaticBytes(data) = response.payload else {
            panic!("static response");
        };
        assert_eq!(data.as_ptr(), PAGE.as_ptr());
        assert_eq!(data.len(), PAGE.len());
    }

    #[test]
    fn dispatch_answers_through_the_embassy_command_lane() {
        type M = CriticalSectionRawMutex;
        let channel = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 1>::new();
        let handle = PrnsNodeHandle::new(channel.sender(), &completions);
        let mut remote_control = remote_control();
        let destination = DestinationHash::new([0x5a; 16]);
        let request = RunnerRequest {
            destination,
            link_id: LinkId::new([1; 16]),
            request_id: RequestId([2; 16]),
            requester: None,
            path_hash: RequestPathHash::of("/destination"),
            requested_at: InstantMillis(3),
            rtt: RttMillis::new(4),
            data: HeaplessVec::<u8, 16>::new(),
        };

        block_on(dispatch::<(), DestinationRoutes, M, 1, 1, 0, 0, 16>(
            &(),
            &mut remote_control,
            handle,
            request,
        ));

        let Ok(issued) = channel.try_receive() else {
            panic!("response command");
        };
        let PrnsCommand::Respond(response) = issued.command else {
            panic!("respond command");
        };
        let crate::engine::RespondPayload::Packed(data) = response.payload else {
            panic!("packed response");
        };
        assert_eq!(data.as_slice(), destination.as_bytes());
    }

    #[test]
    fn unavailable_remote_control_rejects_controller_grant_changes() {
        use crate::remote_control::RemoteControlRequestKind;
        use crate::runtime::{
            RemoteControlControllerGrantControl, RevokeRemoteControlControllerControlError,
            SetRemoteControlControllerGrantControlError,
        };

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let mut engine = EngineState::<GrowableHeap>::default();
        let mut remote_control = crate::runtime::configure_remote_control_service(
            &mut engine,
            crate::remote_control::RemoteControlService::Unavailable,
        )
        .expect("unavailable RemoteControl requires no storage");
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let router = run_router::<(), (), M, 1, 0, 0, 0, 1, 16>(
            &(),
            &mut remote_control,
            requests.receiver(),
            &pairing_events,
            None,
            handle,
        );
        let exercise = async {
            assert_eq!(
                handle.set_remote_control_controller_grant(grant).await,
                Err(SetRemoteControlControllerGrantControlError::Unavailable),
            );
            assert_eq!(
                handle
                    .revoke_remote_control_controller(*grant.controller())
                    .await,
                Err(RevokeRemoteControlControllerControlError::Unavailable),
            );
        };

        match block_on(select(exercise, router)) {
            Either::First(()) => {}
            Either::Second(()) => panic!("router returned"),
        }
    }

    #[test]
    fn router_applies_ready_remote_control_controller_grants_before_a_ready_request() {
        use crate::remote_control::{
            RemoteControlControllerGrantTable, RemoteControlDescription, RemoteControlRequest,
            RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlResponse,
            RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
        };
        use crate::runtime::RemoteControlControllerGrantControl;

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let mut remote_control = remote_control();
        let destination = remote_control.target_endpoint().unwrap().destination_hash();
        let path_hash = remote_control.request_endpoint_id().unwrap();
        let grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let mut request_bytes = [0; RemoteControlRequest::MAX_ENCODED_LEN];
        let request_len = RemoteControlRequest::Describe
            .write_into(&mut request_bytes)
            .unwrap();
        let request = RunnerRequest {
            destination,
            link_id: LinkId::new([0x71; 16]),
            request_id: RequestId([0x72; 16]),
            requester: Some(grant.controller().identity_hash()),
            path_hash,
            requested_at: InstantMillis(73),
            rtt: RttMillis::new(74),
            data: HeaplessVec::from_slice(&request_bytes[..request_len]).unwrap(),
        };
        assert!(requests.try_send(request).is_ok());
        let router = run_router::<(), DestinationRoutes, M, 1, 0, 0, 0, 1, 16>(
            &(),
            &mut remote_control,
            requests.receiver(),
            &pairing_events,
            None,
            handle,
        );
        let exercise = async {
            assert_eq!(
                handle.set_remote_control_controller_grant(grant).await,
                Ok(SetRemoteControlControllerGrantOutcome::Added),
            );
            let issued = commands.receiver().receive().await;
            let PrnsCommand::Respond(response) = issued.command else {
                panic!("RemoteControl response command")
            };
            let crate::engine::RespondPayload::Packed(data) = response.payload else {
                panic!("packed RemoteControl response")
            };
            let expected = RemoteControlDescription::try_from(
                RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
                    .with_current_operator_edits(),
            )
            .unwrap();
            assert_eq!(
                RemoteControlResponse::parse(data.as_slice()),
                Ok(RemoteControlResponse::Describe(expected)),
            );
            assert_eq!(
                handle
                    .revoke_remote_control_controller(*grant.controller())
                    .await,
                Ok(RevokeRemoteControlControllerOutcome::Revoked { grant }),
            );
        };

        match block_on(select(exercise, router)) {
            Either::First(()) => {}
            Either::Second(()) => panic!("router returned"),
        }
        assert!(remote_control.controller_grants().unwrap().is_empty());
    }

    #[test]
    fn router_prioritizes_pairing_transactions_and_rejects_racing_app_mutations() {
        use crate::remote_control::{RemoteControlControllerGrantTable, RemoteControlRequestKind};
        use crate::runtime::{
            RemoteControlControllerGrantControl, RemoteControlTargetAccessControl,
            RemoteControlTargetInventoryControlError, ResolveRemoteControlTargetControlError,
            SetRemoteControlControllerGrantControlError,
        };

        type M = CriticalSectionRawMutex;
        let commands = Channel::<M, crate::engine::IssuedCommand, 1>::new();
        let completions = crate::runtime::CompletionPool::<M, 0>::new();
        let handle = PrnsNodeHandle::new(commands.sender(), &completions);
        let requests = Channel::<M, RunnerRequest<16>, 1>::new();
        let pairing_events = RemoteControlPairingPersistenceEvents::new();
        let authorization_stores = RemoteControlAuthorizationStoreExchange::new();
        let mut remote_control = remote_control();
        let attempt_id = super::super::node_facade::test_remote_control_pairing_attempt(0x77);
        let pairing_grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::Describe,
        );
        let app_grant = super::super::node_facade::test_remote_control_grant(
            RemoteControlRequestKind::AnnounceSelf,
        );
        pairing_events.signal(RemoteControlPairingPersistenceRequired::ControllerGrant {
            attempt_id,
            grant: pairing_grant,
        });
        let router = run_router::<(), (), M, 1, 0, 0, 0, 1, 16>(
            &(),
            &mut remote_control,
            requests.receiver(),
            &pairing_events,
            Some(&authorization_stores),
            handle,
        );
        let exercise = async {
            let (app_mutation, (target_inventory, target_resolution)) = join(
                handle.set_remote_control_controller_grant(app_grant),
                join(
                    handle.remote_control_target_inventory(),
                    handle.resolve_remote_control_target(crate::identity::IdentityHash::new(
                        [0x78; 16],
                    )),
                ),
            )
            .await;
            assert_eq!(
                app_mutation,
                Err(SetRemoteControlControllerGrantControlError::Busy),
            );
            assert_eq!(
                target_inventory,
                Err(RemoteControlTargetInventoryControlError::Busy),
            );
            assert_eq!(
                target_resolution,
                Err(ResolveRemoteControlTargetControlError::Busy),
            );
        };

        match block_on(select(exercise, router)) {
            Either::First(()) => {}
            Either::Second(()) => panic!("router returned"),
        }
        assert_eq!(
            remote_control
                .controller_grants()
                .unwrap()
                .grants_in_identity_hash_order(),
            &[pairing_grant],
        );
    }
}
