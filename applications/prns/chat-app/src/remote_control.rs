//! Product selection for the canonical host RemoteControl service.
//!
//! Bootstrap owns explicit key creation, pinned identity validation, paths and
//! durable storage. This module only configures the existing host and retains
//! the admitted native key capabilities through its actual cleanup.
use prns_core::remote_control::{RemoteControlCapabilities, RemoteControlSelfAnnouncement};
use prns_host::{IdentityConfig, IdentitySecret};
use prns_host_runtime::{remote_control_service::HostRemoteControlConfig, HostEmbedding};
use prns_service_runtime_uniffi::identity::keys::KeyLease;

pub(crate) fn configure(embedding: &mut HostEmbedding, controller: KeyLease, target: KeyLease) {
    embedding.remote_control_config = Some(HostRemoteControlConfig {
        controller_identity: IdentityConfig::Existing(IdentitySecret::new(
            *controller.material().as_bytes(),
        )),
        target_identity: IdentityConfig::Existing(IdentitySecret::new(
            *target.material().as_bytes(),
        )),
        initial_controller_grants: vec![],
        self_announcement: RemoteControlSelfAnnouncement::Unavailable,
        capabilities: RemoteControlCapabilities::describe_only(),
    });
    let mut previous = embedding.on_event.take();
    embedding.on_event = Some(Box::new(move |event| {
        // The callback is retained by the actual host, including partial startup
        // and cleanup after its caller drops. Foreign leases expose no key bytes.
        let _retain_keys = (&controller, &target);
        if let Some(callback) = &mut previous {
            callback(event);
        }
    }));
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use prns_service_runtime_uniffi::identity::{keys::KeyOwner, Error};
    use prns_storage::files;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn configuration_retains_both_exclusive_keys_and_existing_callback() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "prns-app-remote-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        files::create_private_directory(&root).unwrap();
        let controller_path = root.join("controller");
        let target_path = root.join("target");
        let controller = KeyOwner::create_new(&controller_path).unwrap();
        let target = KeyOwner::create_new(&target_path).unwrap();
        let hashes = (controller.identity_hash(), target.identity_hash());
        assert_ne!(hashes.0, hashes.1);
        let previous = Arc::new(());
        let retained = previous.clone();
        let mut embedding = HostEmbedding {
            on_event: Some(Box::new(move |_| {
                let _retain = &retained;
            })),
            ..HostEmbedding::default()
        };
        configure(&mut embedding, controller.lease(), target.lease());
        drop((controller, target));
        let config = embedding.remote_control_config.as_ref().unwrap();
        assert!(config.initial_controller_grants.is_empty());
        assert_eq!(
            config.capabilities,
            RemoteControlCapabilities::describe_only()
        );
        assert!(matches!(
            config.self_announcement,
            RemoteControlSelfAnnouncement::Unavailable
        ));
        assert!(matches!(
            config.controller_identity,
            IdentityConfig::Existing(_)
        ));
        assert!(matches!(
            config.target_identity,
            IdentityConfig::Existing(_)
        ));
        assert_eq!(Arc::strong_count(&previous), 2);
        assert!(matches!(
            KeyOwner::open_existing(&controller_path, hashes.0),
            Err(Error::Busy)
        ));
        assert!(matches!(
            KeyOwner::open_existing(&target_path, hashes.1),
            Err(Error::Busy)
        ));
        drop(embedding);
        assert_eq!(Arc::strong_count(&previous), 1);
        let controller = KeyOwner::open_existing(&controller_path, hashes.0).unwrap();
        let target = KeyOwner::open_existing(&target_path, hashes.1).unwrap();
        drop((controller, target));
        std::fs::remove_dir_all(root).unwrap();
    }
}
