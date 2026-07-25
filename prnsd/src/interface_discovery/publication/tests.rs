#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};

use personal_rns::config::{
    parse_and_plan, DaemonPlan, DiscoveryEncryption, InterfaceDiscoveryPlan,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interface_discovery::{
    discovery_destination_hash, AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails,
    DiscoveryAdvertisement, GeographicLocation, PublishedIfac,
};
use personal_rns::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use personal_rns::wire::TransportId;

use super::*;

const TCP_SERVER: &str = "[reticulum]\n\
    enable_transport = Yes\n\
    [interfaces]\n\
      [[Public TCP]]\n\
        type = TCPServerInterface\n\
        interface_enabled = Yes\n\
        listen_ip = 0.0.0.0\n\
        listen_port = 4242\n\
        network_name = Private Mesh\n\
        pass_phrase = mesh secret\n\
        discoverable = Yes\n\
        reachable_on = tcp.example\n\
        publish_ifac = Yes\n\
        latitude = 45.5\n\
        longitude = -93.2\n\
        height = 270.0\n";

#[cfg(unix)]
struct TestDirectory(PathBuf);

#[cfg(unix)]
impl TestDirectory {
    fn new() -> Self {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "prnsd-discovery-publication-{}-{nanos}-{sequence}",
            std::process::id()
        )))
    }
}

#[cfg(unix)]
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn identity_secret(encryption: u8, signing: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret = Zeroizing::new([0; IDENTITY_SECRET_KEY_LEN]);
    secret[..32].fill(encryption);
    secret[32..].fill(signing);
    secret
}

fn interface_id(value: u8) -> InterfaceId {
    InterfaceId::new([value; INTERFACE_ID_LEN])
}

fn planned(config: &str) -> DaemonPlan {
    parse_and_plan(config)
        .expect("the fixture config is valid")
        .value
}

#[test]
fn network_identity_owns_the_destination_while_transport_identity_stays_advertised() {
    let plan = planned(TCP_SERVER);
    let transport_secret = identity_secret(0x11, 0x22);
    let network_secret = identity_secret(0x33, 0x44);

    let (destination, prepared) = prepare(&plan, &transport_secret, Some(&network_secret));

    let transport = InMemoryNodeIdentity::from_secret_key_bytes(&transport_secret);
    let network = InMemoryNodeIdentity::from_secret_key_bytes(&network_secret);
    let expected_destination = discovery_destination_hash(&network.identity_hash());
    assert_eq!(
        destination
            .destination_hash()
            .expect("the pinned discovery name is valid"),
        expected_destination
    );
    assert_eq!(prepared.destination, expected_destination);
    assert_eq!(
        prepared.transport_id,
        TransportId::new(*transport.identity_hash().as_bytes())
    );
    assert_eq!(
        prepared
            .network_identity
            .expect("the network identity is retained for encryption")
            .identity_hash(),
        network.identity_hash()
    );
    assert!(prepared.transport_enabled);
}

#[test]
fn transport_identity_owns_the_destination_without_a_network_identity() {
    let plan = planned(TCP_SERVER);
    let transport_secret = identity_secret(0x11, 0x22);

    let (destination, prepared) = prepare(&plan, &transport_secret, None);
    let transport = InMemoryNodeIdentity::from_secret_key_bytes(&transport_secret);
    let expected_destination = discovery_destination_hash(&transport.identity_hash());

    assert_eq!(
        destination
            .destination_hash()
            .expect("the pinned discovery name is valid"),
        expected_destination
    );
    assert_eq!(prepared.destination, expected_destination);
    assert!(prepared.network_identity.is_none());
}

#[test]
fn no_publishable_interface_still_registers_the_dormant_discovery_destination() {
    let plan = planned(
        "[interfaces]\n\
         [[LAN]]\n\
         type = AutoInterface\n\
         interface_enabled = Yes\n\
         discoverable = Yes\n",
    );
    let transport_secret = identity_secret(0x11, 0x22);

    let (destination, prepared) = prepare(&plan, &transport_secret, None);
    assert_eq!(
        destination
            .destination_hash()
            .expect("the pinned discovery name is valid"),
        prepared.destination
    );
}

#[tokio::test]
async fn configured_fields_materialize_into_the_complete_wire_advertisement() {
    let mut plan = planned(TCP_SERVER);
    let InterfaceDiscoveryPlan::Announce(announcement) = &mut plan.interfaces[0].discovery else {
        panic!("the fixture interface is publishable");
    };
    announcement.name = Some(String::from("  Public\r\n TCP  "));
    let transport_secret = identity_secret(0x11, 0x22);
    let (_, prepared) = prepare(&plan, &transport_secret, None);
    let id = interface_id(0x51);
    let sources = prepared
        .publication_sources(vec![AttachedConfiguredInterface {
            id,
            plan: plan.interfaces.remove(0),
        }])
        .expect("the attached interface is unique");

    let advertisement = sources
        .get(&id)
        .expect("the publishable interface is retained")
        .advertisement(id)
        .await
        .expect("the literal endpoint resolves");
    let transport = InMemoryNodeIdentity::from_secret_key_bytes(&transport_secret);
    assert_eq!(
        advertisement,
        DiscoveryAdvertisement {
            interface_type: AdvertisedInterfaceType::TcpServer,
            transport: AdvertisedTransport::Enabled(TransportId::new(
                *transport.identity_hash().as_bytes(),
            )),
            name: Some(String::from("Public TCP")),
            location: GeographicLocation {
                latitude: Some(45.5),
                longitude: Some(-93.2),
                height: Some(270.0),
            },
            details: AdvertisementDetails::Reachable {
                host: String::from("tcp.example"),
                port: 4242,
            },
            published_ifac: Some(PublishedIfac {
                network_name: Some(String::from("Private Mesh")),
                passphrase: Some(String::from("mesh secret")),
            }),
        }
    );
}

#[test]
fn unavailable_network_encryption_does_not_suppress_plaintext_interfaces() {
    let config = format!(
        "{TCP_SERVER}\n\
         [[Encrypted TCP]]\n\
         type = TCPServerInterface\n\
         interface_enabled = Yes\n\
         listen_ip = 0.0.0.0\n\
         listen_port = 4243\n\
         discoverable = Yes\n\
         discovery_encrypt = Yes\n\
         reachable_on = encrypted.example\n"
    );
    let mut plan = planned(&config);
    assert_eq!(plan.interfaces.len(), 2);
    let transport_secret = identity_secret(0x11, 0x22);
    let (_, prepared) = prepare(&plan, &transport_secret, None);
    let second = plan.interfaces.remove(1);
    let first = plan.interfaces.remove(0);

    let sources = prepared
        .publication_sources(vec![
            AttachedConfiguredInterface {
                id: interface_id(0x51),
                plan: first,
            },
            AttachedConfiguredInterface {
                id: interface_id(0x52),
                plan: second,
            },
        ])
        .expect("the attached interfaces are unique");

    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources
            .values()
            .next()
            .expect("the plaintext source remains")
            .announcement
            .encryption,
        DiscoveryEncryption::Plaintext
    );
}

#[test]
fn duplicate_attached_interface_ids_are_rejected() {
    let mut plan = planned(TCP_SERVER);
    let transport_secret = identity_secret(0x11, 0x22);
    let (_, prepared) = prepare(&plan, &transport_secret, None);
    let first = plan.interfaces.remove(0);
    let second = first.clone();
    let id = interface_id(0x51);

    assert!(matches!(
        prepared.publication_sources(vec![
            AttachedConfiguredInterface { id, plan: first },
            AttachedConfiguredInterface { id, plan: second },
        ]),
        Err(DiscoveryPublisherStartError::DuplicateInterface { interface }) if interface == id
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn executable_reachable_on_is_re_evaluated_for_each_advertisement() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new();
    fs::create_dir_all(&directory.0).expect("the fixture directory is writable");
    let executable = directory.0.join("reachable-on");
    fs::write(&executable, "#!/bin/sh\nprintf 'first.example\\n'\n")
        .expect("the fixture executable is writable");
    let mut permissions = fs::metadata(&executable)
        .expect("the fixture executable has metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("the fixture can be made executable");
    let id = interface_id(0x51);

    assert_eq!(
        resolve_reachable_on(
            executable.to_str().expect("the fixture path is UTF-8"),
            id,
            "Public TCP",
        )
        .await
        .expect("the first execution succeeds"),
        "first.example"
    );
    let replacement = directory.0.join("reachable-on-next");
    fs::write(&replacement, "#!/bin/sh\nprintf 'second.example\\n'\n")
        .expect("the replacement executable is writable");
    let mut permissions = fs::metadata(&replacement)
        .expect("the replacement executable has metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&replacement, permissions).expect("the replacement can be made executable");
    fs::rename(&replacement, &executable).expect("the fixture executable can change");
    assert_eq!(
        resolve_reachable_on(
            executable.to_str().expect("the fixture path is UTF-8"),
            id,
            "Public TCP",
        )
        .await
        .expect("the second execution succeeds"),
        "second.example"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn failed_reachable_on_executable_is_typed() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new();
    fs::create_dir_all(&directory.0).expect("the fixture directory is writable");
    let executable = directory.0.join("reachable-on");
    fs::write(&executable, "#!/bin/sh\nexit 7\n").expect("the fixture executable is writable");
    let mut permissions = fs::metadata(&executable)
        .expect("the fixture executable has metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("the fixture can be made executable");

    assert!(matches!(
        resolve_reachable_on(
            executable.to_str().expect("the fixture path is UTF-8"),
            interface_id(0x51),
            "Public TCP",
        )
        .await,
        Err(DiscoveryAdvertisementResolutionError::Exit { status, .. })
            if status.code() == Some(7)
    ));
}

#[cfg(unix)]
#[test]
fn user_relative_reachable_on_paths_expand_from_the_supplied_home() {
    assert_eq!(
        expand_user_path(
            Path::new("~/bin/reachable-on"),
            Some(OsStr::new("/home/operator")),
        )
        .expect("the supplied home resolves the path"),
        PathBuf::from("/home/operator/bin/reachable-on")
    );
    assert!(matches!(
        expand_user_path(Path::new("~/bin/reachable-on"), None),
        Err(DiscoveryAdvertisementResolutionError::HomeUnavailable { .. })
    ));
}
