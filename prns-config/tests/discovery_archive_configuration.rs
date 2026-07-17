use std::fs;
use std::path::PathBuf;

use prns_config::{plan, reference, PlannedMedium};
use prns_core::identity::IdentityHash;
use prns_core::interface_discovery::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveredInterface,
    DiscoveredInterfaceId, DiscoveryAdvertisement, DiscoveryArchive, DiscoveryCatalog,
    DiscoveryEnvelopeSecurity, DiscoveryProvenance, GeographicLocation, StampValue,
    DISCOVERED_INTERFACES_FILE,
};
use prns_core::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use prns_core::units::{HopCount, InstantMillis};
use prns_core::wire::TransportId;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        Self(std::env::temp_dir().join(format!(
            "prns-discovery-config-{}-{nanos}",
            std::process::id()
        )))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn archived_manual_configuration_is_accepted_by_the_reference_config_pipeline() {
    let directory = TestDirectory::new();
    let path = directory.0.join(DISCOVERED_INTERFACES_FILE);
    let mut loaded = DiscoveryArchive::load(path.clone()).unwrap();
    let mut catalog = DiscoveryCatalog::new();
    let interface = DiscoveredInterface {
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
            received_at: InstantMillis(2_000),
            envelope_security: DiscoveryEnvelopeSecurity::NetworkEncrypted,
            signed_flag: true,
        },
    };
    let id = interface.id;
    catalog.observe(interface).unwrap();
    loaded.archive.record(catalog.get(id).unwrap()).unwrap();

    let json: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let entry = json["interfaces"]
        .as_object()
        .and_then(|interfaces| interfaces.values().next())
        .and_then(|interface| interface["configuration_entry"].as_str())
        .unwrap();
    let parsed = reference::parse(&format!("[interfaces]\n{entry}\n")).unwrap();
    let planned = plan(&parsed);

    assert!(matches!(
        planned.interfaces.as_slice(),
        [interface]
            if matches!(
                &interface.medium,
                PlannedMedium::BackboneClient { connection }
                    if connection.host == "backbone.example" && connection.port == 4242
            )
    ));
}
