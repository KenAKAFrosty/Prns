use super::*;
use crate::{run, Command, Options};
use clap::Parser;
use personal_rns::identity::{PrivateIdentityMaterial, IDENTITY_SECRET_KEY_LEN};
use personal_rns::persistence::{
    remote_control_target_accesses_snapshot_capacity, FileStore, PersistedStore, SnapshotRegion,
};
use personal_rns::remote_control::{RemoteControlControllerIdentity, RemoteControlRequest};
use personal_rns::runtime::{RemoteControlIdentityDirectory, SaveRemoteControlTargetAccessError};

fn controller(state: &Path) -> RemoteControlControllerIdentity {
    *RemoteControlIdentityDirectory::new(state.join("identity"))
        .load_or_generate()
        .unwrap()
        .into_parts()
        .0
        .identities()
        .controller()
}

fn access(request: RemoteControlRequestKind) -> RemoteControlTargetAccess {
    let material = PrivateIdentityMaterial::from_bytes([0x61; IDENTITY_SECRET_KEY_LEN]);
    RemoteControlTargetAccess::new(
        RemoteControlTargetIdentity::new(material.public().public_keys()),
        RemoteControlControllerAuthority::Operator,
        RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

fn stored(state: &Path) -> Vec<RemoteControlTargetAccess> {
    let store = FileStore::new(state.join("state"));
    let mut bytes = vec![0; remote_control_target_accesses_snapshot_capacity(8)];
    store
        .load(SnapshotRegion::RemoteControlTargetAccesses, &mut bytes)
        .unwrap()
        .map(|bytes| {
            read_remote_control_target_accesses_snapshot(bytes)
                .unwrap()
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn cli_import_restarts_with_same_identity_and_never_silently_replaces_access() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("controller");
    let recipient = controller(&state);
    let path = directory.path().join("target.card");
    let card =
        RemoteControlTargetAccessCard::new(recipient, access(RemoteControlRequestKind::Describe))
            .unwrap();
    std::fs::write(&path, target_card_text(&card).unwrap()).unwrap();
    let import = |trusted_source| Options {
        state_dir: state.clone(),
        command: Command::ImportTarget {
            card: path.clone(),
            trusted_source,
        },
    };
    assert!(matches!(run(import(false)).await, Err(Error::Setup(_))));
    assert!(stored(&state).is_empty());
    run(import(true)).await.unwrap();
    assert_eq!(controller(&state), recipient);
    assert_eq!(
        stored(&state),
        vec![access(RemoteControlRequestKind::Describe)]
    );
    run(Options {
        state_dir: state.clone(),
        command: Command::Targets,
    })
    .await
    .unwrap();
    run(import(true)).await.unwrap();

    let changed = RemoteControlTargetAccessCard::new(
        recipient,
        access(RemoteControlRequestKind::AnnounceSelf),
    )
    .unwrap();
    std::fs::write(&path, target_card_text(&changed).unwrap()).unwrap();
    assert!(matches!(
        run(import(true)).await,
        Err(Error::Save(SaveRemoteControlTargetAccessError::Conflict))
    ));
    assert_eq!(
        stored(&state),
        vec![access(RemoteControlRequestKind::Describe)]
    );

    let foreign = RemoteControlControllerIdentity::new(
        PrivateIdentityMaterial::from_bytes([0x62; IDENTITY_SECRET_KEY_LEN])
            .public()
            .public_keys(),
    );
    let card =
        RemoteControlTargetAccessCard::new(foreign, access(RemoteControlRequestKind::Describe))
            .unwrap();
    std::fs::write(&path, target_card_text(&card).unwrap()).unwrap();
    assert!(matches!(
        run(import(true)).await,
        Err(Error::Save(
            SaveRemoteControlTargetAccessError::RecipientMismatch
        ))
    ));
    assert_eq!(
        stored(&state),
        vec![access(RemoteControlRequestKind::Describe)]
    );
}

#[test]
fn delegation_profiles_encode_as_valid_operator_grants() {
    let recipient = RemoteControlControllerIdentity::new(
        PrivateIdentityMaterial::from_bytes([0x62; IDENTITY_SECRET_KEY_LEN])
            .public()
            .public_keys(),
    );
    for profile in [OperatorProfile::Inspect, OperatorProfile::Operate] {
        let requests = profile.requests();
        assert!(!requests
            .iter()
            .any(RemoteControlRequestKind::requires_administrator));
        let request = RemoteControlRequest::AuthorizeController {
            controller: recipient,
            permitted_requests: requests,
        };
        let mut encoded = [0; RemoteControlRequest::MAX_ENCODED_LEN];
        let len = request.write_into(&mut encoded).unwrap();
        assert_eq!(
            RemoteControlRequest::parse(&encoded[..len]).unwrap(),
            request
        );
    }
}

#[test]
fn cli_requires_exactly_one_target_selector_and_bounds_card_files() {
    let prefix = [
        "controller",
        "--state-dir",
        "/unused",
        "invoke",
        "--tcp",
        "127.0.0.1:4242",
        "--action",
        "describe",
    ];
    assert!(Options::try_parse_from(prefix).is_err());
    let hash = "01".repeat(16);
    let key = "02".repeat(64);
    assert!(Options::try_parse_from(prefix.into_iter().chain(["--target-id", &hash])).is_ok());
    assert!(Options::try_parse_from(prefix.into_iter().chain([
        "--target-key",
        &key,
        "--target-id",
        &hash
    ]))
    .is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large.card");
    std::fs::write(
        &path,
        vec![b'x'; RemoteControlTargetAccessCard::MAX_TEXT_LEN + 3],
    )
    .unwrap();
    assert!(read_card(&path, RemoteControlTargetAccessCard::MAX_TEXT_LEN).is_err());
}

#[tokio::test]
async fn delegated_card_is_imported_durably_by_an_independent_controller_and_controls_target() {
    use personal_rns::remote_control::{RemoteControlCapabilities, RemoteControlControllerGrant};
    use personal_rns::runtime::{NodePersistence, RemoteControlIdentityDirectory};
    use personal_rns::tcp::TcpServer;

    let directory = tempfile::tempdir().unwrap();
    let admin_path = directory.path().join("admin");
    let recipient_path = directory.path().join("recipient");
    let target_path = directory.path().join("target");
    let identities = |path: &Path| {
        RemoteControlIdentityDirectory::new(path.join("identity"))
            .load_or_generate()
            .unwrap()
            .into_parts()
            .0
    };
    let admin_secrets = identities(&admin_path);
    let admin_identity = *admin_secrets.identities().controller();
    let recipient_secrets = identities(&recipient_path);
    let recipient_identity = *recipient_secrets.identities().controller();
    assert_ne!(admin_identity, recipient_identity);
    let target_secrets = identities(&target_path);
    let target_keys = *target_secrets.identities().target().public_keys();
    let target_id = RemoteControlTargetIdentity::new(target_keys).identity_hash();
    let grants = [RemoteControlControllerGrant::new(
        admin_identity,
        RemoteControlControllerAuthority::Administrator,
        RemoteControlRequestSet::all(),
    )
    .unwrap()];
    let target_node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: RemoteControlService::with_capabilities(
            target_secrets,
            RemoteControlInitialControllerGrants::Grants(
                RemoteControlControllerGrants::try_from(grants.as_slice()).unwrap(),
            ),
            RemoteControlSelfAnnouncement::Unavailable,
            RemoteControlCapabilities::describe_only()
                .with_request(RemoteControlRequestKind::AuthorizeController),
        )
        .into(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: personal_rns::runtime::NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NodePersistence::custom_dir(target_path.join("state")).unwrap(),
        on_event: |_, _: &personal_rns::runtime::NoRemoteControlHostControls| {},
    });
    let server = TcpServer::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let _server = target_node.handle().supervise(server);
    let make_controller = |secrets, path: &Path| {
        PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            remote_control: RemoteControlService::new(
                secrets,
                RemoteControlInitialControllerGrants::Nobody,
                RemoteControlSelfAnnouncement::Unavailable,
            )
            .into(),
            pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
            app_state: personal_rns::runtime::NoRemoteControlHostControls,
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            interfaces: ManuallyAttached,
            persistence: NodePersistence::custom_dir(path.join("state")).unwrap(),
            on_event: |_, _: &personal_rns::runtime::NoRemoteControlHostControls| {},
        })
    };
    let admin_node = make_controller(admin_secrets, &admin_path);
    let admin = admin_node.handle();
    let recipient_node = make_controller(recipient_secrets, &recipient_path);
    let recipient = recipient_node.handle();
    let recipient_card_file = directory.path().join("recipient.card");
    std::fs::write(
        &recipient_card_file,
        controller_card_text(&RemoteControlControllerCard::new(recipient_identity).unwrap())
            .unwrap(),
    )
    .unwrap();
    let returned_card_file = directory.path().join("returned.card");
    let exercise = async {
        let administrator_access = RemoteControlTargetAccess::new(
            RemoteControlTargetIdentity::new(target_keys),
            RemoteControlControllerAuthority::Administrator,
            RemoteControlRequestSet::all(),
        )
        .unwrap();
        admin
            .save_remote_control_target_access_card(
                RemoteControlTargetAccessCard::new(admin_identity, administrator_access).unwrap(),
            )
            .await
            .unwrap();
        let stage = Cell::new("delegation");
        let returned = delegate_card(
            &admin,
            address,
            target_id,
            &recipient_card_file,
            true,
            OperatorProfile::Operate,
            &stage,
        )
        .await
        .unwrap();
        assert_eq!(*returned.recipient(), recipient_identity);
        assert_eq!(
            returned.access().authority(),
            RemoteControlControllerAuthority::Operator
        );
        assert_eq!(
            *returned.access().permitted_requests(),
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
        );
        std::fs::write(&returned_card_file, target_card_text(&returned).unwrap()).unwrap();
        import_target(&recipient, &returned_card_file, true)
            .await
            .unwrap();
        let interface = TcpClientInterface::new(address.to_string());
        let status = interface.status();
        recipient.add_interface(interface);
        while status.connection() != ConnectionState::Connected {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        recipient
            .request_path(
                RemoteControlTargetIdentity::new(target_keys)
                    .endpoint()
                    .destination_hash(),
            )
            .await
            .unwrap();
        let connection = recipient
            .connect_remote_control_target(target_id)
            .await
            .unwrap();
        let (description, _) = connection.describe().await.unwrap();
        assert_eq!(
            description.available_requests(),
            &RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
        );
        assert!(matches!(
            connection
                .authorize_controller(
                    admin_identity,
                    RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
                )
                .await,
            Err(
                personal_rns::runtime::RemoteControlTargetOperationError::NotPermitted(
                    RemoteControlRequestKind::AuthorizeController
                )
            )
        ));
        connection.close();
    };
    tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(20), exercise) => result.unwrap(),
        result = admin_node.run() => panic!("admin stopped: {result:?}"),
        result = recipient_node.run() => panic!("recipient stopped: {result:?}"),
        result = target_node.run() => panic!("target stopped: {result:?}"),
    }
    let restored = make_controller(identities(&recipient_path), &recipient_path);
    let handle = restored.handle();
    tokio::select! {
        resolved = handle.resolve_remote_control_target(target_id) => {
            let resolved = resolved.unwrap();
            assert_eq!(resolved.controller(), &recipient_identity);
            assert_eq!(resolved.permitted_requests(), &RemoteControlRequestSet::only(RemoteControlRequestKind::Describe));
        }
        result = restored.run() => panic!("restored recipient stopped: {result:?}"),
    }
}
