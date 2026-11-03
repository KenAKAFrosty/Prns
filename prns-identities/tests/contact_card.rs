#![cfg(all(unix, feature = "contact-card"))]
#![allow(clippy::unwrap_used)]
use prns_identities::keys::KeyOwner;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "key-card-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn standalone_lease_issues_generic_service_cards_without_catalog_or_host() {
    let root = Root::new();
    let owner = KeyOwner::create_new(&root.0).unwrap();
    let identity = owner.identity_hash();
    let public = owner.public_identity();
    let lease = owner.lease();
    drop(owner);
    assert!(KeyOwner::open_existing(&root.0, identity).is_err());
    for service in [
        "example.telemetry",
        "prns.groups.enrollment",
        "lxmf.delivery",
    ] {
        let hash = prns_contact_card::service_name_hash(service);
        let wire = lease.contact_card(hash, "Device α").unwrap();
        let card = prns_contact_card::verify(&wire).unwrap();
        assert_eq!(card.identity_hash(), identity.as_bytes());
        assert_eq!(card.public_identity(), public.as_bytes());
        assert_eq!(card.service_name_hash(), &hash);
        assert_eq!(card.display_name(), "Device α");
        assert_eq!(lease.contact_card(hash, "Device α").unwrap(), wire);
    }
    assert!(!root.0.join("catalog.bin").exists());
    drop(lease);
    let reopened = KeyOwner::open_existing(&root.0, identity).unwrap();
    assert_eq!(reopened.public_identity(), public);
}

#[test]
fn invalid_claim_name_preserves_the_key_and_uses_the_codec_rules() {
    let root = Root::new();
    let owner = KeyOwner::create_new(&root.0).unwrap();
    let before = std::fs::read(root.0.join("identity.key")).unwrap();
    let service = prns_contact_card::service_name_hash("example.telemetry");
    for name in ["a\nb".to_owned(), "a\u{202e}b".to_owned(), "x".repeat(129)] {
        assert!(matches!(
            owner.lease().contact_card(service, &name),
            Err(prns_identities_core::ApiError::InvalidInput)
        ));
    }
    assert_eq!(std::fs::read(root.0.join("identity.key")).unwrap(), before);
}
