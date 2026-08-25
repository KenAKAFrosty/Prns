#![expect(clippy::expect_used, clippy::panic)]

use core::time::Duration;

use personal_rns::identity::PrivateIdentityMaterial;
use personal_rns::prelude::*;

const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const TARGET_ASPECTS: &[&str] = &["remote-control", "target"];
const CONTROLLER_ASPECTS: &[&str] = &["remote-control", "controller"];

struct TargetState {
    access_table: HeapRemoteControlAccessTable,
    description: RemoteControlDescription,
}

impl RemoteControlEndpointState for TargetState {
    type AccessTable = HeapRemoteControlAccessTable;

    fn remote_control_access(&self) -> &Self::AccessTable {
        &self.access_table
    }

    fn remote_control_description(&self) -> RemoteControlDescription {
        self.description
    }
}

#[tokio::main]
async fn main() {
    let target_secret = try_generate_identity_secret().expect("target identity generation failed");
    let target_destination =
        destination(target_secret, TARGET_ASPECTS, ServeMyRequestEndpoints::Yes);
    let target_destination_hash = target_destination
        .destination_hash()
        .expect("invalid target destination name");

    let controller_secret =
        try_generate_identity_secret().expect("controller identity generation failed");
    let controller_material = PrivateIdentityMaterial::from_bytes(*controller_secret);
    let controller_identity = controller_material.identity_hash();
    let controller_public_identity =
        RemoteControlIdentity::new(controller_material.public().public_keys());
    let controller_destination = destination(
        controller_secret,
        CONTROLLER_ASPECTS,
        ServeMyRequestEndpoints::No,
    );

    let mut access_table = HeapRemoteControlAccessTable::default();
    access_table
        .upsert(controller_public_identity)
        .expect("growable access table refused an identity");

    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("could not bind localhost TCP server");
    let server_address = server
        .local_addr()
        .expect("could not read server address")
        .to_string();

    let target = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [target_destination],
        app_state: TargetState {
            access_table,
            description: RemoteControlDescription::default(),
        },
        storage: GrowableHeap,
        request_endpoints: request_endpoints![RemoteControlEndpoint],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let target_handle = target.handle();
    let _server = target_handle.supervise(server);

    let (heard_sender, mut heard_receiver) = tokio::sync::mpsc::unbounded_channel();
    let controller = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [controller_destination],
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
            .supported_requests()
            .supports(RemoteControlRequestKind::Describe));
        assert!(description
            .supported_requests()
            .supports(RemoteControlRequestKind::Announce));
        println!("Authorized Describe returned {description:?} in {rtt:?}");

        let announce_rtt = controller_handle
            .remote_control(link_id)
            .announce()
            .await
            .expect("announce request did not settle");
        println!("Authorized Announce completed in {announce_rtt:?}");
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

fn destination(
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    aspects: &'static [&'static str],
    request_endpoints: ServeMyRequestEndpoints,
) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        app_name: "prns-example",
        aspects,
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        request_endpoints,
    }
}
