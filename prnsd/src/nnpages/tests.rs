use super::*;
use personal_rns::engine::RatchetPolicy;

#[test]
fn operator_layout_is_isolated_beneath_nnpages() {
    let config = Path::new("/var/lib/prnsd");
    assert_eq!(root(config), config.join("nnpages"));
    assert_eq!(page_root(config), config.join("nnpages/pages"));
    assert_eq!(file_root(config), config.join("nnpages/files"));
    assert_eq!(settings_path(config), config.join("nnpages/settings.toml"));
    assert_eq!(
        NnPagesCatalog::empty(config).node_name_path(),
        config.join("nnpages/name")
    );
}

#[test]
fn settings_creation_alone_requires_a_live_refresh() {
    assert!(seed_requires_refresh(true, false, false, false));
    assert!(!seed_requires_refresh(false, false, false, false));
}

#[test]
fn the_node_name_file_is_never_published() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = page_root(directory.path());
    fs::create_dir_all(&root).expect("page root");
    fs::write(root.join(INDEX_FILE_NAME), b"index").expect("index");
    fs::write(root.join(NODE_NAME_FILE_NAME), b"Frosty Relay").expect("name");
    let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");
    assert_eq!(
        catalog.request_paths(),
        vec![String::from("/page/index.mu")]
    );
    assert_eq!(
        safe_page_name(std::ffi::OsStr::new(NODE_NAME_FILE_NAME)),
        None
    );
}

#[test]
fn node_names_read_trimmed_and_blank_is_none() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join(NODE_NAME_FILE_NAME);
    fs::write(&path, "  Frosty Relay \n").expect("name");
    assert_eq!(read_node_name(&path).as_deref(), Some("Frosty Relay"));
    fs::write(&path, " \n").expect("blank");
    assert_eq!(read_node_name(&path), None);
    fs::write(&path, "first\nsecond").expect("multiline");
    assert_eq!(read_node_name(&path), None);
    fs::write(&path, "control\tname").expect("control");
    assert_eq!(read_node_name(&path), None);
    fs::write(&path, "x".repeat(MAX_ANNOUNCE_APP_DATA_LEN + 1)).expect("long");
    assert_eq!(read_node_name(&path), None);
    assert_eq!(read_node_name(&directory.path().join("absent")), None);
}

#[test]
fn node_name_writes_atomically_replace_complete_values() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = super::root(directory.path());
    prepare_operator_root(&root).expect("NNPages root");
    let path = root.join(NODE_NAME_FILE_NAME);

    atomic_control_write(&path, b"First Name").expect("first name");
    atomic_control_write(&path, b"Replacement Name").expect("replacement name");

    assert_eq!(
        fs::read_to_string(path).expect("name is readable"),
        "Replacement Name"
    );
}

#[tokio::test]
async fn rename_succeeds_durably_when_index_is_unavailable() {
    let directory = tempfile::tempdir().expect("temporary directory");
    run_cli(crate::cli::NnPagesArgs {
        command: crate::cli::NnPagesCommand::Rename(crate::cli::NnPagesRenameArgs {
            name: String::from("My Node"),
            config: Some(directory.path().to_path_buf()),
        }),
    })
    .await
    .expect("rename succeeds");
    assert_eq!(
        fs::read_to_string(root(directory.path()).join(NODE_NAME_FILE_NAME)).expect("saved name"),
        "My Node"
    );
}

use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;

#[test]
fn catalog_indexes_safe_mu_files_and_recurses_into_safe_directories() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = page_root(directory.path());
    fs::create_dir_all(&root).expect("page root");
    fs::write(root.join("index.mu"), b"index").expect("index");
    fs::write(root.join("about-us_2.mu"), b"about").expect("about");
    fs::write(root.join("ignored.txt"), b"ignored").expect("ignored");
    fs::write(root.join(".private.mu"), b"private").expect("private");
    fs::create_dir(root.join("docs")).expect("docs directory");
    fs::write(root.join("docs/guide.mu"), b"guide").expect("guide");
    fs::create_dir(root.join("docs/deep")).expect("deep directory");
    fs::write(root.join("docs/deep/detail.mu"), b"detail").expect("detail");
    fs::create_dir(root.join(".hidden")).expect("hidden directory");
    fs::write(root.join(".hidden/secret.mu"), b"secret").expect("secret");
    fs::create_dir(root.join("nested.mu")).expect("nested directory");

    let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");

    assert_eq!(
        catalog.request_paths(),
        [
            "/page/about-us_2.mu",
            "/page/docs/deep/detail.mu",
            "/page/docs/guide.mu",
            "/page/index.mu"
        ]
    );
}

#[test]
fn the_files_directory_serves_safe_names_under_file_paths() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let pages = page_root(directory.path());
    fs::create_dir_all(&pages).expect("page root");
    fs::write(pages.join(INDEX_FILE_NAME), b"index").expect("index");
    let files = file_root(directory.path());
    fs::create_dir(&files).expect("file root");
    fs::write(files.join("demo.txt"), b"demo").expect("demo");
    fs::write(files.join("download.mu"), b"download").expect("mu download");
    fs::create_dir(files.join("sub")).expect("sub directory");
    fs::write(files.join("sub/data.bin"), b"data").expect("data");
    fs::write(files.join(".hidden"), b"hidden").expect("hidden");

    let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");

    assert_eq!(
        catalog.request_paths(),
        [
            "/file/demo.txt",
            "/file/download.mu",
            "/file/sub/data.bin",
            "/page/index.mu"
        ]
    );
}

#[test]
fn page_bytes_are_read_fresh_and_deletion_is_unavailable() {
    use std::io::Read;

    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();
    let path = root.join(INDEX_FILE_NAME);
    fs::write(&path, b"first").expect("first page");
    let mut first = open_hosted(root, Path::new(INDEX_FILE_NAME), MAX_PAGE_BYTES)
        .expect("first open")
        .file;
    let mut first_bytes = Vec::new();
    first.read_to_end(&mut first_bytes).expect("first read");
    assert_eq!(first_bytes, b"first");

    fs::write(&path, b"second").expect("second page");
    let mut second = open_hosted(root, Path::new(INDEX_FILE_NAME), MAX_PAGE_BYTES)
        .expect("second open")
        .file;
    let mut second_bytes = Vec::new();
    second.read_to_end(&mut second_bytes).expect("second read");
    assert_eq!(second_bytes, b"second");

    fs::remove_file(&path).expect("delete page");
    assert!(matches!(
        open_hosted(root, Path::new(INDEX_FILE_NAME), MAX_PAGE_BYTES),
        Err(HostedReadError::Unavailable)
    ));
}

#[test]
fn hosted_reads_enforce_the_kinds_size_limit() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("data.bin");
    fs::write(&path, b"12345").expect("data");
    assert_eq!(
        open_hosted(directory.path(), Path::new("data.bin"), 5)
            .expect("fits")
            .byte_len,
        5
    );
    assert!(matches!(
        open_hosted(directory.path(), Path::new("data.bin"), 4),
        Err(HostedReadError::TooLarge)
    ));
}

#[tokio::test]
async fn live_refresh_registers_added_paths_and_retires_removed_paths() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = page_root(directory.path());
    fs::create_dir_all(&root).expect("page root");
    let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");
    let mut node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: crate::test_support::remote_control_service(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: personal_rns::request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state: &()| {},
    });
    let destination = node
        .register_preconfigured_destination(PreConfiguredDestination::Single {
            app_name: "nomadnetwork",
            aspects: &["node"],
            identity: Zeroizing::new([0x42; IDENTITY_SECRET_KEY_LEN]),
            announce_app_data: &[],
            proof: ProofStrategy::ProveNone,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        })
        .expect("destination");
    let handle = node.handle();
    let mut announcement_settings = catalog.announcement_settings();
    let exercise = async {
        fs::write(root.join("index.mu"), b"index").expect("index");
        fs::write(root.join("about.mu"), b"about").expect("about");
        let files = file_root(directory.path());
        fs::create_dir(&files).expect("file root");
        fs::write(files.join("hello.txt"), b"hello").expect("hello");
        let added = catalog
            .refresh(&handle, destination)
            .await
            .expect("add routes");
        assert_eq!(
            added,
            NnPagesRefreshReport {
                discovered: 3,
                added: 3,
                removed: 0,
                unchanged: 0,
                settings_status: NnPagesSettingsStatus::MissingDefaults,
                settings_changed: false,
            }
        );
        assert_eq!(
            catalog.request_paths(),
            ["/file/hello.txt", "/page/about.mu", "/page/index.mu"]
        );

        fs::remove_file(root.join("about.mu")).expect("remove about");
        fs::write(
            settings_path(directory.path()),
            "announce = false\nannounce_interval_minutes = 45\n",
        )
        .expect("changed settings");
        let removed = catalog
            .refresh(&handle, destination)
            .await
            .expect("remove route");
        assert_eq!(
            removed,
            NnPagesRefreshReport {
                discovered: 2,
                added: 0,
                removed: 1,
                unchanged: 2,
                settings_status: NnPagesSettingsStatus::Loaded,
                settings_changed: true,
            }
        );
        announcement_settings
            .changed()
            .await
            .expect("settings update");
        assert!(!announcement_settings.borrow_and_update().announce());
        assert_eq!(
            catalog.request_paths(),
            ["/file/hello.txt", "/page/index.mu"]
        );

        let unchanged = catalog
            .refresh(&handle, destination)
            .await
            .expect("unchanged refresh");
        assert_eq!(unchanged.settings_status, NnPagesSettingsStatus::Loaded);
        assert!(!unchanged.settings_changed);
    };
    tokio::pin!(exercise);
    tokio::select! {
        () = &mut exercise => {}
        result = node.run() => panic!("node stopped during refresh: {result:?}"),
    }
}

#[cfg(unix)]
#[test]
fn symlinks_are_never_published_served_or_traversed() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temporary directory");
    let root = page_root(directory.path());
    fs::create_dir_all(&root).expect("page root");
    let source = directory.path().join("source.mu");
    fs::write(&source, b"outside").expect("source");
    let linked = root.join("linked.mu");
    symlink(&source, &linked).expect("symlink");
    let outside = directory.path().join("outside");
    fs::create_dir(&outside).expect("outside directory");
    fs::write(outside.join("leak.mu"), b"leak").expect("leak");
    symlink(&outside, root.join("tour")).expect("directory symlink");

    let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");
    assert!(catalog.request_paths().is_empty());
    assert!(matches!(
        open_hosted(&root, Path::new("linked.mu"), MAX_PAGE_BYTES),
        Err(HostedReadError::Unavailable) | Err(HostedReadError::Read(_))
    ));

    let external_name = directory.path().join("external-name");
    fs::write(&external_name, b"Outside Name").expect("external name");
    let name = super::root(directory.path()).join(NODE_NAME_FILE_NAME);
    symlink(&external_name, &name).expect("name symlink");
    assert_eq!(read_node_name(&name), None);
}

#[cfg(unix)]
#[test]
fn a_directory_replaced_by_a_symlink_after_scan_cannot_escape_the_root() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temporary directory");
    let root = page_root(directory.path());
    let section = root.join("section");
    fs::create_dir_all(&section).expect("section");
    fs::write(section.join("entry.mu"), b"inside").expect("inside");
    let catalog = NnPagesCatalog::discover(directory.path()).expect("catalog");
    assert_eq!(catalog.request_paths(), ["/page/section/entry.mu"]);

    fs::remove_file(section.join("entry.mu")).expect("remove entry");
    fs::remove_dir(&section).expect("remove section");
    let outside = directory.path().join("outside");
    fs::create_dir(&outside).expect("outside");
    fs::write(outside.join("entry.mu"), b"outside").expect("outside entry");
    symlink(&outside, &section).expect("replace section with symlink");

    assert!(matches!(
        open_hosted(&root, Path::new("section/entry.mu"), MAX_PAGE_BYTES),
        Err(HostedReadError::Unavailable) | Err(HostedReadError::Read(_))
    ));
}
