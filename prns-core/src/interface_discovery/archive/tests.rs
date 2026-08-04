use std::fs;
use std::path::PathBuf;

use crate::identity::IdentityHash;
use crate::interface_discovery::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveredInterface,
    DiscoveredInterfaceId, DiscoveryAdvertisement, DiscoveryCatalog, DiscoveryEnvelopeSecurity,
    DiscoveryProvenance, GeographicLocation, StampValue,
};
use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use crate::units::{HopCount, InstantMillis};
use crate::wire::TransportId;

use super::document::ArchivedFloat;
use super::{
    ArchiveRecordError, DiscoveryArchive, DiscoveryArchiveError, DiscoveryArchiveFileState,
    DiscoveryArchiveRecord, DISCOVERED_INTERFACES_FILE, MAX_ARCHIVE_BYTES,
};

fn test_path(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "prns-discovery-archive-{}-{nanos}-{name}",
        std::process::id()
    ))
}

#[test]
fn removing_a_discovery_is_persisted() {
    let dir = test_path("remove");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    let mut loaded = DiscoveryArchive::load(path.clone()).unwrap();
    let id = DiscoveredInterfaceId::from_bytes([0x11; 32]);
    let mut catalog = DiscoveryCatalog::new();
    catalog
        .observe(discovered(2_000))
        .expect("the growable catalog accepts the test record");
    loaded.archive.record(catalog.get(id).unwrap()).unwrap();

    loaded
        .archive
        .record(DiscoveryArchiveRecord::remove(id))
        .unwrap();

    assert!(loaded.archive.is_empty());
    assert!(DiscoveryArchive::load(path).unwrap().catalog.is_empty());
    let _ = fs::remove_dir_all(dir);
}

fn discovered(received_at: u64) -> DiscoveredInterface {
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
        stamp_value: StampValue::new(19).unwrap(),
        provenance: DiscoveryProvenance {
            announced_by: IdentityHash::new([0x33; 16]),
            hops: HopCount(2),
            received_on: InterfaceId::new([0x44; INTERFACE_ID_LEN]),
            received_at: InstantMillis(received_at),
            envelope_security: DiscoveryEnvelopeSecurity::NetworkEncrypted,
            signed_flag: true,
        },
    }
}

#[test]
fn archive_round_trip_restores_catalog_history_and_manual_configuration() {
    let dir = test_path("round-trip");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    let mut loaded = DiscoveryArchive::load(path.clone()).unwrap();
    assert_eq!(loaded.file_state, DiscoveryArchiveFileState::Missing);
    loaded.archive.persist().unwrap();

    let mut catalog = DiscoveryCatalog::new();
    let id = DiscoveredInterfaceId::from_bytes([0x11; 32]);
    catalog
        .observe(discovered(2_000))
        .expect("the growable catalog accepts the test record");
    let stored = catalog.get(id).unwrap();
    loaded.archive.record(stored).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("BackboneClientInterface"));
    assert!(text.contains("target_host = 'backbone.example'"));
    assert!(text.contains("\"network_encrypted\""));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let entry = json["interfaces"]
        .as_object()
        .and_then(|interfaces| interfaces.values().next())
        .and_then(|interface| interface["configuration_entry"].as_str())
        .unwrap();
    assert_eq!(
        entry,
        concat!(
            "[[Public Backbone (111111111111)]]\n",
            "  type = BackboneClientInterface\n",
            "  target_host = 'backbone.example'\n",
            "  target_port = 4242\n",
            "  enabled = Yes\n",
            "  transport_identity = 22222222222222222222222222222222",
        )
    );

    let restored = DiscoveryArchive::load(path).unwrap();
    let restored_record = restored.catalog.get(id).unwrap();
    assert_eq!(restored_record.first_heard(), InstantMillis(2_000));
    assert_eq!(restored_record.last_heard(), InstantMillis(2_000));
    assert_eq!(restored_record.observation_count().get(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn rediscovery_after_catalog_expiry_keeps_durable_history() {
    let dir = test_path("rediscovery");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    let mut loaded = DiscoveryArchive::load(path.clone()).unwrap();
    let id = DiscoveredInterfaceId::from_bytes([0x11; 32]);

    let mut first_catalog = DiscoveryCatalog::new();
    first_catalog
        .observe(discovered(1_000))
        .expect("the growable catalog accepts the test record");
    loaded
        .archive
        .record(first_catalog.get(id).unwrap())
        .unwrap();

    let mut second_catalog = DiscoveryCatalog::new();
    second_catalog
        .observe(discovered(9_000))
        .expect("the growable catalog accepts the test record");
    loaded
        .archive
        .record(second_catalog.get(id).unwrap())
        .unwrap();

    let restored = DiscoveryArchive::load(path).unwrap();
    let restored_record = restored.catalog.get(id).unwrap();
    assert_eq!(restored_record.first_heard(), InstantMillis(1_000));
    assert_eq!(restored_record.last_heard(), InstantMillis(9_000));
    assert_eq!(restored_record.observation_count().get(), 2);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn non_finite_locations_round_trip_without_invalid_json_numbers() {
    let value = ArchivedFloat::from_value(f64::NAN);
    let encoded = serde_json::to_string(&value).unwrap();
    assert_eq!(encoded, "\"NaN\"");
    let decoded: ArchivedFloat = serde_json::from_str(&encoded).unwrap();
    assert!(decoded.decode("latitude").unwrap().is_nan());
}

#[test]
fn invalid_archives_are_left_byte_identical() {
    let dir = test_path("invalid");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    fs::create_dir_all(&dir).unwrap();
    let original = b"this is not a discovery archive\n";
    fs::write(&path, original).unwrap();

    assert!(DiscoveryArchive::load(path.clone()).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn edited_archives_cannot_bypass_reachable_address_validation() {
    let dir = test_path("invalid-address");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    let mut loaded = DiscoveryArchive::load(path.clone()).unwrap();
    let mut catalog = DiscoveryCatalog::new();
    let interface = discovered(2_000);
    let id = interface.id;
    catalog
        .observe(interface)
        .expect("the growable catalog accepts the test record");
    loaded.archive.record(catalog.get(id).unwrap()).unwrap();
    let invalid = fs::read_to_string(&path)
        .unwrap()
        .replace("backbone.example", "not a host");
    fs::write(&path, &invalid).unwrap();

    let result = DiscoveryArchive::load(path.clone());

    assert!(matches!(
        result,
        Err(DiscoveryArchiveError::InvalidRecord {
            source: ArchiveRecordError::InvalidReachableAddress { .. },
            ..
        })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn edited_archives_cannot_restore_unattainable_stamp_values() {
    let dir = test_path("invalid-stamp");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    let mut loaded = DiscoveryArchive::load(path.clone()).unwrap();
    let mut catalog = DiscoveryCatalog::new();
    let interface = discovered(2_000);
    let id = interface.id;
    catalog
        .observe(interface)
        .expect("the growable catalog accepts the test record");
    loaded.archive.record(catalog.get(id).unwrap()).unwrap();
    let invalid = fs::read_to_string(&path)
        .unwrap()
        .replace("\"stamp_value\": 19", "\"stamp_value\": 257");
    fs::write(&path, &invalid).unwrap();

    let result = DiscoveryArchive::load(path.clone());

    assert!(matches!(
        result,
        Err(DiscoveryArchiveError::InvalidRecord {
            source: ArchiveRecordError::StampValue(_),
            ..
        })
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn serialized_interfaces_are_ordered_by_discovery_id() {
    let dir = test_path("ordering");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    let mut loaded = DiscoveryArchive::load(path.clone()).unwrap();
    let mut catalog = DiscoveryCatalog::new();
    let second_id = DiscoveredInterfaceId::from_bytes([0x22; 32]);
    let first_id = DiscoveredInterfaceId::from_bytes([0x11; 32]);

    let mut second = discovered(2_000);
    second.id = second_id;
    catalog
        .observe(second)
        .expect("the growable catalog accepts the test record");
    loaded
        .archive
        .record(catalog.get(second_id).unwrap())
        .unwrap();

    let mut first = discovered(1_000);
    first.id = first_id;
    catalog
        .observe(first)
        .expect("the growable catalog accepts the test record");
    loaded
        .archive
        .record(catalog.get(first_id).unwrap())
        .unwrap();

    let text = fs::read_to_string(&path).unwrap();
    let first_key = "11".repeat(32);
    let second_key = "22".repeat(32);
    let first_position = text.find(&first_key).unwrap();
    let second_position = text.find(&second_key).unwrap();
    assert!(first_position < second_position);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_archive_never_writes_a_file_it_would_refuse_to_load() {
    let dir = test_path("write-limit");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    let mut loaded = DiscoveryArchive::load(path.clone()).unwrap();
    loaded.archive.persist().unwrap();
    let original = fs::read(&path).unwrap();
    let mut interface = discovered(2_000);
    interface.name = "x".repeat(MAX_ARCHIVE_BYTES);
    let id = interface.id;
    let mut catalog = DiscoveryCatalog::new();
    catalog
        .observe(interface)
        .expect("the growable catalog accepts the test record");

    let result = loaded.archive.record(catalog.get(id).unwrap());

    assert!(matches!(
        result,
        Err(DiscoveryArchiveError::TooLarge { .. })
    ));
    assert!(loaded.archive.is_empty());
    assert_eq!(fs::read(&path).unwrap(), original);
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn archive_replacements_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = test_path("permissions");
    let path = dir.join(DISCOVERED_INTERFACES_FILE);
    let loaded = DiscoveryArchive::load(path.clone()).unwrap();
    loaded.archive.persist().unwrap();

    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let _ = fs::remove_dir_all(dir);
}
