#![cfg(feature = "host-test")]
#![allow(clippy::expect_used, clippy::panic)]

use core::time::Duration;
use std::sync::Arc;

use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    RemoteControlInitialControllerGrants, RemoteControlNodeIdentitySecrets, RemoteControlRequest,
    RemoteControlRequestKind, RemoteControlRequestSet, RemoteControlSelfAnnouncement,
    RemoteControlService, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    REMOTE_CONTROL_APPLICATION_ASPECTS, REMOTE_CONTROL_APPLICATION_NAME,
    REMOTE_CONTROL_REQUEST_ENDPOINT_ID,
};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{NodePersistence, PrnsNodeApi, RemoteControlTargetAccessControl};
use prns_app::contract::DescribeRemoteControlTargetInput;
use prns_app::host_test::describe_with_handle;
use tokio::sync::Notify;

struct HeldDescribeState {
    controller: personal_rns::identity::IdentityHash,
    received: Arc<Notify>,
}

struct HeldDescribe;

impl RequestEndpoint<HeldDescribeState> for HeldDescribe {
    const ENDPOINT_ID: &'static str = REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::RequireIdentified;

    async fn handle(
        context: RequestContext<'_, HeldDescribeState>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        assert_eq!(context.requester, Some(context.state.controller));
        assert_eq!(
            RemoteControlRequest::parse(context.data),
            Ok(RemoteControlRequest::Describe)
        );
        context.state.received.notify_one();
        // Only the responder is a fixture: the application has established and
        // identified a real Link and transmitted its real Describe request.
        std::future::pending().await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_connected_describe_retires_both_tcp_links() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let temporary = tempfile::tempdir().expect("isolated cancellation fixture");
            let controller_secrets = identity_secrets(0xE1, 0xE2);
            let controller_identity = controller_secrets.identities().controller().identity_hash();
            let target_secrets = identity_secrets(0xF1, 0xF2);
            let target_identity = RemoteControlTargetIdentity::new(
                *target_secrets.identities().target().public_keys(),
            );
            let target_destination = target_identity.endpoint().destination_hash();
            let destination = PreConfiguredDestination::Single {
                app_name: REMOTE_CONTROL_APPLICATION_NAME,
                aspects: REMOTE_CONTROL_APPLICATION_ASPECTS,
                identity: Zeroizing::new([0xF2; IDENTITY_SECRET_KEY_LEN]),
                announce_app_data: b"Controlled held Describe",
                proof: ProofStrategy::ProveNone,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: Default::default(),
                request_endpoints: ServeMyRequestEndpoints::Yes,
            };
            assert_eq!(destination.destination_hash(), Ok(target_destination));
            let received = Arc::new(Notify::new());
            let target = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: RemoteControlService::Unavailable,
                pre_configured_destinations: [destination],
                app_state: HeldDescribeState {
                    controller: controller_identity,
                    received: Arc::clone(&received),
                },
                storage: GrowableHeap,
                request_endpoints: request_endpoints![HeldDescribe],
                on_event: |_event, _state| {},
                interfaces: ManuallyAttached,
                persistence: NodePersistence::custom_dir(temporary.path().join("target"))
                    .expect("target persistence opens"),
            });
            let target_handle = target.handle();
            let server = TcpServer::bind("127.0.0.1:0").await.expect("target binds");
            let address = server.local_addr().expect("target address").to_string();
            let _server = target_handle.supervise(server);

            let controller = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: RemoteControlService::new(
                    controller_secrets,
                    RemoteControlInitialControllerGrants::Nobody,
                    RemoteControlSelfAnnouncement::Unavailable,
                ),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: |_event, _state| {},
                interfaces: ManuallyAttached,
                persistence: NodePersistence::custom_dir(temporary.path().join("controller"))
                    .expect("controller persistence opens"),
            });
            let controller_handle = controller.handle();
            let _interface = controller_handle.attach(TcpClientInterface::new(address));

            let scenario = async {
                controller_handle
                    .set_remote_control_target_access(
                        RemoteControlTargetAccess::new(
                            RemoteControlTargetIdentity::new(*target_identity.public_keys()),
                            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
                        )
                        .expect("nonempty target permission"),
                    )
                    .await
                    .expect("controller target access is stored");
                while !has_online_interface(&controller_handle) || !has_online_interface(&target_handle) {
                    tokio::task::yield_now().await;
                }
                target_handle
                    .announce_now(AnnounceNow {
                        destination: target_destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    })
                    .await
                    .expect("target announces its controlled endpoint");
                while controller_handle.route(target_destination).await.is_none() {
                    tokio::task::yield_now().await;
                }
                let mut describing = Box::pin(describe_with_handle(
                    &controller_handle,
                    DescribeRemoteControlTargetInput {
                        target_identity_fingerprint: target_identity.identity_hash().as_bytes().to_vec(),
                    },
                ));
                tokio::select! {
                    () = received.notified() => {},
                    outcome = &mut describing => panic!("Describe settled while its response was held: {outcome:?}"),
                }
                assert_eq!(controller_handle.link_count().await, 1);
                assert_eq!(target_handle.link_count().await, 1);
                assert!(tokio::time::timeout(Duration::from_millis(20), &mut describing).await.is_err());

                // Dropping the real application future must send teardown, not
                // leave either established Link alive until its idle timeout.
                drop(describing);
                tokio::time::timeout(Duration::from_secs(1), async {
                    while controller_handle.link_count().await != 0 || target_handle.link_count().await != 0 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("cancellation retires the established Link at both ends");
                assert!(has_online_interface(&controller_handle));
                assert!(has_online_interface(&target_handle));
            };
            tokio::select! {
                result = tokio::time::timeout(Duration::from_secs(5), scenario) => {
                    result.expect("held Describe cancellation finishes within its bound");
                }
                result = target.run() => panic!("target stopped unexpectedly: {result:?}"),
                result = controller.run() => panic!("controller stopped unexpectedly: {result:?}"),
            }
        })
        .await;
}

fn has_online_interface(handle: &PrnsNodeHandle) -> bool {
    handle
        .interface_timing_inventory()
        .iter()
        .any(|interface| interface.connection.is_online())
}

fn identity_secrets(controller_fill: u8, target_fill: u8) -> RemoteControlNodeIdentitySecrets {
    RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [controller_fill; IDENTITY_SECRET_KEY_LEN],
        )),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [target_fill; IDENTITY_SECRET_KEY_LEN],
        )),
    )
    .expect("controller and target identities differ")
}
