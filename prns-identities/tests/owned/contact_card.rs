use super::*;

#[test]
fn paused_named_identity_issues_cards_without_services_or_activation() {
    let root = Root::new();
    let owner = Only::create_new(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    let client = owner.client();
    let identity = create(&client, 61, ServiceSet::empty());
    let before = wait(identity.snapshot()).unwrap();
    let catalog_before = std::fs::read(root.0.join("catalog.bin")).unwrap();
    let service = prns_contact_card::service_name_hash("example.telemetry");
    let wire = wait(identity.contact_card(service, "A sensor".into())).unwrap();
    let card = prns_contact_card::verify(&wire).unwrap();
    assert_eq!(card.identity_hash(), &identity.reference().public_hash.0);
    assert_eq!(card.service_name_hash(), &service);
    assert_eq!(card.display_name(), "A sensor");
    assert_eq!(before.identity.desired, DesiredState::Paused);
    assert!(before.identity.services.is_empty());
    assert_eq!(wait(identity.snapshot()).unwrap().identity, before.identity);
    assert_eq!(
        std::fs::read(root.0.join("catalog.bin")).unwrap(),
        catalog_before
    );
    // Losing a submitted result does not require a durable operation journal:
    // ordinary Ed25519 signing is deterministic and creates no domain effects.
    drop(admit(identity.contact_card(service, "A sensor".into())));
    assert_eq!(
        wait(identity.contact_card(service, "A sensor".into())).unwrap(),
        wire
    );
    let wrong = client
        .identity(IdentityReference {
            public_hash: PublicIdentityHash([81; 16]),
            ..identity.reference()
        })
        .unwrap();
    assert!(matches!(
        wait(wrong.contact_card(service, "A sensor".into())),
        Err(ApiError::Authority {
            reason: Fault::IdentityChanged
        })
    ));
    let (token, prepared) = begin(&client, &identity, TransitionTarget::Retire);
    assert!(matches!(
        completed(&client, token, prepared),
        OperationResult::Retired { .. }
    ));
    assert!(matches!(
        wait(identity.contact_card(service, "A sensor".into())),
        Err(ApiError::Authority {
            reason: Fault::Retired
        })
    ));
    wait(owner.close()).unwrap();
    assert!(wait(identity.contact_card(service, "A sensor".into())).is_err());
}

#[test]
fn card_issuance_never_replaces_missing_key_material() {
    let root = Root::new();
    let owner = Only::create_new(&root.0, options(), |_| Ok(IdentityOnly)).unwrap();
    let client = owner.client();
    let identity = create(&client, 62, ServiceSet::empty());
    let path = std::fs::read_dir(root.0.join("keys"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "key"))
        .unwrap();
    let catalog_before = std::fs::read(root.0.join("catalog.bin")).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        wait(identity.contact_card(
            prns_contact_card::service_name_hash("example.telemetry"),
            "A sensor".into()
        )),
        Err(ApiError::Storage {
            reason: StorageFailure::MissingKey
        })
    ));
    assert!(!path.exists());
    assert_eq!(
        std::fs::read(root.0.join("catalog.bin")).unwrap(),
        catalog_before
    );
    wait(owner.close()).unwrap();
}
