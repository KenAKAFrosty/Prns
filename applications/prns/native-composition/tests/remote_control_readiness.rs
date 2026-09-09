#![cfg(feature = "host-test")]
#![allow(clippy::expect_used, clippy::panic)]

use core::time::Duration;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerGrants,
    RemoteControlInitialControllerGrants, RemoteControlNodeIdentitySecrets,
    RemoteControlRequestSet, RemoteControlSelfAnnouncement, RemoteControlService,
    RemoteControlTargetAccess, RemoteControlTargetIdentity,
};
use personal_rns::runtime::{NodePersistence, RemoteControlTargetAccessControl};
use prns_app::contract::{DescribeRemoteControlTargetInput, RemoteControlDescribeOutcome};
use prns_app::host_test::describe_with_handle;

const FIXTURE_TIMEOUT: Duration = Duration::from_secs(18);
// Real radio recovery can exceed the former five-second readiness cutoff.
const DETACHED_OBSERVATION: Duration = Duration::from_secs(6);

#[tokio::test(flavor = "current_thread")]
async fn describe_waits_for_the_retained_route_interface_to_return() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let temporary = tempfile::tempdir().expect("isolated runtime persistence");
            let controller_secrets = identity_secrets(0xC1, 0xC2);
            let target_secrets = identity_secrets(0xD1, 0xD2);
            let target_identity = RemoteControlTargetIdentity::new(
                *target_secrets.identities().target().public_keys(),
            );
            let target_destination = target_identity.endpoint().destination_hash();
            let permissions = RemoteControlRequestSet::only(
                personal_rns::remote_control::RemoteControlRequestKind::Describe,
            );
            let grants = [RemoteControlControllerGrant::new(
                *controller_secrets.identities().controller(),
                permissions,
            )
            .expect("nonempty target grant")];
            let requests = Arc::new(AtomicUsize::new(0));
            let target_requests = Arc::clone(&requests);
            let target = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: RemoteControlService::new(
                    target_secrets,
                    RemoteControlInitialControllerGrants::Grants(
                        RemoteControlControllerGrants::try_from(grants.as_slice())
                            .expect("valid target grants"),
                    ),
                    RemoteControlSelfAnnouncement::Unavailable,
                ),
                pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: request_endpoints![],
                on_event: move |event, _state| {
                    if matches!(event, PrnsEvent::Message(Message::Request { .. })) {
                        target_requests.fetch_add(1, Ordering::AcqRel);
                    }
                },
                interfaces: ManuallyAttached,
                persistence: NodePersistence::custom_dir(temporary.path().join("target"))
                    .expect("target persistence opens"),
            });
            let target_handle = target.handle();
            let server = TcpServer::bind("127.0.0.1:0")
                .await
                .expect("controlled target binds");
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
            let attached = controller_handle.attach(TcpClientInterface::new(address.clone()));
            let interface_id = attached.id();

            let scenario = async {
                controller_handle
                    .set_remote_control_target_access(
                        RemoteControlTargetAccess::new(
                            RemoteControlTargetIdentity::new(*target_identity.public_keys()),
                            permissions,
                        )
                            .expect("nonempty controller access"),
                    )
                    .await
                    .expect("controller persists its target access");
                wait_until(|| {
                    is_online(&controller_handle, interface_id)
                        && target_handle
                            .interface_timing_inventory()
                            .iter()
                            .any(|interface| interface.connection.is_online())
                })
                .await;
                target_handle
                    .announce_now(AnnounceNow {
                        destination: target_destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    })
                    .await
                    .expect("one initial target announcement");
                wait_for_route(&controller_handle, target_destination, interface_id).await;
                let input = DescribeRemoteControlTargetInput {
                    target_identity_fingerprint: target_identity.identity_hash().as_bytes().to_vec(),
                };
                assert_described(describe_with_handle(&controller_handle, input.clone()).await);
                wait_for_no_links(&target_handle, &controller_handle).await;
                let completed_requests = requests.load(Ordering::Acquire);
                assert_eq!(completed_requests, 1, "baseline performs exactly one request");

                // Public teardown declares MayReturn: the authenticated route remains,
                // but no sender exists until this same stable interface is reattached.
                attached.teardown();
                wait_until(|| {
                    !controller_handle
                        .interface_timing_inventory()
                        .iter()
                        .any(|interface| interface.id == interface_id)
                })
                .await;
                wait_for_route(&controller_handle, target_destination, interface_id).await;

                let describing = describe_with_handle(&controller_handle, input);
                tokio::pin!(describing);
                // Keep the real interface absent while polling the operation. This is
                // a fixture barrier, not a production reconnect delay or network retry.
                assert!(tokio::time::timeout(DETACHED_OBSERVATION, &mut describing)
                    .await
                    .is_err());
                assert_eq!(requests.load(Ordering::Acquire), completed_requests);
                assert_eq!(target_handle.link_count().await, 0);
                assert!(!is_online(&controller_handle, interface_id));

                let reattached = controller_handle.attach(TcpClientInterface::new(address));
                assert_eq!(reattached.id(), interface_id);
                // No target announce and no second Describe is submitted after attach.
                let outcome = tokio::time::timeout(FIXTURE_TIMEOUT, &mut describing)
                    .await
                    .expect("the original Describe settles after interface reattachment");
                assert_described(outcome);
                assert_eq!(requests.load(Ordering::Acquire), completed_requests + 1);
                wait_for_no_links(&target_handle, &controller_handle).await;
            };

            tokio::select! {
                result = tokio::time::timeout(FIXTURE_TIMEOUT + Duration::from_secs(5), scenario) => {
                    result.expect("controlled readiness scenario completes within its bound");
                }
                result = target.run() => panic!("target stopped during readiness scenario: {result:?}"),
                result = controller.run() => panic!("controller stopped during readiness scenario: {result:?}"),
            }
        })
        .await;
}

fn is_online(handle: &PrnsNodeHandle, interface_id: personal_rns::interfaces::InterfaceId) -> bool {
    handle.interface_timing_inventory().iter().any(|interface| {
        interface.id == interface_id
            && interface.connection.is_online()
            && interface.capabilities.allows_transmit()
    })
}

async fn wait_until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("controlled interface transition completes");
}

async fn wait_for_route(
    handle: &PrnsNodeHandle,
    destination: personal_rns::wire::DestinationHash,
    interface_id: personal_rns::interfaces::InterfaceId,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(route) = handle.route(destination).await {
                assert_eq!(route.interface, interface_id);
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("target route is retained on the exact interface");
}

async fn wait_for_no_links(target: &PrnsNodeHandle, controller: &PrnsNodeHandle) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while target.link_count().await != 0 || controller.link_count().await != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("operation links are retired");
}

fn assert_described(outcome: RemoteControlDescribeOutcome) {
    assert!(
        matches!(outcome, RemoteControlDescribeOutcome::Described { .. }),
        "the original application Describe must succeed: {outcome:?}",
    );
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
    .expect("controller and target identities are distinct")
}
