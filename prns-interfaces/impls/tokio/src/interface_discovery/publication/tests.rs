use prns_core::identity::in_memory::InMemoryNodeIdentity;
use prns_core::identity::{IdentitySigner, RemoteIdentity};
use prns_core::interface_discovery::{
    decode_envelope, AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails,
    DiscoveryEnvelopeBody, GeographicLocation, StampCost,
};
use prns_core::units::DurationMillis;
use prns_core::wire::TransportId;

use super::*;

fn advertisement() -> DiscoveryAdvertisement {
    DiscoveryAdvertisement {
        interface_type: AdvertisedInterfaceType::Backbone,
        transport: AdvertisedTransport::Enabled(TransportId::new([0x44; 16])),
        name: Some(String::from("Public backbone")),
        location: GeographicLocation::UNKNOWN,
        details: AdvertisementDetails::Reachable {
            host: String::from("router.example"),
            port: 4242,
        },
        published_ifac: None,
    }
}

fn registration(
    interface: InterfaceId,
    security: DiscoveryPublicationSecurity,
) -> DiscoveryPublicationRegistration {
    DiscoveryPublicationRegistration {
        interface,
        interval: DurationMillis(1),
        stamp_cost: StampCost::new(1).expect("one is a valid stamp cost"),
        security,
    }
}

#[test]
fn construction_rejects_empty_invalid_and_unencryptable_schedules() {
    let destination = DestinationHash::new([0x11; 16]);
    assert!(matches!(
        TokioInterfaceDiscoveryPublisher::new(destination, [], None),
        Err(TokioDiscoveryPublisherConstructionError::NoPublications)
    ));
    let interface = InterfaceId::new([0x22; 8]);
    assert!(matches!(
        TokioInterfaceDiscoveryPublisher::new(
            destination,
            [registration(
                interface,
                DiscoveryPublicationSecurity::NetworkEncrypted,
            )],
            None,
        ),
        Err(
            TokioDiscoveryPublisherConstructionError::MissingNetworkIdentity {
                interface: missing,
            }
        ) if missing == interface
    ));
    assert!(matches!(
        TokioInterfaceDiscoveryPublisher::new(
            destination,
            [
                registration(interface, DiscoveryPublicationSecurity::Plaintext),
                registration(interface, DiscoveryPublicationSecurity::Plaintext),
            ],
            None,
        ),
        Err(TokioDiscoveryPublisherConstructionError::InvalidSchedule(
            DiscoveryPublicationScheduleError::DuplicateInterface { .. }
        ))
    ));
}

#[derive(Debug, PartialEq, Eq)]
enum ObservedPublication {
    Prepared { attempts: u64, cache_hit: bool },
    Announced,
}

#[tokio::test]
async fn cadence_reuses_the_validated_stamp_and_sends_one_publication_at_a_time() {
    let interface = InterfaceId::new([0x33; 8]);
    let publisher = TokioInterfaceDiscoveryPublisher::with_job_interval(
        DestinationHash::new([0x11; 16]),
        [registration(
            interface,
            DiscoveryPublicationSecurity::Plaintext,
        )],
        None,
        Duration::from_millis(2),
    )
    .expect("the publisher constructs");
    let cancellation = PublicationCancellation::new();
    let task_cancellation = cancellation.clone();
    let (app_data_tx, mut app_data_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(publisher.run_with(
        TokioHost::new(),
        |_| async { Ok::<_, ()>(advertisement()) },
        move |app_data| {
            let app_data_tx = app_data_tx.clone();
            async move {
                app_data_tx
                    .send(app_data)
                    .expect("the app-data observer remains open");
                Ok::<_, AnnounceNowError>(())
            }
        },
        move |event| match event {
            TokioDiscoveryPublicationEvent::Prepared {
                stamp_attempts,
                cache_hit,
                ..
            } => {
                event_tx
                    .send(ObservedPublication::Prepared {
                        attempts: stamp_attempts,
                        cache_hit,
                    })
                    .expect("the event observer remains open");
            }
            TokioDiscoveryPublicationEvent::Announced { .. } => {
                event_tx
                    .send(ObservedPublication::Announced)
                    .expect("the event observer remains open");
            }
            TokioDiscoveryPublicationEvent::AdvertisementUnavailable { .. }
            | TokioDiscoveryPublicationEvent::PreparationFailed { .. }
            | TokioDiscoveryPublicationEvent::FramingFailed { .. }
            | TokioDiscoveryPublicationEvent::AnnounceFailed { .. } => {
                panic!("the valid publication should not fail");
            }
        },
        task_cancellation,
    ));
    let mut observed = Vec::new();
    while observed.len() < 4 {
        observed.push(
            tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
                .await
                .expect("the publisher reports within the window")
                .expect("the publisher remains active"),
        );
    }
    let first = tokio::time::timeout(Duration::from_secs(2), app_data_rx.recv())
        .await
        .expect("the first app data arrives")
        .expect("the publisher remains active");
    let second = tokio::time::timeout(Duration::from_secs(2), app_data_rx.recv())
        .await
        .expect("the second app data arrives")
        .expect("the publisher remains active");
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("the publisher cancels within the window")
        .expect("the publisher task joins");
    assert!(matches!(
        observed.as_slice(),
        [
            ObservedPublication::Prepared {
                attempts,
                cache_hit: false,
            },
            ObservedPublication::Announced,
            ObservedPublication::Prepared {
                attempts: 0,
                cache_hit: true,
            },
            ObservedPublication::Announced,
        ] if *attempts > 0
    ));
    assert_eq!(first, second);
}

#[tokio::test]
async fn network_encrypted_publication_uses_fresh_host_entropy_and_is_decryptable() {
    let secret = [0x42; prns_core::identity::IDENTITY_SECRET_KEY_LEN];
    let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
    let remote = RemoteIdentity::from_public_keys(
        identity.encryption_public_key(),
        identity.signing_public_key(),
    );
    let interface = InterfaceId::new([0x55; 8]);
    let publisher = TokioInterfaceDiscoveryPublisher::with_job_interval(
        DestinationHash::new([0x11; 16]),
        [registration(
            interface,
            DiscoveryPublicationSecurity::NetworkEncrypted,
        )],
        Some(remote),
        Duration::from_millis(1),
    )
    .expect("the encrypted publisher constructs");
    let cancellation = PublicationCancellation::new();
    let task_cancellation = cancellation.clone();
    let (app_data_tx, mut app_data_rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(publisher.run_with(
        TokioHost::new(),
        |_| async { Ok::<_, ()>(advertisement()) },
        move |app_data| {
            let app_data_tx = app_data_tx.clone();
            async move {
                app_data_tx
                    .send(app_data)
                    .expect("the app-data observer remains open");
                Ok::<_, AnnounceNowError>(())
            }
        },
        |_| {},
        task_cancellation,
    ));
    let app_data = tokio::time::timeout(Duration::from_secs(2), app_data_rx.recv())
        .await
        .expect("the encrypted app data arrives")
        .expect("the publisher remains active");
    cancellation.cancel();
    task.await.expect("the publisher task joins");
    let envelope = decode_envelope(&app_data).expect("the encrypted envelope decodes");
    let DiscoveryEnvelopeBody::Encrypted { ciphertext } = envelope.body else {
        panic!("the network publication should be encrypted");
    };
    let mut plaintext = vec![0; ciphertext.len()];
    let written = identity
        .decrypt(ciphertext, &mut plaintext)
        .expect("the network identity decrypts its publication");
    assert!(written > prns_core::interface_discovery::STAMP_SIZE);
}
