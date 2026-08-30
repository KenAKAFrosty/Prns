#![expect(clippy::expect_used)]

use core::time::Duration;

use personal_rns::engine::{
    BeginRemoteControlControllerPairing, EgressTarget, OpenRemoteControlPairing,
    OpenRemoteControlPairingFailure, OpenRemoteControlPairingRejection, RemoteControlPairingOpened,
};
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::identity::IdentityPublicKeys;
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    RemoteControlPairingAttemptId, RemoteControlPairingAttemptTimeout,
    RemoteControlPairingConfirmationCode, RemoteControlPairingContext,
    RemoteControlPairingEndpoint, RemoteControlPairingExpiresAfter,
    RemoteControlPairingPermissions, RemoteControlPairingPublicAppDataBytes,
};
use personal_rns::runtime::RemoteControlPairingControlError;
use personal_rns::units::{DurationMillis, InstantMillis};

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const PAIRING_WINDOW: DurationMillis = DurationMillis(60_000);
const PAIRING_ATTEMPT_TIMEOUT: DurationMillis = DurationMillis(30_000);
const PUBLIC_APP_DATA: &[u8] = b"integration target";

struct PairingAvailability {
    endpoint: RemoteControlPairingEndpoint,
    expires_at: InstantMillis,
    public_app_data: std::vec::Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct PairingConfirmation {
    attempt_id: RemoteControlPairingAttemptId,
    context: RemoteControlPairingContext,
    code: RemoteControlPairingConfirmationCode,
    controller: RemoteControlControllerIdentity,
    target: IdentityPublicKeys,
    permissions: RemoteControlPairingPermissions,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_pairing_reaches_the_same_confirmation_on_both_nodes() {
    let target_identity_secrets = remote_control_identity_secrets(0xA1, 0xA2);
    let controller_identity_secrets = remote_control_identity_secrets(0xB1, 0xB2);
    let controller_identity = *controller_identity_secrets.identities().controller();

    let (target_confirmation_tx, mut target_confirmation_rx) =
        tokio::sync::mpsc::unbounded_channel();
    let target = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: remote_control_service(target_identity_secrets),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            let PrnsEvent::Message(Message::RemoteControlTargetPairingConfirmationRequired(
                pairing,
            )) = event
            else {
                return;
            };
            let _ignored = target_confirmation_tx.send(PairingConfirmation {
                attempt_id: pairing.attempt_id(),
                context: pairing.context(),
                code: pairing.confirmation_code(),
                controller: *pairing.controller(),
                target: *pairing.target().public_keys(),
                permissions: pairing.permissions().clone(),
            });
        },
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let target_handle = target.handle();

    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("the target TCP server binds");
    let server_address = server
        .local_addr()
        .expect("the target TCP server has an address")
        .to_string();
    let _server = target_handle.supervise(server);

    let (availability_tx, mut availability_rx) = tokio::sync::mpsc::unbounded_channel();
    let (controller_confirmation_tx, mut controller_confirmation_rx) =
        tokio::sync::mpsc::unbounded_channel();
    let controller = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: remote_control_service(controller_identity_secrets),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| match event {
            PrnsEvent::Message(Message::RemoteControlPairingAvailable(pairing)) => {
                let _ignored = availability_tx.send(PairingAvailability {
                    endpoint: pairing.endpoint(),
                    expires_at: pairing.expires_at(),
                    public_app_data: pairing.public_app_data().as_bytes().to_vec(),
                });
            }
            PrnsEvent::Message(Message::RemoteControlControllerPairingConfirmationRequired(
                pairing,
            )) => {
                let _ignored = controller_confirmation_tx.send(PairingConfirmation {
                    attempt_id: pairing.attempt_id(),
                    context: pairing.context(),
                    code: pairing.confirmation_code(),
                    controller: *pairing.controller(),
                    target: *pairing.target().public_keys(),
                    permissions: pairing.permissions().clone(),
                });
            }
            PrnsEvent::Message(_) | PrnsEvent::Diagnostic(_) => {}
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(TcpClientInterface::new(server_address));
        },
        persistence: NoPersistence,
    });
    let controller_handle = controller.handle();

    let exchange = async {
        wait_for_direct_connection(&target_handle, &controller_handle).await;

        let opened = open_pairing_when_attached(
            &target_handle,
            OpenRemoteControlPairing {
                target: EgressTarget::AllInterfaces,
                expires_after: RemoteControlPairingExpiresAfter::try_from(PAIRING_WINDOW)
                    .expect("the pairing window is valid"),
                attempt_timeout: RemoteControlPairingAttemptTimeout::try_from(
                    PAIRING_ATTEMPT_TIMEOUT,
                )
                .expect("the pairing attempt timeout is valid"),
                permissions: RemoteControlPairingPermissions::try_from(
                    RemoteControlRequestSet::all(),
                )
                .expect("the request set is not empty"),
                public_app_data: RemoteControlPairingPublicAppDataBytes::try_from(PUBLIC_APP_DATA)
                    .expect("the public app data fits"),
            },
        )
        .await
        .expect("the target opens pairing once its interface is attached");

        let availability = availability_rx
            .recv()
            .await
            .expect("the controller observes pairing availability");
        assert_eq!(availability.endpoint, opened.endpoint);
        assert_eq!(availability.public_app_data, PUBLIC_APP_DATA);

        let link_id = controller_handle
            .establish_link(availability.endpoint.destination_hash())
            .await
            .expect("the controller links to the ephemeral pairing endpoint");
        controller_handle
            .identify(link_id, controller_identity.identity_hash())
            .await
            .expect("the controller identifies on the pairing link");

        let _offered = controller_handle
            .begin_remote_control_controller_pairing(BeginRemoteControlControllerPairing {
                context: RemoteControlPairingContext::new(availability.endpoint, link_id),
                invitation_code: opened.invitation_code,
                pairing_expires_at: availability.expires_at,
            })
            .await
            .expect("the target returns a valid pairing offer");

        let target_confirmation = target_confirmation_rx
            .recv()
            .await
            .expect("the target asks for confirmation");
        let controller_confirmation = controller_confirmation_rx
            .recv()
            .await
            .expect("the controller asks for confirmation");
        assert_eq!(controller_confirmation, target_confirmation);
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.expect("pairing reaches confirmation within the bounded test window");
        }
        result = target.run() => {
            result.expect("the target node runs");
            panic!("the target stopped before pairing reached confirmation");
        }
        result = controller.run() => {
            result.expect("the controller node runs");
            panic!("the controller stopped before pairing reached confirmation");
        }
    }
}

fn remote_control_identity_secrets(
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
    .expect("the controller and target identities are distinct")
}

fn remote_control_service(
    identity_secrets: RemoteControlNodeIdentitySecrets,
) -> RemoteControlService<'static> {
    RemoteControlService::new(
        identity_secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    )
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
) -> Result<RemoteControlPairingOpened, OpenRemoteControlPairingControlError> {
    loop {
        match target.open_remote_control_pairing(open.clone()).await {
            Ok(opened) => return Ok(opened),
            Err(RemoteControlPairingControlError::Failed(
                OpenRemoteControlPairingFailure::Rejected(
                    OpenRemoteControlPairingRejection::NoTransmittingInterfaces,
                ),
            )) => tokio::task::yield_now().await,
            Err(error) => return Err(error),
        }
    }
}
