use super::*;

fn characteristic(
    uuid: &objc2_core_bluetooth::CBUUID,
    properties: CBCharacteristicProperties,
) -> Retained<CBMutableCharacteristic> {
    // SAFETY: this constructs ordinary retained metadata without a Bluetooth manager or radio.
    unsafe {
        CBMutableCharacteristic::initWithType_properties_value_permissions(
            CBMutableCharacteristic::alloc(),
            uuid,
            properties,
            None,
            CBAttributePermissions::Readable | CBAttributePermissions::Writeable,
        )
    }
}

fn service(liveness: bool) -> Retained<CBMutableService> {
    let writable = CBCharacteristicProperties::Write | CBCharacteristicProperties::Notify;
    let mut characteristics = vec![
        characteristic(&control_uuid(), writable),
        characteristic(&data_uuid(), writable),
        characteristic(&columba_rx_uuid(), CBCharacteristicProperties::Write),
        characteristic(
            &columba_tx_uuid(),
            CBCharacteristicProperties::Read | CBCharacteristicProperties::Notify,
        ),
        characteristic(&columba_identity_uuid(), CBCharacteristicProperties::Read),
    ];
    if liveness {
        characteristics.push(characteristic(
            &liveness_uuid(),
            CBCharacteristicProperties::Read,
        ));
    }
    // SAFETY: the fresh service and its immutable UUID are retained for initialization.
    let service = unsafe {
        CBMutableService::initWithType_primary(CBMutableService::alloc(), &service_uuid(), true)
    };
    let references: Vec<&CBCharacteristic> =
        characteristics.iter().map(|value| &**value as _).collect();
    let values = NSArray::from_slice(&references);
    // SAFETY: this test-owned service is unpublished, and each characteristic is retained.
    unsafe { service.setCharacteristics(Some(&values)) };
    service
}

#[test]
fn restored_profile_inspects_real_service_metadata_without_mutating_it() {
    for (liveness, expected) in [
        (false, RestoredProfile::Legacy),
        (true, RestoredProfile::Current),
    ] {
        let service = service(liveness);
        // SAFETY: the test owns this ordinary, unpublished service metadata.
        let before = unsafe { service.characteristics() }.unwrap();
        let (profile, restored) = restored_characteristics(&service).unwrap();
        assert_eq!(profile, expected);
        assert_eq!(restored.len(), before.len());
        for characteristic in before.iter() {
            assert!(restored
                .values()
                .any(|retained| core::ptr::eq(&*characteristic, &**retained as &CBCharacteristic)));
        }
    }
}

#[test]
fn same_uuid_old_attribute_cannot_write_or_unsubscribe_a_current_owner() {
    for uuid in [
        control_uuid(),
        data_uuid(),
        columba_rx_uuid(),
        columba_tx_uuid(),
        liveness_uuid(),
    ] {
        let old = characteristic(&uuid, CBCharacteristicProperties::Read);
        let current = characteristic(&uuid, CBCharacteristicProperties::Read);
        assert!(!current_attribute_matches(true, &old, &current));
        assert!(!current_attribute_matches(false, &current, &current));
        assert!(current_attribute_matches(true, &current, &current));
    }
}

#[test]
fn malformed_restoration_is_not_silently_migrated() {
    let malformed = service(true);
    // SAFETY: this unpublished metadata is owned only by the test.
    let characteristics = unsafe { malformed.characteristics() }.unwrap();
    let retained: Vec<_> = characteristics
        .iter()
        .filter(|characteristic| {
            // SAFETY: the array owns this characteristic during its immutable UUID query.
            let uuid = unsafe { characteristic.UUID() };
            !cbuuid_eq(&uuid, &data_uuid())
        })
        .collect();
    let missing_data: Vec<&CBCharacteristic> = retained.iter().map(|value| &**value).collect();
    let values = NSArray::from_slice(&missing_data);
    // SAFETY: test-owned unpublished metadata may have its characteristic list replaced.
    unsafe { malformed.setCharacteristics(Some(&values)) };
    assert!(restored_characteristics(&malformed).is_none());
}

#[test]
fn restoration_selects_only_exact_prns_service_and_refuses_duplicates() {
    let current = service(true);
    let legacy = service(false);
    // SAFETY: this unrelated unpublished service is local test metadata.
    let unrelated = unsafe {
        CBMutableService::initWithType_primary(CBMutableService::alloc(), &data_uuid(), true)
    };
    let values = NSArray::from_slice(&[&*unrelated as &CBService, &*current]);
    let selected = restored_prns_service(&values).unwrap().unwrap();
    assert!(core::ptr::eq(&*selected, &*current));
    let duplicate = NSArray::from_slice(&[&*current as &CBService, &*legacy]);
    assert_eq!(
        restored_prns_service(&duplicate).unwrap_err(),
        RestoredServiceError::Multiple
    );
    let only_unrelated = NSArray::from_slice(&[&*unrelated as &CBService]);
    assert!(restored_prns_service(&only_unrelated).unwrap().is_none());
}
