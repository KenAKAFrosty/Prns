#![expect(clippy::expect_used, clippy::panic)]

mod common;

use core::time::Duration;

use personal_rns::prelude::*;

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() {
    let target_identity_secrets = common::remote_control_identity_secrets(0xD0, 0xD1);
    let target_identities = target_identity_secrets.identities();
    let target_destination_hash = target_identities.target().endpoint().destination_hash();
    let controller_identity_secrets = common::remote_control_identity_secrets(0xD2, 0xD3);
    let controller_identities = controller_identity_secrets.identities();
    let controller_identity = controller_identities.controller().identity_hash();
    let controller_public_identity = *controller_identities.controller();
    let self_destination = PreConfiguredDestination::Single {
        app_name: "prns-example",
        aspects: &["node"],
        identity: Zeroizing::new([0xD4; IDENTITY_SECRET_KEY_LEN]),
        announce_app_data: b"RemoteControl example",
        proof: ProofStrategy::ProveNone,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    };
    let self_destination_hash = self_destination
        .destination_hash()
        .expect("the example destination name is valid");
    let controller_grants = [RemoteControlControllerGrant::new(
        controller_public_identity,
        RemoteControlRequestSet::all(),
    )
    .expect("the complete request set is not empty")];
    let target_remote_control = RemoteControlService::new(
        target_identity_secrets,
        RemoteControlPublicAppData::try_from(b"target".as_slice()).expect("target app data fits"),
        RemoteControlInitialAccess::Grants(
            RemoteControlControllerGrants::try_from(controller_grants.as_slice())
                .expect("one controller grant is configured"),
        ),
        RemoteControlSelfAnnouncement::Destination(self_destination_hash),
    );
    let controller_remote_control = RemoteControlService::new(
        controller_identity_secrets,
        RemoteControlPublicAppData::try_from(b"controller".as_slice())
            .expect("controller app data fits"),
        RemoteControlInitialAccess::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    );

    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("could not bind localhost TCP server");
    let server_address = server
        .local_addr()
        .expect("could not read server address")
        .to_string();

    let target = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: target_remote_control,
        pre_configured_destinations: [self_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let target_handle = target.handle();
    let _server = target_handle.supervise(server);

    let (heard_sender, mut heard_receiver) = tokio::sync::mpsc::unbounded_channel();
    let controller = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: controller_remote_control,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ignored = heard_sender.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(TcpClientInterface::new(server_address));
        },
        persistence: NoPersistence,
    });
    let controller_handle = controller.handle();

    let announcer = target_handle.clone();
    let announce_task = tokio::spawn(async move {
        loop {
            if announcer
                .announce_now(AnnounceNow {
                    destination: target_destination_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                })
                .await
                .is_err()
            {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    let exchange = async {
        loop {
            let destination = heard_receiver
                .recv()
                .await
                .expect("announce stream closed before target discovery");
            if destination == target_destination_hash {
                break;
            }
        }
        announce_task.abort();
        let link_id = controller_handle
            .establish_link(target_destination_hash)
            .await
            .expect("remote control link did not establish");
        controller_handle
            .identify(link_id, controller_identity)
            .await
            .expect("controller identity was not sent");

        let (description, rtt) = controller_handle
            .remote_control(link_id)
            .describe()
            .await
            .expect("describe request did not settle");
        assert!(description
            .available_requests()
            .supports(RemoteControlRequestKind::Describe));
        assert!(description
            .available_requests()
            .supports(RemoteControlRequestKind::AnnounceSelf));
        println!("Authorized Describe returned {description:?} in {rtt:?}");

        let announce_rtt = controller_handle
            .remote_control(link_id)
            .announce_self()
            .await
            .expect("announce-self request did not settle");
        println!("Authorized AnnounceSelf completed in {announce_rtt:?}");
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.expect("RemoteControl exchange did not complete within 10 seconds");
        }
        result = target.run() => {
            result.expect("target failed");
            panic!("target stopped before RemoteControl exchange completed");
        }
        result = controller.run() => {
            result.expect("controller failed");
            panic!("controller stopped before RemoteControl exchange completed");
        }
    }
}
