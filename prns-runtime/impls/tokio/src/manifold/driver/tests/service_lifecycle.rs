use super::*;
use crate::crypto::ratchets::RatchetPolicy;
use crate::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget};
use crate::identity::{vault::IdentitySecretKey, Zeroizing};
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use crate::runtime::{ServiceLifecycleError, ServiceRegistration};

fn registration(fill: u8, aspect: &str) -> ServiceRegistration {
    ServiceRegistration {
        initially_accepting: true,
        secret: IdentitySecretKey::new([fill; 64]),
        app_name: "identities".into(),
        aspects: vec![aspect.into()],
        app_data: vec![],
        proof_strategy: ProofStrategy::ProveAll,
        link_request_policy: LinkRequestPolicy::AcceptAll,
        ratchet_policy: RatchetPolicy::Ratcheted,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: crate::units::ByteLimit::Maximum(1024),
        request_handlers: vec![],
        restore_ratchets: None,
    }
}
async fn announce(handle: &PrnsNodeHandle, destination: DestinationHash) {
    handle
        .announce_now(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn live_service_pause_waits_for_old_frames_preserves_peer_and_restores_ratchets() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let id = InterfaceId::new([0xEA; 8]);
        let (_wake, wake_rx) = manifold_wake();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let handle = PrnsNodeHandle::over(commands.clone());
        let (producer, mut consumer) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let node = tokio::spawn(run(
            EngineState::<TestStorageLayout>::default(),
            TokioHost::new(),
            ManifoldWiring {
                interfaces: vec![descriptor(id)],
                ifacs: vec![],
                wake: wake_rx,
                inbound_lanes: vec![],
                commands: command_rx,
                egress: Egress::new(vec![(id, producer)]),
            },
            |_| {},
        ));
        let first = handle.service_endpoint();
        let second = handle.service_endpoint();
        let spec = registration(0x31, "first");
        let a = first.register(spec.clone()).await.unwrap();
        assert_eq!(first.register(spec.clone()).await.unwrap(), a);
        let b = second.register(registration(0x32, "second")).await.unwrap();
        assert_ne!(a, b);
        assert_eq!(
            handle.service_endpoint().register(spec.clone()).await,
            Err(ServiceLifecycleError::Conflict)
        );
        announce(&handle, a).await;
        let pause = first.prepare_pause(vec![]).await.unwrap();
        assert!(!pause.is_drained());
        assert_eq!(
            first.finish_pause(&pause).await,
            Err(ServiceLifecycleError::NotDrained)
        );
        let repeated = first.prepare_pause(vec![]).await.unwrap();
        assert_eq!(pause.ratchets(), repeated.ratchets());
        let saved = Zeroizing::new(pause.ratchets().unwrap().to_vec());
        assert_eq!(
            crate::persistence::read_self_ratchets_snapshot(&saved)
                .unwrap()
                .secret_count(),
            1
        );
        // Unrelated service keeps generating traffic behind the finite fence.
        announce(&handle, b).await;
        let header = WirePacketHeader::parse(consumer.peek().await.frame())
            .unwrap()
            .0;
        assert_eq!(header.address, a.to_address());
        consumer.release();
        assert!(pause.is_drained());
        assert!(
            consumer.try_peek().is_some(),
            "other identity's frame remains queued"
        );
        first.finish_pause(&pause).await.unwrap();
        first.finish_pause(&pause).await.unwrap();
        consumer.release();
        let mut restored = spec;
        restored.restore_ratchets = Some(saved);
        assert_eq!(first.register(restored).await.unwrap(), a);
        assert_eq!(
            first.finish_pause(&pause).await,
            Err(ServiceLifecycleError::WrongPhase)
        );
        let restored_pause = first.prepare_pause(vec![]).await.unwrap();
        assert_eq!(restored_pause.ratchets(), pause.ratchets());
        assert_eq!(
            first.finish_pause(&pause).await,
            Err(ServiceLifecycleError::Conflict)
        );
        first.finish_pause(&restored_pause).await.unwrap();
        // Releasing first key has not disabled another service.
        announce(&handle, b).await;
        assert_eq!(
            WirePacketHeader::parse(consumer.peek().await.frame())
                .unwrap()
                .0
                .address,
            b.to_address()
        );
        consumer.release();
        let final_pause = second.prepare_pause(vec![]).await.unwrap();
        second.finish_pause(&final_pause).await.unwrap();
        node.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn service_registration_failure_is_atomic_and_changed_retry_is_rejected() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (_wake, wake_rx) = manifold_wake();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let handle = PrnsNodeHandle::over(commands.clone());
        let node = tokio::spawn(run(
            EngineState::<TestStorageLayout>::default(),
            TokioHost::new(),
            ManifoldWiring {
                interfaces: vec![],
                ifacs: vec![],
                wake: wake_rx,
                inbound_lanes: vec![],
                commands: command_rx,
                egress: Egress::new(vec![]),
            },
            |_| {},
        ));
        let endpoint = handle.service_endpoint();
        assert!(matches!(
            endpoint.prepare_pause(vec![]).await,
            Err(ServiceLifecycleError::NotRegistered)
        ));
        let mut invalid = registration(0x41, "first");
        invalid.restore_ratchets = Some(Zeroizing::new(vec![1, 2, 3]));
        assert_eq!(
            endpoint.register(invalid).await,
            Err(ServiceLifecycleError::InvalidRatchets)
        );
        let mut spec = registration(0x41, "first");
        spec.initially_accepting = false;
        // Lose the acknowledgement after dispatch; retry must recover the same
        // manifold entry rather than adopt a conflicting destination.
        let (reply, receive) = tokio::sync::oneshot::channel();
        drop(receive);
        commands
            .send(HostCommand::ServiceLifecycle(
                crate::runtime::node_facade::service_lifecycle::ServiceCommand::Register {
                    endpoint: endpoint.id,
                    registration: Box::new(spec.clone()),
                    reply,
                },
            ))
            .unwrap();
        let destination = endpoint.register(spec.clone()).await.unwrap();
        assert!(handle
            .announce_now(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered
            })
            .await
            .is_err());
        endpoint.activate_ingress().await.unwrap();
        endpoint.activate_ingress().await.unwrap();
        announce(&handle, destination).await;
        let mut changed = spec;
        changed.maximum_request_bytes = crate::units::ByteLimit::Unlimited;
        assert_eq!(
            endpoint.register(changed).await,
            Err(ServiceLifecycleError::Conflict)
        );
        let pause = endpoint.prepare_pause(vec![]).await.unwrap();
        assert_eq!(
            endpoint.activate_ingress().await,
            Err(ServiceLifecycleError::WrongPhase)
        );
        assert!(pause.is_drained());
        endpoint.finish_pause(&pause).await.unwrap();
        node.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn service_key_ownership_survives_sibling_pause_and_reclaims_bounded_capacity() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (_wake, wake_rx) = manifold_wake();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let handle = PrnsNodeHandle::over(commands);
        let node = tokio::spawn(run(
            EngineState::<TestStorageLayout>::default(),
            TokioHost::new(),
            ManifoldWiring {
                interfaces: vec![],
                ifacs: vec![],
                wake: wake_rx,
                inbound_lanes: vec![],
                commands: command_rx,
                egress: Egress::new(vec![]),
            },
            |_| {},
        ));
        let mut spec = registration(0x71, "one");
        spec.ratchet_policy = RatchetPolicy::NoRatchets;
        let first = handle.service_endpoint();
        let second = handle.service_endpoint();
        first.register(spec.clone()).await.unwrap();
        spec.aspects = vec!["two".into()];
        let sibling = second.register(spec).await.unwrap();
        let pause = first.prepare_pause(vec![]).await.unwrap();
        first.finish_pause(&pause).await.unwrap();
        // Signing still needs the shared key after its original owner retires.
        announce(&handle, sibling).await;
        let pause = second.prepare_pause(vec![]).await.unwrap();
        second.finish_pause(&pause).await.unwrap();
        // More unique keys than fixed test held-key capacity must fit over time.
        for fill in 1..=32 {
            let endpoint = handle.service_endpoint();
            let mut spec = registration(fill, "cycled");
            spec.ratchet_policy = RatchetPolicy::NoRatchets;
            endpoint.register(spec).await.unwrap();
            let pause = endpoint.prepare_pause(vec![]).await.unwrap();
            endpoint.finish_pause(&pause).await.unwrap();
        }
        node.abort();
    })
    .await
    .unwrap();
}
