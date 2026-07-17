use std::fs;
use std::path::PathBuf;

use personal_rns::config::{plan, reference};
use personal_rns::identity::IdentityHash;
use personal_rns::interface_discovery::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveredInterface,
    DiscoveredInterfaceId, DiscoveryAdvertisement, DiscoveryArchive, DiscoveryArchiveFileState,
    DiscoveryCatalog, DiscoveryCatalogRefresh, DiscoveryCatalogUpdate, DiscoveryEnvelopeSecurity,
    DiscoveryProvenance, GeographicLocation, StampValue, DISCOVERED_INTERFACES_FILE,
};
use personal_rns::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use personal_rns::units::{HopCount, InstantMillis};
use personal_rns::wire::TransportId;

use super::{
    discovery_archive_record, load_discovery_archive, start_archive_worker, PreparedDiscovery,
    TokioDiscoveryEvent,
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        Self(std::env::temp_dir().join(format!(
            "prnsd-discovery-archive-{}-{nanos}",
            std::process::id()
        )))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn discovered(received_at: InstantMillis) -> DiscoveredInterface {
    DiscoveredInterface {
        id: DiscoveredInterfaceId::from_bytes([0x11; 32]),
        name: String::from("Public Backbone"),
        advertisement: DiscoveryAdvertisement {
            interface_type: AdvertisedInterfaceType::Backbone,
            transport: AdvertisedTransport::Enabled(TransportId::new([0x22; 16])),
            name: Some(String::from("Public Backbone")),
            location: GeographicLocation::UNKNOWN,
            details: AdvertisementDetails::Reachable {
                host: String::from("backbone.example"),
                port: 4242,
            },
            published_ifac: None,
        },
        stamp_value: StampValue::new(19).expect("the fixture stamp value is attainable"),
        provenance: DiscoveryProvenance {
            announced_by: IdentityHash::new([0x33; 16]),
            hops: HopCount(2),
            received_on: InterfaceId::new([0x44; INTERFACE_ID_LEN]),
            received_at,
            envelope_security: DiscoveryEnvelopeSecurity::NetworkEncrypted,
            signed_flag: true,
        },
    }
}

#[test]
fn enabled_discovery_selects_the_well_known_config_directory_archive() {
    let directory = TestDirectory::new();
    let config = reference::parse("[reticulum]\ndiscover_interfaces = Yes\n")
        .expect("the fixture config is valid");
    let plan = plan(&config);

    let prepared = PreparedDiscovery::from_plan(&plan, None, &directory.0)
        .expect("the fixture enables discovery");

    assert_eq!(
        prepared.archive_path,
        directory.0.join(DISCOVERED_INTERFACES_FILE)
    );
    assert!(!prepared.archive_path.exists());
}

#[test]
fn disabled_discovery_leaves_an_existing_archive_untouched() {
    let directory = TestDirectory::new();
    fs::create_dir_all(&directory.0).expect("the fixture directory is writable");
    let path = directory.0.join(DISCOVERED_INTERFACES_FILE);
    let original = b"existing archive contents\n";
    fs::write(&path, original).expect("the fixture archive is writable");
    let config = reference::parse("[reticulum]\ndiscover_interfaces = No\n")
        .expect("the fixture config is valid");
    let plan = plan(&config);

    let prepared = PreparedDiscovery::from_plan(&plan, None, &directory.0);

    assert!(prepared.is_none());
    assert_eq!(
        fs::read(path).expect("the fixture remains readable"),
        original
    );
}

#[test]
fn archive_capture_selects_only_catalog_updates_that_changed_history() {
    let mut catalog = DiscoveryCatalog::new();
    let id = DiscoveredInterfaceId::from_bytes([0x11; 32]);
    let added = catalog
        .observe(discovered(InstantMillis(2_000)))
        .expect("the growable catalog accepts the fixture");
    let record = catalog.get(id).expect("the fixture was inserted");
    assert!(
        discovery_archive_record(&TokioDiscoveryEvent::CatalogUpdated {
            update: added,
            record,
        })
        .is_some()
    );

    let refreshed = catalog
        .observe(discovered(InstantMillis(3_000)))
        .expect("the growable catalog accepts the refresh");
    assert!(matches!(
        refreshed,
        DiscoveryCatalogUpdate::Refreshed {
            refresh: DiscoveryCatalogRefresh::AdvertisementUnchanged,
            ..
        }
    ));
    let record = catalog.get(id).expect("the fixture remains available");
    assert!(
        discovery_archive_record(&TokioDiscoveryEvent::CatalogUpdated {
            update: refreshed,
            record,
        })
        .is_some()
    );

    assert!(
        discovery_archive_record(&TokioDiscoveryEvent::CatalogUpdated {
            update: DiscoveryCatalogUpdate::IgnoredOutOfOrder {
                id,
                received_at: InstantMillis(2_999),
                last_heard: InstantMillis(3_000),
            },
            record,
        })
        .is_none()
    );
}

#[tokio::test]
async fn archive_loading_initializes_the_well_known_file_off_loop() {
    let directory = TestDirectory::new();
    let path = directory.0.join(DISCOVERED_INTERFACES_FILE);

    let loaded = load_discovery_archive(path.clone())
        .await
        .expect("the missing archive initializes");

    assert_eq!(loaded.file_state, DiscoveryArchiveFileState::Missing);
    assert_eq!(loaded.archive.path(), path);
    assert!(path.is_file());
}

#[tokio::test]
async fn archive_worker_drains_queued_records_before_finishing() {
    let directory = TestDirectory::new();
    let path = directory.0.join(DISCOVERED_INTERFACES_FILE);
    let loaded = DiscoveryArchive::load(path.clone()).expect("the missing archive initializes");
    loaded
        .archive
        .persist()
        .expect("the empty archive is writable");
    let (sink, worker) = start_archive_worker(loaded.archive);
    let mut catalog = DiscoveryCatalog::new();
    let update = catalog
        .observe(discovered(InstantMillis(2_000)))
        .expect("the growable catalog accepts the fixture");
    let id = update.id();
    let record = catalog.get(id).expect("the fixture was inserted");

    sink.record(&TokioDiscoveryEvent::CatalogUpdated { update, record });
    drop(sink);
    worker.finish().await;

    let restored = DiscoveryArchive::load(path).expect("the drained archive remains valid");
    let record = restored
        .catalog
        .get(id)
        .expect("the queued record was persisted");
    assert_eq!(record.first_heard(), InstantMillis(2_000));
    assert_eq!(record.last_heard(), InstantMillis(2_000));
    assert_eq!(record.observation_count().get(), 1);
}
