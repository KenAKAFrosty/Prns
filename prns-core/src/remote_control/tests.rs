use ::core::convert::Infallible;

use super::*;
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::entropy::{EntropySource, RuntimeEntropy};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey, Zeroizing,
    IDENTITY_SECRET_KEY_LEN,
};
use proptest::prelude::*;

struct TestEntropySource(u8);

impl EntropySource for TestEntropySource {
    type Error = Infallible;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        output.fill(self.0);
        Ok(())
    }
}

fn runtime_entropy(seed: u8) -> RuntimeEntropy<TestEntropySource> {
    RuntimeEntropy::try_new(TestEntropySource(seed)).unwrap()
}

fn controller_identity(fill: u8) -> RemoteControlControllerIdentity {
    RemoteControlControllerIdentity::new(IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
    })
}

fn grant(fill: u8, request: RemoteControlRequestKind) -> RemoteControlControllerGrant {
    RemoteControlControllerGrant::new(
        controller_identity(fill),
        RemoteControlControllerAuthority::Operator,
        RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

fn controller_grant_table_contract(table: &mut impl RemoteControlControllerGrantTable) {
    let first = grant(0x21, RemoteControlRequestKind::Describe);
    let updated_first = grant(0x21, RemoteControlRequestKind::AnnounceSelf);
    let second = grant(0x43, RemoteControlRequestKind::Describe);
    let first_hash = first.controller().identity_hash();
    let second_hash = second.controller().identity_hash();

    assert!(table.is_empty());
    assert_eq!(
        table.set_controller_grant(first),
        Ok(SetRemoteControlControllerGrantOutcome::Added),
    );
    assert_eq!(
        table.set_controller_grant(first),
        Ok(SetRemoteControlControllerGrantOutcome::Unchanged),
    );
    assert_eq!(
        table.set_controller_grant(updated_first),
        Ok(SetRemoteControlControllerGrantOutcome::Updated { previous: first }),
    );
    assert_eq!(table.len(), 1);
    assert_eq!(table.grant_for(&first_hash), Some(&updated_first));
    assert!(table.contains_controller(&first_hash));
    assert!(!table.contains_controller(&second_hash));
    assert_eq!(
        table.revoke_controller(second.controller()),
        RevokeRemoteControlControllerOutcome::NotFound,
    );
    assert_eq!(
        table.set_controller_grant(second),
        Ok(SetRemoteControlControllerGrantOutcome::Added),
    );
    assert_eq!(table.len(), 2);
    assert!(table
        .grants_in_identity_hash_order()
        .windows(2)
        .all(|pair| matches!(
        pair,
        [first, second]
            if first.controller().identity_hash().as_bytes()
                < second.controller().identity_hash().as_bytes()
        )));
    assert_eq!(
        table.revoke_controller(first.controller()),
        RevokeRemoteControlControllerOutcome::Revoked {
            grant: updated_first,
        },
    );
    assert_eq!(table.grants_in_identity_hash_order(), &[second]);
}

#[test]
fn fixed_table_obeys_the_controller_grant_table_contract() {
    let mut table = FixedRemoteControlControllerGrantTable::<2>::default();

    assert_eq!(table.capacity(), 2);
    controller_grant_table_contract(&mut table);
}

#[test]
fn a_full_fixed_table_refuses_only_a_new_identity() {
    let mut table = FixedRemoteControlControllerGrantTable::<1>::default();
    let first = grant(0x65, RemoteControlRequestKind::Describe);
    let updated_first = grant(0x65, RemoteControlRequestKind::AnnounceSelf);

    assert_eq!(
        table.set_controller_grant(first),
        Ok(SetRemoteControlControllerGrantOutcome::Added),
    );
    assert_eq!(
        table.set_controller_grant(updated_first),
        Ok(SetRemoteControlControllerGrantOutcome::Updated { previous: first }),
    );
    assert_eq!(
        table.set_controller_grant(grant(0x87, RemoteControlRequestKind::Describe)),
        Err(SetRemoteControlControllerGrantError::CapacityExhausted),
    );
    assert_eq!(table.grants_in_identity_hash_order(), &[updated_first]);
}

#[cfg(feature = "alloc")]
#[test]
fn heap_table_obeys_the_controller_grant_table_contract() {
    let mut table = HeapRemoteControlControllerGrantTable::default();

    assert_eq!(table.capacity(), usize::MAX);
    controller_grant_table_contract(&mut table);
}

#[test]
fn a_zero_capacity_controller_grant_table_is_an_empty_disabled_table() {
    let mut table = FixedRemoteControlControllerGrantTable::<0>::default();

    assert!(table.is_empty());
    assert_eq!(
        table.set_controller_grant(grant(0xA9, RemoteControlRequestKind::Describe)),
        Err(SetRemoteControlControllerGrantError::CapacityExhausted),
    );
}

struct OutOfOrderControllerGrantTable {
    grants: [RemoteControlControllerGrant; 2],
}

impl RemoteControlControllerGrantTable for OutOfOrderControllerGrantTable {
    fn capacity(&self) -> usize {
        self.grants.len()
    }

    fn len(&self) -> usize {
        self.grants.len()
    }

    fn grants_in_identity_hash_order(&self) -> &[RemoteControlControllerGrant] {
        &self.grants
    }

    fn set_controller_grant(
        &mut self,
        _grant: RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantError> {
        Err(SetRemoteControlControllerGrantError::CapacityExhausted)
    }

    fn revoke_controller(
        &mut self,
        _controller: &RemoteControlControllerIdentity,
    ) -> RevokeRemoteControlControllerOutcome {
        RevokeRemoteControlControllerOutcome::NotFound
    }
}

#[test]
fn controller_inventory_rejects_a_grant_table_that_breaks_its_ordering_contract() {
    let first = grant(0x31, RemoteControlRequestKind::Describe);
    let second = grant(0x32, RemoteControlRequestKind::Describe);
    let grants = if first.controller().identity_hash().as_bytes()
        < second.controller().identity_hash().as_bytes()
    {
        [second, first]
    } else {
        [first, second]
    };
    let table = OutOfOrderControllerGrantTable { grants };

    assert_eq!(
        RemoteControlControllerInventory::from_grants(&table, RemoteControlControllerPage::First,),
        Err(RemoteControlControllerInventoryError::NonAscending),
    );
}

fn target_identity(fill: u8) -> RemoteControlTargetIdentity {
    RemoteControlTargetIdentity::new(IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
    })
}

fn target_access(fill: u8, request: RemoteControlRequestKind) -> RemoteControlTargetAccess {
    RemoteControlTargetAccess::new(
        target_identity(fill),
        RemoteControlControllerAuthority::Operator,
        RemoteControlRequestSet::only(request),
    )
    .unwrap()
}

fn target_access_table_contract(table: &mut impl RemoteControlTargetAccessTable) {
    let first_identity = target_identity(0x21);
    let second_identity = target_identity(0x43);
    let first_hash = first_identity.identity_hash();
    let second_hash = second_identity.identity_hash();

    assert!(table.is_empty());
    assert_eq!(
        table.set_target_access(target_access(0x21, RemoteControlRequestKind::Describe)),
        Ok(SetRemoteControlTargetAccessOutcome::Added),
    );
    assert_eq!(
        table.set_target_access(target_access(0x21, RemoteControlRequestKind::Describe)),
        Ok(SetRemoteControlTargetAccessOutcome::Unchanged),
    );
    assert_eq!(
        table.set_target_access(target_access(0x21, RemoteControlRequestKind::AnnounceSelf)),
        Ok(SetRemoteControlTargetAccessOutcome::Updated {
            previous: target_access(0x21, RemoteControlRequestKind::Describe),
        }),
    );
    assert_eq!(table.len(), 1);
    let expected = target_access(0x21, RemoteControlRequestKind::AnnounceSelf);
    assert_eq!(table.access_for(&first_hash), Some(&expected));
    assert!(table.contains_target(&first_hash));
    assert!(!table.contains_target(&second_hash));
    assert_eq!(
        table.forget_target(&second_identity),
        ForgetRemoteControlTargetOutcome::NotFound,
    );
    assert_eq!(
        table.set_target_access(target_access(0x43, RemoteControlRequestKind::Describe)),
        Ok(SetRemoteControlTargetAccessOutcome::Added),
    );
    assert_eq!(table.len(), 2);
    assert!(table
        .accesses_in_identity_hash_order()
        .windows(2)
        .all(|pair| matches!(
        pair,
        [first, second]
            if first.target().identity_hash().as_bytes()
                < second.target().identity_hash().as_bytes()
        )));
    assert_eq!(
        table.forget_by_identity_hash(&first_hash),
        ForgetRemoteControlTargetOutcome::Forgotten { access: expected },
    );
    assert_eq!(
        table.accesses_in_identity_hash_order(),
        &[target_access(0x43, RemoteControlRequestKind::Describe)],
    );
}

#[test]
fn fixed_table_obeys_the_target_access_table_contract() {
    let mut table = FixedRemoteControlTargetAccessTable::<2>::default();

    assert_eq!(table.capacity(), 2);
    target_access_table_contract(&mut table);
}

#[test]
fn a_full_fixed_target_access_table_refuses_only_a_new_identity() {
    let mut table = FixedRemoteControlTargetAccessTable::<1>::default();

    assert_eq!(
        table.set_target_access(target_access(0x65, RemoteControlRequestKind::Describe)),
        Ok(SetRemoteControlTargetAccessOutcome::Added),
    );
    assert_eq!(
        table.set_target_access(target_access(0x65, RemoteControlRequestKind::AnnounceSelf)),
        Ok(SetRemoteControlTargetAccessOutcome::Updated {
            previous: target_access(0x65, RemoteControlRequestKind::Describe),
        }),
    );
    assert_eq!(
        table.set_target_access(target_access(0x87, RemoteControlRequestKind::Describe)),
        Err(SetRemoteControlTargetAccessError::CapacityExhausted),
    );
    assert_eq!(
        table.accesses_in_identity_hash_order(),
        &[target_access(0x65, RemoteControlRequestKind::AnnounceSelf)],
    );
}

#[cfg(feature = "alloc")]
#[test]
fn heap_table_obeys_the_target_access_table_contract() {
    let mut table = HeapRemoteControlTargetAccessTable::default();

    assert_eq!(table.capacity(), usize::MAX);
    target_access_table_contract(&mut table);
}

#[test]
fn a_zero_capacity_target_access_table_is_an_empty_disabled_table() {
    let mut table = FixedRemoteControlTargetAccessTable::<0>::default();

    assert!(table.is_empty());
    assert_eq!(
        table.set_target_access(target_access(0xA9, RemoteControlRequestKind::Describe)),
        Err(SetRemoteControlTargetAccessError::CapacityExhausted),
    );
}

fn identity_secret(fill: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([fill; IDENTITY_SECRET_KEY_LEN])
}

#[test]
fn controller_and_target_secret_brands_derive_their_public_identities() {
    let controller_secret = identity_secret(0x31);
    let controller_parts =
        InMemoryNodeIdentity::from_secret_key_bytes(&controller_secret).into_parts();
    let expected_controller = IdentityPublicKeys {
        encryption: controller_parts.encryption_public,
        signing: controller_parts.signing_public,
    };
    let target_secret = identity_secret(0x42);
    let target_parts = InMemoryNodeIdentity::from_secret_key_bytes(&target_secret).into_parts();
    let expected_target = IdentityPublicKeys {
        encryption: target_parts.encryption_public,
        signing: target_parts.signing_public,
    };
    let secrets = RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(controller_secret),
        RemoteControlTargetIdentitySecret::from(target_secret),
    )
    .unwrap();
    let identities = secrets.identities();

    assert_eq!(identities.controller().public_keys(), &expected_controller);
    assert_eq!(identities.target().public_keys(), &expected_target);
    assert_ne!(
        identities.controller().identity_hash(),
        identities.target().identity_hash(),
    );
}

#[test]
fn one_identity_cannot_fill_both_remote_control_positions() {
    let controller = RemoteControlControllerIdentitySecret::from(identity_secret(0x31));
    let target = RemoteControlTargetIdentitySecret::from(identity_secret(0x31));

    assert!(matches!(
        RemoteControlNodeIdentitySecrets::new(controller, target),
        Err(RemoteControlNodeIdentitySecretsError::ControllerAndTargetAreSameIdentity),
    ));
}

#[test]
fn generated_node_identity_secrets_fill_both_roles_once() {
    let secrets =
        RemoteControlNodeIdentitySecrets::generate_with_runtime_entropy(&mut runtime_entropy(0x30))
            .unwrap();
    let identities = secrets.identities();

    assert_ne!(
        identities.controller().identity_hash(),
        identities.target().identity_hash(),
    );
}

#[test]
fn target_sealing_keys_are_deterministic_and_domain_separated() {
    let first = RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(identity_secret(0x31)),
        RemoteControlTargetIdentitySecret::from(identity_secret(0x42)),
    )
    .unwrap();
    let second = RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(identity_secret(0x31)),
        RemoteControlTargetIdentitySecret::from(identity_secret(0x42)),
    )
    .unwrap();

    let first_wifi = first.target_sealing_key(b"wifi-configuration/v1");
    let second_wifi = second.target_sealing_key(b"wifi-configuration/v1");
    let other_domain = second.target_sealing_key(b"not-wifi-configuration/v1");

    assert_eq!(first_wifi.as_bytes(), second_wifi.as_bytes());
    assert_ne!(first_wifi.as_bytes(), other_domain.as_bytes());
}

#[test]
fn protocol_discriminants_are_stable_typed_values() {
    assert_eq!(
        RemoteControlProtocolVersion::ALL,
        [RemoteControlProtocolVersion::V1],
    );
    assert_eq!(
        RemoteControlRequestKind::ALL,
        [
            RemoteControlRequestKind::Describe,
            RemoteControlRequestKind::AnnounceSelf,
            RemoteControlRequestKind::InventoryInterfaces,
            RemoteControlRequestKind::SetInterfacePower,
            RemoteControlRequestKind::SleepRadios,
            RemoteControlRequestKind::WakeRadios,
            RemoteControlRequestKind::SetInterfaceMode,
            RemoteControlRequestKind::SetInterfaceGroup,
            RemoteControlRequestKind::InventoryInterfacePeers,
            RemoteControlRequestKind::InventoryInterfaceConfig,
            RemoteControlRequestKind::SetInterfaceLoRaProfile,
            RemoteControlRequestKind::DescribeBuild,
            RemoteControlRequestKind::SetInterfaceWifiStation,
            RemoteControlRequestKind::InventoryControllers,
            RemoteControlRequestKind::AuthorizeController,
            RemoteControlRequestKind::RevokeController,
            RemoteControlRequestKind::DescribePower,
            RemoteControlRequestKind::SetSystemPower,
            RemoteControlRequestKind::SetGnssPower,
            RemoteControlRequestKind::SetDisplayVisibility,
            RemoteControlRequestKind::SetDisplayAutoOff,
            RemoteControlRequestKind::SetStationUplink,
            RemoteControlRequestKind::SetEspRadioMode,
            RemoteControlRequestKind::StageWifiCredentials,
            RemoteControlRequestKind::ActivateWifiCredentials,
            RemoteControlRequestKind::ConfirmWifiCredentials,
            RemoteControlRequestKind::CancelWifiCredentials,
            RemoteControlRequestKind::InspectWifiTransaction,
            RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,
            RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
            RemoteControlRequestKind::InventoryPathTable,
        ],
    );
    assert_eq!(
        RemoteControlResponseKind::ALL,
        [
            RemoteControlResponseKind::Describe,
            RemoteControlResponseKind::AnnounceSelf,
            RemoteControlResponseKind::InventoryInterfaces,
            RemoteControlResponseKind::SetInterfacePower,
            RemoteControlResponseKind::SleepRadios,
            RemoteControlResponseKind::WakeRadios,
            RemoteControlResponseKind::SetInterfaceMode,
            RemoteControlResponseKind::SetInterfaceGroup,
            RemoteControlResponseKind::InventoryInterfacePeers,
            RemoteControlResponseKind::InventoryInterfaceConfig,
            RemoteControlResponseKind::SetInterfaceLoRaProfile,
            RemoteControlResponseKind::DescribeBuild,
            RemoteControlResponseKind::SetInterfaceWifiStation,
            RemoteControlResponseKind::InventoryControllers,
            RemoteControlResponseKind::AuthorizeController,
            RemoteControlResponseKind::RevokeController,
            RemoteControlResponseKind::DescribePower,
            RemoteControlResponseKind::SetSystemPower,
            RemoteControlResponseKind::SetGnssPower,
            RemoteControlResponseKind::SetDisplayVisibility,
            RemoteControlResponseKind::SetDisplayAutoOff,
            RemoteControlResponseKind::SetStationUplink,
            RemoteControlResponseKind::SetEspRadioMode,
            RemoteControlResponseKind::StageWifiCredentials,
            RemoteControlResponseKind::ActivateWifiCredentials,
            RemoteControlResponseKind::ConfirmWifiCredentials,
            RemoteControlResponseKind::CancelWifiCredentials,
            RemoteControlResponseKind::InspectWifiTransaction,
            RemoteControlResponseKind::InventoryInterfaceDiscoveryGroups,
            RemoteControlResponseKind::ReplaceInterfaceDiscoveryGroups,
            RemoteControlResponseKind::InventoryPathTable,
            RemoteControlResponseKind::ProtocolError,
        ],
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::ALL,
        [
            RemoteControlProtocolErrorKind::MalformedRequest,
            RemoteControlProtocolErrorKind::UnsupportedVersion,
            RemoteControlProtocolErrorKind::UnknownRequestKind,
            RemoteControlProtocolErrorKind::UnsupportedRequest,
            RemoteControlProtocolErrorKind::Busy,
            RemoteControlProtocolErrorKind::ApplyFailed,
            RemoteControlProtocolErrorKind::PersistenceFailed,
            RemoteControlProtocolErrorKind::RollbackFailed,
            RemoteControlProtocolErrorKind::InternalFailure,
        ],
    );
    assert_eq!(RemoteControlProtocolVersion::V1.wire_value(), 0x01);
    assert_eq!(RemoteControlRequestKind::Describe.wire_value(), 0x01);
    assert_eq!(RemoteControlRequestKind::AnnounceSelf.wire_value(), 0x02);
    assert_eq!(
        RemoteControlRequestKind::InventoryInterfaces.wire_value(),
        0x03
    );
    assert_eq!(
        RemoteControlRequestKind::SetInterfacePower.wire_value(),
        0x04
    );
    assert_eq!(RemoteControlRequestKind::SleepRadios.wire_value(), 0x05);
    assert_eq!(RemoteControlRequestKind::WakeRadios.wire_value(), 0x06);
    assert_eq!(
        RemoteControlRequestKind::SetInterfaceMode.wire_value(),
        0x07
    );
    assert_eq!(
        RemoteControlRequestKind::SetInterfaceGroup.wire_value(),
        0x08
    );
    assert_eq!(
        RemoteControlRequestKind::InventoryInterfacePeers.wire_value(),
        0x09
    );
    assert_eq!(
        RemoteControlRequestKind::InventoryInterfaceConfig.wire_value(),
        0x0A
    );
    assert_eq!(
        RemoteControlRequestKind::SetInterfaceLoRaProfile.wire_value(),
        0x0B
    );
    assert_eq!(RemoteControlRequestKind::DescribeBuild.wire_value(), 0x0C);
    assert_eq!(
        RemoteControlRequestKind::SetInterfaceWifiStation.wire_value(),
        0x0D
    );
    assert_eq!(
        RemoteControlRequestKind::InventoryControllers.wire_value(),
        0x0E
    );
    assert_eq!(
        RemoteControlRequestKind::AuthorizeController.wire_value(),
        0x0F
    );
    assert_eq!(
        RemoteControlRequestKind::RevokeController.wire_value(),
        0x10
    );
    assert_eq!(RemoteControlRequestKind::DescribePower.wire_value(), 0x11);
    assert_eq!(RemoteControlResponseKind::Describe.wire_value(), 0x01);
    assert_eq!(RemoteControlResponseKind::AnnounceSelf.wire_value(), 0x02);
    assert_eq!(
        RemoteControlResponseKind::InventoryInterfaces.wire_value(),
        0x03
    );
    assert_eq!(
        RemoteControlResponseKind::SetInterfacePower.wire_value(),
        0x04
    );
    assert_eq!(RemoteControlResponseKind::SleepRadios.wire_value(), 0x05);
    assert_eq!(RemoteControlResponseKind::WakeRadios.wire_value(), 0x06);
    assert_eq!(
        RemoteControlResponseKind::SetInterfaceMode.wire_value(),
        0x07
    );
    assert_eq!(
        RemoteControlResponseKind::SetInterfaceGroup.wire_value(),
        0x08
    );
    assert_eq!(
        RemoteControlResponseKind::InventoryInterfacePeers.wire_value(),
        0x09
    );
    assert_eq!(
        RemoteControlResponseKind::InventoryInterfaceConfig.wire_value(),
        0x0A
    );
    assert_eq!(
        RemoteControlResponseKind::SetInterfaceLoRaProfile.wire_value(),
        0x0B
    );
    assert_eq!(RemoteControlResponseKind::DescribeBuild.wire_value(), 0x0C);
    assert_eq!(
        RemoteControlResponseKind::SetInterfaceWifiStation.wire_value(),
        0x0D
    );
    assert_eq!(
        RemoteControlResponseKind::InventoryControllers.wire_value(),
        0x0E
    );
    assert_eq!(
        RemoteControlResponseKind::AuthorizeController.wire_value(),
        0x0F
    );
    assert_eq!(
        RemoteControlResponseKind::RevokeController.wire_value(),
        0x10
    );
    assert_eq!(RemoteControlResponseKind::DescribePower.wire_value(), 0x11);
    assert_eq!(
        RemoteControlRequestKind::InventoryPathTable.wire_value(),
        0x1F
    );
    assert_eq!(
        RemoteControlResponseKind::InventoryPathTable.wire_value(),
        0x1F
    );
    assert_eq!(RemoteControlResponseKind::ProtocolError.wire_value(), 0xFF,);
    assert_eq!(
        RemoteControlProtocolErrorKind::MalformedRequest.wire_value(),
        0x01,
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::UnsupportedVersion.wire_value(),
        0x02,
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::UnknownRequestKind.wire_value(),
        0x03,
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::UnsupportedRequest.wire_value(),
        0x04,
    );
    assert_eq!(RemoteControlProtocolErrorKind::Busy.wire_value(), 0x05);
    assert_eq!(
        RemoteControlProtocolErrorKind::ApplyFailed.wire_value(),
        0x06
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::PersistenceFailed.wire_value(),
        0x07,
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::RollbackFailed.wire_value(),
        0x08,
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::InternalFailure.wire_value(),
        0x09,
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::ALL,
        [
            RemoteControlAnnounceSelfOutcome::Announced,
            RemoteControlAnnounceSelfOutcome::Unavailable,
            RemoteControlAnnounceSelfOutcome::Rejected,
            RemoteControlAnnounceSelfOutcome::WriteFailed,
        ],
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::Announced.wire_value(),
        0x01
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::Unavailable.wire_value(),
        0x02
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::Rejected.wire_value(),
        0x03
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::WriteFailed.wire_value(),
        0x04
    );
}

#[test]
fn describe_build_carries_a_length_prefixed_version_and_rejects_trailers() {
    use crate::remote_control::RemoteControlBuildVersion;

    let version = RemoteControlBuildVersion::from_label("0.3.7", "abcdef0123456789")
        .expect("the build label is bounded");
    assert_eq!(version.as_str(), Some("0.3.7+abcdef0"));
    let request = RemoteControlRequest::DescribeBuild;
    let mut request_bytes = [0u8; RemoteControlRequest::DescribeBuild.encoded_len()];
    assert_eq!(
        request.write_into(&mut request_bytes),
        Ok(request.encoded_len())
    );
    assert_eq!(RemoteControlRequest::parse(&request_bytes), Ok(request));
    assert_eq!(
        RemoteControlRequest::parse(&[
            RemoteControlProtocolVersion::V1.wire_value(),
            RemoteControlRequestKind::DescribeBuild.wire_value(),
            0x00,
        ]),
        Err(crate::remote_control::RemoteControlRequestParseError::Malformed),
    );

    let response = RemoteControlResponse::DescribeBuild(version);
    let mut response_bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let written = response.write_into(&mut response_bytes).unwrap();
    let encoded = response_bytes
        .get(..written)
        .expect("encode stays in buffer");
    assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
    let mut trailing = encoded.to_vec();
    trailing.push(0x00);
    assert_eq!(
        RemoteControlResponse::parse(&trailing),
        Err(crate::remote_control::RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlBuildVersion::from_label(&"v".repeat(80), "deadbeef"),
        Err(crate::remote_control::RemoteControlBuildVersionLabelError::VersionTooLong),
    );
}

#[test]
fn describe_power_carries_a_fixed_power_snapshot_and_rejects_trailers() {
    use crate::capabilities::power::{
        BatteryPercent, ChargingState, ExternalPowerState, PowerSnapshot,
    };

    let snapshot = PowerSnapshot::new(
        Some(BatteryPercent::saturating(73)),
        ExternalPowerState::Present {
            charging: ChargingState::Charging,
        },
    );
    let request = RemoteControlRequest::DescribePower;
    let mut request_bytes = [0u8; RemoteControlRequest::DescribePower.encoded_len()];
    assert_eq!(
        request.write_into(&mut request_bytes),
        Ok(request.encoded_len())
    );
    assert_eq!(RemoteControlRequest::parse(&request_bytes), Ok(request));
    assert_eq!(
        RemoteControlRequest::parse(&[
            RemoteControlProtocolVersion::V1.wire_value(),
            RemoteControlRequestKind::DescribePower.wire_value(),
            0x00,
        ]),
        Err(crate::remote_control::RemoteControlRequestParseError::Malformed),
    );

    let response = RemoteControlResponse::DescribePower(snapshot);
    let mut response_bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let written = response.write_into(&mut response_bytes).unwrap();
    let encoded = response_bytes
        .get(..written)
        .expect("encode stays in buffer");
    assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
    let mut trailing = encoded.to_vec();
    trailing.push(0x00);
    assert_eq!(
        RemoteControlResponse::parse(&trailing),
        Err(crate::remote_control::RemoteControlResponseParseError::Malformed),
    );
}

#[test]
fn describe_request_round_trips_through_its_own_wire_shape() {
    let request = RemoteControlRequest::Describe;
    let mut bytes = [0u8; RemoteControlRequest::Describe.encoded_len()];

    assert_eq!(request.kind(), RemoteControlRequestKind::Describe);
    assert_eq!(request.write_into(&mut bytes), Ok(request.encoded_len()));
    assert_eq!(
        bytes,
        [
            RemoteControlProtocolVersion::V1.wire_value(),
            request.kind().wire_value(),
        ],
    );
    assert_eq!(RemoteControlRequest::parse(&bytes), Ok(request));
}

#[test]
fn announce_self_request_round_trips_through_its_own_wire_shape() {
    let request = RemoteControlRequest::AnnounceSelf;
    let mut bytes = [0u8; RemoteControlRequest::AnnounceSelf.encoded_len()];

    assert_eq!(request.kind(), RemoteControlRequestKind::AnnounceSelf);
    assert_eq!(request.write_into(&mut bytes), Ok(request.encoded_len()));
    assert_eq!(
        bytes,
        [
            RemoteControlProtocolVersion::V1.wire_value(),
            request.kind().wire_value(),
        ],
    );
    assert_eq!(
        request.maximum_response_encoded_len(),
        RemoteControlResponse::ProtocolError(RemoteControlProtocolError::UnknownRequestKind {
            found: 0
        },)
        .encoded_len(),
    );
    assert_eq!(RemoteControlRequest::parse(&bytes), Ok(request));
}

#[test]
fn wifi_station_credentials_reject_empty_ssid_and_omit_password_from_inventory() {
    assert_eq!(
        RemoteControlWifiStation::parse("", "secret"),
        Err(crate::remote_control::RemoteControlWifiStationParseError::EmptySsid),
    );
    assert_eq!(
        RemoteControlWifiStation::parse(&"x".repeat(33), "secret"),
        Err(crate::remote_control::RemoteControlWifiStationParseError::SsidTooLong),
    );
    assert_eq!(
        RemoteControlWifiStation::parse("field-lab", &"x".repeat(65)),
        Err(crate::remote_control::RemoteControlWifiStationParseError::PasswordTooLong),
    );
    let station = RemoteControlWifiStation::parse("field-lab", "secret").expect("valid station");
    assert_eq!(station.ssid(), "field-lab");
    assert_eq!(station.password(), "secret");
    assert!(
        !format!("{station:?}").contains("secret"),
        "password must not appear in Debug"
    );
    assert_eq!(
        crate::remote_control::wifi_station_inventory_config("field-lab")
            .expect("a valid SSID fits its inventory field")
            .as_str(),
        "W,field-lab"
    );
    assert_eq!(
        crate::remote_control::wifi_station_inventory_config(&"s".repeat(33)),
        Err(crate::remote_control::RemoteControlWifiStationInventoryConfigError::SsidTooLong),
    );
    assert_eq!(
        crate::remote_control::parse_wifi_station_ssid("W,field,lab"),
        Some("field,lab")
    );
    assert!(crate::remote_control::parse_wifi_station_ssid("LoRa").is_none());
}

#[test]
fn managing_grants_include_the_path_table() {
    let describe_only = grant(0x21, RemoteControlRequestKind::Describe);
    assert!(!describe_only
        .effective_requests()
        .supports(RemoteControlRequestKind::InventoryPathTable));

    let manager = grant(0x22, RemoteControlRequestKind::DescribePower);
    assert!(manager
        .effective_requests()
        .supports(RemoteControlRequestKind::InventoryPathTable));
    assert!(!manager
        .permitted_requests()
        .supports(RemoteControlRequestKind::InventoryPathTable));
}

#[test]
fn request_sets_intersect_without_changing_either_input() {
    let all = RemoteControlRequestSet::all();
    let describe = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
    let announce = RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf);

    assert_eq!(all.intersection(&describe), describe);
    assert_eq!(describe.intersection(&all), describe);
    assert_eq!(
        describe.intersection(&announce),
        RemoteControlRequestSet::empty()
    );
    assert_eq!(all, RemoteControlRequestSet::all());
    assert_eq!(
        describe,
        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
    );
}

#[test]
fn stored_grants_never_gain_later_request_kinds() {
    let mut stored = RemoteControlRequestSet::empty();
    assert!(stored.insert(RemoteControlRequestKind::InventoryInterfaces));
    assert!(stored.insert(RemoteControlRequestKind::SetInterfacePower));
    assert!(stored.insert(RemoteControlRequestKind::SetInterfaceMode));
    assert!(!stored.supports(RemoteControlRequestKind::SetInterfaceGroup));
    assert!(!stored.supports(RemoteControlRequestKind::InventoryInterfacePeers));
    assert!(!stored.supports(RemoteControlRequestKind::InventoryInterfaceConfig));
    assert!(!stored.supports(RemoteControlRequestKind::SetInterfaceLoRaProfile));
    assert!(!stored.supports(RemoteControlRequestKind::SetInterfaceWifiStation));
    assert!(!stored.supports(RemoteControlRequestKind::InventoryControllers));
    assert!(!stored.supports(RemoteControlRequestKind::AuthorizeController));
    assert!(!stored.supports(RemoteControlRequestKind::RevokeController));
    assert!(!stored.supports(RemoteControlRequestKind::DescribeBuild));
    assert!(!stored.supports(RemoteControlRequestKind::DescribePower));
    assert!(!stored.supports(RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups));
    assert!(!stored.supports(RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups));
    let restored = stored;
    assert_eq!(restored, stored);
    assert!(!restored.supports(RemoteControlRequestKind::SetInterfaceGroup));
    assert!(!restored.supports(RemoteControlRequestKind::InventoryInterfacePeers));
    assert!(!restored.supports(RemoteControlRequestKind::InventoryInterfaceConfig));
    assert!(!restored.supports(RemoteControlRequestKind::SetInterfaceLoRaProfile));
    assert!(!restored.supports(RemoteControlRequestKind::SetInterfaceWifiStation));
    assert!(!restored.supports(RemoteControlRequestKind::InventoryControllers));
    assert!(!restored.supports(RemoteControlRequestKind::AuthorizeController));
    assert!(!restored.supports(RemoteControlRequestKind::RevokeController));
    assert!(!restored.supports(RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups));
    assert!(!restored.supports(RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups));
}

#[test]
fn describe_response_reports_its_available_requests_canonically() {
    let mut available = RemoteControlRequestSet::all();
    assert_eq!(available.len(), RemoteControlRequestKind::ALL.len());
    assert!(!available.insert(RemoteControlRequestKind::Describe));
    assert!(!available.insert(RemoteControlRequestKind::AnnounceSelf));
    assert_eq!(available.len(), RemoteControlRequestKind::ALL.len());
    assert!(available.supports(RemoteControlRequestKind::Describe));
    assert!(available.supports(RemoteControlRequestKind::AnnounceSelf));
    assert!(available.supports(RemoteControlRequestKind::InventoryInterfaces));
    assert!(available.supports(RemoteControlRequestKind::SetInterfacePower));
    assert!(available.supports(RemoteControlRequestKind::SleepRadios));
    assert!(available.supports(RemoteControlRequestKind::WakeRadios));
    assert!(available.supports(RemoteControlRequestKind::SetInterfaceMode));
    assert!(available.supports(RemoteControlRequestKind::SetInterfaceGroup));
    assert!(available.supports(RemoteControlRequestKind::InventoryInterfacePeers));
    assert!(available.supports(RemoteControlRequestKind::InventoryInterfaceConfig));
    assert!(available.supports(RemoteControlRequestKind::SetInterfaceLoRaProfile));
    assert!(available.supports(RemoteControlRequestKind::DescribeBuild));
    assert!(available.supports(RemoteControlRequestKind::DescribePower));
    assert!(available.supports(RemoteControlRequestKind::SetInterfaceWifiStation));
    assert!(available.supports(RemoteControlRequestKind::InventoryControllers));
    assert!(available.supports(RemoteControlRequestKind::AuthorizeController));
    assert!(available.supports(RemoteControlRequestKind::RevokeController));
    assert!(!available.is_empty());
    assert_eq!(
        available.iter().collect::<std::vec::Vec<_>>(),
        RemoteControlRequestKind::ALL.to_vec(),
    );
    assert_eq!(
        RemoteControlDescription::try_from(RemoteControlRequestSet::empty()),
        Err(RemoteControlDescriptionError::DescribeUnavailable),
    );
    assert_eq!(
        RemoteControlDescription::try_from(RemoteControlRequestSet::only(
            RemoteControlRequestKind::AnnounceSelf,
        )),
        Err(RemoteControlDescriptionError::DescribeUnavailable),
    );

    let description = RemoteControlDescription::try_from(available).unwrap();
    let response = RemoteControlResponse::Describe(description);
    let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let available_count = u8::try_from(available.len()).unwrap_or(u8::MAX);

    let written = response.write_into(&mut bytes).unwrap();
    let encoded = bytes.get(..written).unwrap_or_default();
    assert_eq!(written, response.encoded_len());
    let mut expected = std::vec![
        RemoteControlProtocolVersion::V1.wire_value(),
        response.kind().wire_value(),
        available_count,
    ];
    expected.extend(RemoteControlRequestKind::ALL.map(RemoteControlRequestKind::wire_value));
    assert_eq!(encoded, expected.as_slice());
    assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
}

#[test]
fn announce_self_outcomes_round_trip_with_their_typed_wire_values() {
    for outcome in RemoteControlAnnounceSelfOutcome::ALL {
        let response = RemoteControlResponse::AnnounceSelf(outcome);
        let mut bytes =
            [0u8; RemoteControlRequestKind::AnnounceSelf.maximum_response_encoded_len()];
        let written = response.write_into(&mut bytes).unwrap();
        let encoded = bytes.get(..written).unwrap_or_default();

        assert_eq!(written, response.encoded_len());
        assert_eq!(
            encoded,
            &[
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlResponseKind::AnnounceSelf.wire_value(),
                outcome.wire_value(),
            ],
        );
        assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
    }
}

#[test]
fn protocol_error_responses_round_trip_with_their_own_lengths() {
    let cases = [
        (RemoteControlProtocolError::MalformedRequest, None),
        (
            RemoteControlProtocolError::UnsupportedVersion { found: 0x71 },
            Some(0x71),
        ),
        (
            RemoteControlProtocolError::UnknownRequestKind { found: 0x93 },
            Some(0x93),
        ),
        (
            RemoteControlProtocolError::UnsupportedRequest {
                request: RemoteControlRequestKind::DescribePower,
            },
            Some(RemoteControlRequestKind::DescribePower.wire_value()),
        ),
        (
            RemoteControlProtocolError::Busy {
                request: RemoteControlRequestKind::SetInterfacePower,
            },
            Some(RemoteControlRequestKind::SetInterfacePower.wire_value()),
        ),
        (
            RemoteControlProtocolError::ApplyFailed {
                request: RemoteControlRequestKind::SetInterfaceMode,
            },
            Some(RemoteControlRequestKind::SetInterfaceMode.wire_value()),
        ),
        (
            RemoteControlProtocolError::PersistenceFailed {
                request: RemoteControlRequestKind::SetInterfaceGroup,
            },
            Some(RemoteControlRequestKind::SetInterfaceGroup.wire_value()),
        ),
        (
            RemoteControlProtocolError::RollbackFailed {
                request: RemoteControlRequestKind::SetInterfaceLoRaProfile,
            },
            Some(RemoteControlRequestKind::SetInterfaceLoRaProfile.wire_value()),
        ),
        (
            RemoteControlProtocolError::InternalFailure {
                request: RemoteControlRequestKind::InventoryInterfaces,
            },
            Some(RemoteControlRequestKind::InventoryInterfaces.wire_value()),
        ),
    ];

    for (error, detail) in cases {
        let response = RemoteControlResponse::ProtocolError(error);
        let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let written = response.write_into(&mut bytes).unwrap();
        let encoded = bytes.get(..written).unwrap_or_default();
        let mut expected = std::vec![
            RemoteControlProtocolVersion::V1.wire_value(),
            response.kind().wire_value(),
            error.kind().wire_value(),
        ];
        expected.extend(detail);

        assert_eq!(written, response.encoded_len());
        assert_eq!(encoded, expected);
        assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
    }
}

#[test]
fn request_parser_classifies_protocol_failures() {
    let unsupported_version = RemoteControlProtocolVersion::V1
        .wire_value()
        .wrapping_add(1);
    let unknown_kind = 0xA5;
    assert_eq!(
        RemoteControlRequest::parse(&[]),
        Err(RemoteControlRequestParseError::Truncated),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[
            unsupported_version,
            RemoteControlRequestKind::Describe.wire_value(),
        ]),
        Err(RemoteControlRequestParseError::UnsupportedVersion {
            found: unsupported_version,
        }),
    );
    assert_eq!(
        RemoteControlRequest::parse(
            &[RemoteControlProtocolVersion::V1.wire_value(), unknown_kind,]
        ),
        Err(RemoteControlRequestParseError::UnknownRequestKind {
            found: unknown_kind,
        }),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[
            RemoteControlProtocolVersion::V1.wire_value(),
            RemoteControlRequestKind::Describe.wire_value(),
            0x00,
        ]),
        Err(RemoteControlRequestParseError::Malformed),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[
            RemoteControlProtocolVersion::V1.wire_value(),
            RemoteControlRequestKind::AnnounceSelf.wire_value(),
            0x00,
        ]),
        Err(RemoteControlRequestParseError::Malformed),
    );
}

#[test]
fn response_parser_rejects_noncanonical_and_unknown_descriptions() {
    let version = RemoteControlProtocolVersion::V1.wire_value();
    let describe = RemoteControlResponseKind::Describe.wire_value();
    let describe_request = RemoteControlRequestKind::Describe.wire_value();
    let unknown_request = 0x80;
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe]),
        Err(RemoteControlResponseParseError::Truncated),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[
            version,
            describe,
            0x02,
            describe_request,
            describe_request,
        ]),
        Err(RemoteControlResponseParseError::NonCanonicalRequestSet),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe, 0x02, describe_request]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe, 0x01, describe_request, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe, 0x01, unknown_request]),
        Err(RemoteControlResponseParseError::UnknownRequestKind {
            found: unknown_request,
        }),
    );
}

#[test]
fn response_parser_classifies_response_header_and_error_failures() {
    let version = RemoteControlProtocolVersion::V1.wire_value();
    let unsupported_version = version.wrapping_add(1);
    let describe = RemoteControlResponseKind::Describe.wire_value();
    let announce = RemoteControlResponseKind::AnnounceSelf.wire_value();
    let protocol_error = RemoteControlResponseKind::ProtocolError.wire_value();
    let malformed_request = RemoteControlProtocolErrorKind::MalformedRequest.wire_value();
    let unsupported_version_error = RemoteControlProtocolErrorKind::UnsupportedVersion.wire_value();
    let announced = RemoteControlAnnounceSelfOutcome::Announced.wire_value();
    let unknown_response = 0x72;
    let unknown_protocol_error = 0x82;
    let unknown_announce_outcome = 0x72;
    assert_eq!(
        RemoteControlResponse::parse(&[unsupported_version, describe]),
        Err(RemoteControlResponseParseError::UnsupportedVersion {
            found: unsupported_version,
        }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, unknown_response]),
        Err(RemoteControlResponseParseError::UnknownResponseKind {
            found: unknown_response,
        }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, protocol_error, unknown_protocol_error]),
        Err(RemoteControlResponseParseError::UnknownProtocolErrorKind {
            found: unknown_protocol_error,
        }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, protocol_error, unsupported_version_error]),
        Err(RemoteControlResponseParseError::Truncated),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, protocol_error, malformed_request, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, announce]),
        Err(RemoteControlResponseParseError::Truncated),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, announce, unknown_announce_outcome]),
        Err(
            RemoteControlResponseParseError::UnknownAnnounceSelfOutcome {
                found: unknown_announce_outcome,
            }
        ),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, announce, announced, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
}

#[test]
fn parse_failures_map_to_the_public_protocol_errors() {
    assert_eq!(
        RemoteControlProtocolError::from(RemoteControlRequestParseError::Truncated),
        RemoteControlProtocolError::MalformedRequest,
    );
    assert_eq!(
        RemoteControlProtocolError::from(RemoteControlRequestParseError::UnsupportedVersion {
            found: 0x33
        },),
        RemoteControlProtocolError::UnsupportedVersion { found: 0x33 },
    );
    assert_eq!(
        RemoteControlProtocolError::from(RemoteControlRequestParseError::UnknownRequestKind {
            found: 0x44
        },),
        RemoteControlProtocolError::UnknownRequestKind { found: 0x44 },
    );
}

#[test]
fn message_writers_use_only_their_reported_prefix_and_refuse_short_buffers() {
    let request = RemoteControlRequest::Describe;
    let mut request_bytes = [0xA5; 3];
    assert_eq!(
        request.write_into(&mut request_bytes),
        Ok(request.encoded_len())
    );
    assert_eq!(
        request_bytes,
        [
            RemoteControlProtocolVersion::V1.wire_value(),
            request.kind().wire_value(),
            0xA5,
        ],
    );
    assert_eq!(
        request.write_into(&mut request_bytes[..1]),
        Err(RemoteControlMessageWriteError::BufferTooShort),
    );

    let description = RemoteControlDescription::try_from(RemoteControlRequestSet::all()).unwrap();
    let available_count = u8::try_from(description.available_requests().len()).unwrap_or(u8::MAX);
    let response = RemoteControlResponse::Describe(description);
    let response_kind = response.kind().wire_value();
    let encoded_len = response.encoded_len();
    let mut response_bytes = std::vec![0x5A; encoded_len.saturating_add(1)];
    assert_eq!(response.write_into(&mut response_bytes), Ok(encoded_len),);
    let mut expected = std::vec![
        RemoteControlProtocolVersion::V1.wire_value(),
        response_kind,
        available_count,
    ];
    expected.extend(RemoteControlRequestKind::ALL.map(RemoteControlRequestKind::wire_value));
    expected.push(0x5A);
    assert_eq!(response_bytes, expected);
    assert_eq!(
        response.write_into(response_bytes.get_mut(..3).expect("3-byte prefix exists")),
        Err(RemoteControlMessageWriteError::BufferTooShort),
    );
}

#[test]
fn inventory_power_and_sleep_messages_round_trip() {
    use crate::interfaces::{
        ConnectionState, DiscoveryGroupId, DiscoveryGroupSet, InterfaceId, InterfaceKind,
        InterfaceMode, INTERFACE_ID_LEN,
    };
    use crate::remote_control::{
        RemoteControlApplyOutcome, RemoteControlControllerPage, RemoteControlDiscoveryGroups,
        RemoteControlDiscoveryGroupsInventoryOutcome, RemoteControlDiscoveryGroupsReplaceOutcome,
        RemoteControlDisplayAutoOff, RemoteControlDisplayVisibility, RemoteControlEspRadioMode,
        RemoteControlGnssPower, RemoteControlGroupOutcome, RemoteControlInterfaceCard,
        RemoteControlInterfaceEntry, RemoteControlInterfaceGroup, RemoteControlInterfaceInventory,
        RemoteControlInterfacePage, RemoteControlInterfacePower, RemoteControlModeOutcome,
        RemoteControlPeerCursor, RemoteControlPeerPage, RemoteControlPowerOutcome,
        RemoteControlSleepOutcome, RemoteControlStationUplink, RemoteControlSystemPower,
        RemoteControlWifiConfirmationRemaining, RemoteControlWifiCredentialRevision,
        RemoteControlWifiStageOutcome, RemoteControlWifiTransactionStatus,
    };

    for request in [
        RemoteControlRequest::InventoryInterfaces {
            page: RemoteControlInterfacePage::First,
        },
        RemoteControlRequest::SetInterfacePower {
            id: InterfaceId::new([0x11; INTERFACE_ID_LEN]),
            power: RemoteControlInterfacePower::On,
        },
        RemoteControlRequest::SetInterfacePower {
            id: InterfaceId::new([0x22; INTERFACE_ID_LEN]),
            power: RemoteControlInterfacePower::Off,
        },
        RemoteControlRequest::SetInterfaceMode {
            id: InterfaceId::new([0x33; INTERFACE_ID_LEN]),
            mode: InterfaceMode::Gateway,
        },
        RemoteControlRequest::SetInterfaceGroup {
            id: InterfaceId::new([0x44; INTERFACE_ID_LEN]),
            group: RemoteControlInterfaceGroup::parse("field-mesh").expect("valid group"),
        },
        RemoteControlRequest::InventoryInterfaceDiscoveryGroups {
            id: InterfaceId::new([0x45; INTERFACE_ID_LEN]),
        },
        RemoteControlRequest::InventoryPathTable {
            page: RemoteControlPathPage::First,
        },
        RemoteControlRequest::ReplaceInterfaceDiscoveryGroups {
            id: InterfaceId::new([0x46; INTERFACE_ID_LEN]),
            groups: RemoteControlDiscoveryGroups::new(
                DiscoveryGroupSet::try_from_slice(&[
                    DiscoveryGroupId::parse("alpha").expect("valid group"),
                    DiscoveryGroupId::parse("beta").expect("valid group"),
                ])
                .expect("valid groups"),
            ),
        },
        RemoteControlRequest::InventoryInterfacePeers {
            id: InterfaceId::new([0x55; INTERFACE_ID_LEN]),
            page: RemoteControlPeerPage::After(RemoteControlPeerCursor::after(InterfaceId::new(
                [0x54; INTERFACE_ID_LEN],
            ))),
        },
        RemoteControlRequest::InventoryInterfaceConfig {
            id: InterfaceId::new([0x66; INTERFACE_ID_LEN]),
        },
        RemoteControlRequest::SetInterfaceLoRaProfile {
            id: InterfaceId::new([0x77; INTERFACE_ID_LEN]),
            profile: crate::remote_control::RemoteControlLoRaProfile::from_profile(
                crate::interfaces::lora::DEFAULT_915_PROFILE,
            )
            .expect("default profile is valid"),
        },
        RemoteControlRequest::SetInterfaceWifiStation {
            id: InterfaceId::new([0x88; INTERFACE_ID_LEN]),
            station: crate::remote_control::RemoteControlWifiStation::parse("field-lab", "secret")
                .expect("valid station"),
        },
        RemoteControlRequest::InventoryControllers {
            page: RemoteControlControllerPage::First,
        },
        RemoteControlRequest::AuthorizeController {
            controller: controller_identity(0x91),
            permitted_requests: RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
        },
        RemoteControlRequest::RevokeController {
            hash: controller_identity(0x92).identity_hash(),
        },
        RemoteControlRequest::DescribeBuild,
        RemoteControlRequest::DescribePower,
        RemoteControlRequest::SleepRadios,
        RemoteControlRequest::WakeRadios,
        RemoteControlRequest::SetSystemPower {
            power: RemoteControlSystemPower::Asleep,
        },
        RemoteControlRequest::SetGnssPower {
            power: RemoteControlGnssPower::On,
        },
        RemoteControlRequest::SetDisplayVisibility {
            visibility: RemoteControlDisplayVisibility::Hidden,
        },
        RemoteControlRequest::SetDisplayAutoOff {
            auto_off: RemoteControlDisplayAutoOff::Disabled,
        },
        RemoteControlRequest::SetStationUplink {
            id: InterfaceId::new([0xA1; INTERFACE_ID_LEN]),
            uplink: RemoteControlStationUplink::Enabled,
        },
        RemoteControlRequest::SetEspRadioMode {
            mode: RemoteControlEspRadioMode::AccessPoint,
        },
        RemoteControlRequest::StageWifiCredentials {
            station: crate::remote_control::RemoteControlWifiStation::parse(
                "replacement-network",
                "move-only-secret",
            )
            .expect("valid station"),
        },
        RemoteControlRequest::ActivateWifiCredentials {
            revision: RemoteControlWifiCredentialRevision::new(41).expect("nonzero revision"),
        },
        RemoteControlRequest::ConfirmWifiCredentials {
            revision: RemoteControlWifiCredentialRevision::new(42).expect("nonzero revision"),
        },
        RemoteControlRequest::CancelWifiCredentials {
            revision: RemoteControlWifiCredentialRevision::new(43).expect("nonzero revision"),
        },
        RemoteControlRequest::InspectWifiTransaction,
    ] {
        let mut bytes = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let written = request.write_into(&mut bytes).unwrap();
        assert_eq!(
            RemoteControlRequest::parse(bytes.get(..written).expect("encode stays in buffer")),
            Ok(request)
        );
    }

    let mut inventory = RemoteControlInterfaceInventory::empty();
    inventory
        .push(RemoteControlInterfaceEntry {
            id: InterfaceId::new([0xAB; INTERFACE_ID_LEN]),
            kind: InterfaceKind::LoRa,
            mode: InterfaceMode::Full,
            connection: ConnectionState::Connected,
            enabled: true,
            tx_bytes: 1,
            rx_bytes: 2,
            links: 3,
            rate_bytes_per_sec: 4,
        })
        .unwrap();

    for response in [
        RemoteControlResponse::InventoryInterfaces(inventory.clone()),
        RemoteControlResponse::SetInterfacePower(RemoteControlPowerOutcome::Applied),
        RemoteControlResponse::SetInterfacePower(RemoteControlPowerOutcome::UnknownInterface),
        RemoteControlResponse::SetInterfacePower(RemoteControlPowerOutcome::Failed),
        RemoteControlResponse::SetInterfaceMode(RemoteControlModeOutcome::Applied),
        RemoteControlResponse::SetInterfaceMode(RemoteControlModeOutcome::UnknownInterface),
        RemoteControlResponse::SetInterfaceMode(RemoteControlModeOutcome::Failed),
        RemoteControlResponse::SetInterfaceGroup(RemoteControlGroupOutcome::Applied),
        RemoteControlResponse::SetInterfaceGroup(RemoteControlGroupOutcome::UnknownInterface),
        RemoteControlResponse::SetInterfaceGroup(RemoteControlGroupOutcome::Failed),
        RemoteControlResponse::InventoryInterfaceDiscoveryGroups(
            RemoteControlDiscoveryGroupsInventoryOutcome::Groups(
                RemoteControlDiscoveryGroups::new(
                    DiscoveryGroupSet::try_from_slice(&[
                        DiscoveryGroupId::parse("alpha").expect("valid group"),
                        DiscoveryGroupId::parse("beta").expect("valid group"),
                    ])
                    .expect("valid groups"),
                ),
            ),
        ),
        RemoteControlResponse::InventoryInterfaceDiscoveryGroups(
            RemoteControlDiscoveryGroupsInventoryOutcome::UnknownInterface,
        ),
        RemoteControlResponse::InventoryInterfaceDiscoveryGroups(
            RemoteControlDiscoveryGroupsInventoryOutcome::Unsupported,
        ),
        RemoteControlResponse::ReplaceInterfaceDiscoveryGroups(
            RemoteControlDiscoveryGroupsReplaceOutcome::Applied,
        ),
        RemoteControlResponse::ReplaceInterfaceDiscoveryGroups(
            RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged,
        ),
        RemoteControlResponse::ReplaceInterfaceDiscoveryGroups(
            RemoteControlDiscoveryGroupsReplaceOutcome::UnknownInterface,
        ),
        RemoteControlResponse::ReplaceInterfaceDiscoveryGroups(
            RemoteControlDiscoveryGroupsReplaceOutcome::Unsupported,
        ),
        RemoteControlResponse::InventoryInterfacePeers(
            crate::remote_control::RemoteControlInterfacePeersOutcome::UnknownInterface,
        ),
        RemoteControlResponse::InventoryInterfaceConfig(
            crate::remote_control::RemoteControlInterfaceConfigOutcome::UnknownInterface,
        ),
        RemoteControlResponse::SetInterfaceLoRaProfile(
            crate::remote_control::RemoteControlLoRaOutcome::Applied,
        ),
        RemoteControlResponse::SetInterfaceLoRaProfile(
            crate::remote_control::RemoteControlLoRaOutcome::UnknownInterface,
        ),
        RemoteControlResponse::SetInterfaceLoRaProfile(
            crate::remote_control::RemoteControlLoRaOutcome::Failed,
        ),
        RemoteControlResponse::SetInterfaceWifiStation(
            crate::remote_control::RemoteControlWifiStationOutcome::Applied,
        ),
        RemoteControlResponse::SetInterfaceWifiStation(
            crate::remote_control::RemoteControlWifiStationOutcome::UnknownInterface,
        ),
        RemoteControlResponse::SetInterfaceWifiStation(
            crate::remote_control::RemoteControlWifiStationOutcome::Failed,
        ),
        RemoteControlResponse::InventoryControllers({
            let mut grants =
                crate::remote_control::FixedRemoteControlControllerGrantTable::<8>::default();
            grants
                .set_controller_grant(
                    crate::remote_control::RemoteControlControllerGrant::new(
                        controller_identity(0x91),
                        RemoteControlControllerAuthority::Operator,
                        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
                    )
                    .expect("grant"),
                )
                .expect("fits");
            crate::remote_control::RemoteControlControllerInventory::from_grants(
                &grants,
                RemoteControlControllerPage::First,
            )
            .expect("ordered grants fit one inventory page")
        }),
        RemoteControlResponse::AuthorizeController(
            crate::remote_control::RemoteControlAuthorizeControllerOutcome::Applied,
        ),
        RemoteControlResponse::AuthorizeController(
            crate::remote_control::RemoteControlAuthorizeControllerOutcome::CapacityExhausted,
        ),
        RemoteControlResponse::RevokeController(
            crate::remote_control::RemoteControlRevokeControllerOutcome::Applied,
        ),
        RemoteControlResponse::RevokeController(
            crate::remote_control::RemoteControlRevokeControllerOutcome::Forbidden,
        ),
        RemoteControlResponse::DescribeBuild(
            crate::remote_control::RemoteControlBuildVersion::from_text("0.3.7+abcdef0")
                .expect("version fits"),
        ),
        RemoteControlResponse::DescribeBuild(
            crate::remote_control::RemoteControlBuildVersion::empty(),
        ),
        RemoteControlResponse::SleepRadios(RemoteControlSleepOutcome::Applied),
        RemoteControlResponse::SleepRadios(RemoteControlSleepOutcome::Unavailable),
        RemoteControlResponse::SleepRadios(RemoteControlSleepOutcome::Failed),
        RemoteControlResponse::WakeRadios(RemoteControlSleepOutcome::Applied),
        RemoteControlResponse::WakeRadios(RemoteControlSleepOutcome::Failed),
        RemoteControlResponse::SetSystemPower(RemoteControlApplyOutcome::Scheduled),
        RemoteControlResponse::SetGnssPower(RemoteControlApplyOutcome::Applied),
        RemoteControlResponse::SetDisplayVisibility(RemoteControlApplyOutcome::Unchanged),
        RemoteControlResponse::SetDisplayAutoOff(RemoteControlApplyOutcome::Applied),
        RemoteControlResponse::SetStationUplink(RemoteControlApplyOutcome::Applied),
        RemoteControlResponse::SetEspRadioMode(RemoteControlApplyOutcome::Scheduled),
        RemoteControlResponse::StageWifiCredentials(RemoteControlWifiStageOutcome::Staged(
            RemoteControlWifiCredentialRevision::new(51).expect("nonzero revision"),
        )),
        RemoteControlResponse::StageWifiCredentials(
            RemoteControlWifiStageOutcome::InvalidCredentials,
        ),
        RemoteControlResponse::ActivateWifiCredentials(RemoteControlApplyOutcome::Applied),
        RemoteControlResponse::ConfirmWifiCredentials(RemoteControlApplyOutcome::Applied),
        RemoteControlResponse::CancelWifiCredentials(RemoteControlApplyOutcome::Unchanged),
        RemoteControlResponse::InspectWifiTransaction(
            RemoteControlWifiTransactionStatus::FactoryProvisioning,
        ),
        RemoteControlResponse::InspectWifiTransaction(
            RemoteControlWifiTransactionStatus::AwaitingConfirmation {
                revision: RemoteControlWifiCredentialRevision::new(52).expect("nonzero revision"),
                remaining: RemoteControlWifiConfirmationRemaining::new(120)
                    .expect("bounded remaining time"),
            },
        ),
    ] {
        let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let written = response.write_into(&mut bytes).unwrap();
        assert_eq!(
            RemoteControlResponse::parse(bytes.get(..written).expect("encode stays in buffer")),
            Ok(response)
        );
    }

    let mut card = RemoteControlInterfaceCard::empty();
    card.set_name("BLE").unwrap();
    card.set_group("home").unwrap();
    card.set_config("IFAC 16").unwrap();
    card.set_failure("radio timeout").unwrap();
    card.destinations = 4;
    card.transported_links = 1;
    let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let response = RemoteControlResponse::InventoryInterfaceConfig(
        crate::remote_control::RemoteControlInterfaceConfigOutcome::Card(card),
    );
    let written = response.write_into(&mut bytes).unwrap();
    assert_eq!(
        RemoteControlResponse::parse(bytes.get(..written).expect("encode stays in buffer")),
        Ok(response)
    );
}

#[test]
fn inventory_responses_reject_overlong_and_noncanonical_fields() {
    use crate::interfaces::{
        ConnectionState, InterfaceId, InterfaceKind, InterfaceMode, PeerDetails, RadioIndication,
        INTERFACE_ID_LEN,
    };
    use crate::remote_control::{
        RemoteControlInterfaceCard, RemoteControlInterfaceCardError, RemoteControlInterfaceEntry,
        RemoteControlInterfacePeer, RemoteControlInterfacePeerPage,
        RemoteControlInterfacePeersOutcome,
    };

    let entry = RemoteControlInterfaceEntry {
        id: InterfaceId::new([0x11; INTERFACE_ID_LEN]),
        kind: InterfaceKind::LoRa,
        mode: InterfaceMode::Full,
        connection: ConnectionState::Connected,
        enabled: true,
        tx_bytes: 1,
        rx_bytes: 2,
        links: 3,
        rate_bytes_per_sec: 4,
    };
    let mut entry_wire = [0u8; RemoteControlInterfaceEntry::ENCODED_LEN];
    entry.write_into(&mut entry_wire).unwrap();
    let connection_offset = INTERFACE_ID_LEN + 2;
    *entry_wire.get_mut(connection_offset).unwrap() = 0x7e;
    assert_eq!(
        RemoteControlInterfaceEntry::parse(&entry_wire),
        Err(RemoteControlResponseParseError::UnknownConnectionState { found: 0x7e }),
    );
    *entry_wire.get_mut(connection_offset).unwrap() = 1;
    *entry_wire.get_mut(connection_offset + 1).unwrap() = 0x80;
    assert_eq!(
        RemoteControlInterfaceEntry::parse(&entry_wire),
        Err(RemoteControlResponseParseError::Malformed),
    );

    let mut card = RemoteControlInterfaceCard::empty();
    card.set_name("prior").unwrap();
    assert_eq!(
        card.set_name(&"n".repeat(33)),
        Err(RemoteControlInterfaceCardError::NameTooLong),
    );
    assert_eq!(card.name.as_str(), "prior");
    card.set_name(&"n".repeat(32)).unwrap();
    let response = RemoteControlResponse::InventoryInterfaceConfig(
        RemoteControlInterfaceConfigOutcome::Card(card),
    );
    let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let written = response.write_into(&mut encoded).unwrap();
    let mut overlong = encoded.get(..written).unwrap().to_vec();
    let name_length_offset = 2 + 1 + 4 + 4;
    *overlong.get_mut(name_length_offset).unwrap() = 33;
    overlong.insert(name_length_offset + 1 + 32, b'n');
    assert_eq!(
        RemoteControlResponse::parse(&overlong),
        Err(RemoteControlResponseParseError::Malformed),
    );

    let supervisor = InterfaceId::new([0x22; INTERFACE_ID_LEN]);
    let mut peers = RemoteControlInterfacePeerPage::empty(supervisor);
    peers
        .push(RemoteControlInterfacePeer {
            id: InterfaceId::new([0x23; INTERFACE_ID_LEN]),
            connection: ConnectionState::Connected,
            tx_bytes: 1,
            rx_bytes: 2,
            links: 3,
            destinations: 4,
            rate_bytes_per_sec: 5,
            radio: RadioIndication::NotRadio,
            details: PeerDetails::NotApplicable,
        })
        .unwrap();
    let response = RemoteControlResponse::InventoryInterfacePeers(
        RemoteControlInterfacePeersOutcome::Page(peers),
    );
    let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let written = response.write_into(&mut encoded).unwrap();
    let peer_start = 2 + 1 + INTERFACE_ID_LEN + 1;
    let radio_start = peer_start + INTERFACE_ID_LEN + 1 + 8 + 8 + 4 + 4 + 4;
    let mut nonzero_radio_padding = encoded.get(..written).unwrap().to_vec();
    *nonzero_radio_padding.get_mut(radio_start + 1).unwrap() = 1;
    assert_eq!(
        RemoteControlResponse::parse(&nonzero_radio_padding),
        Err(RemoteControlResponseParseError::Malformed),
    );
    let mut noncanonical_details = encoded.get(..written).unwrap().to_vec();
    *noncanonical_details
        .get_mut(radio_start + RadioIndication::MAX_ENCODED_LEN + 1)
        .unwrap() = 1;
    assert_eq!(
        RemoteControlResponse::parse(&noncanonical_details),
        Err(RemoteControlResponseParseError::Malformed),
    );
}

#[test]
fn discovery_group_requests_reject_noncanonical_whole_values() {
    use crate::interfaces::{DiscoveryGroupId, DiscoveryGroupSet, InterfaceId, INTERFACE_ID_LEN};
    use crate::remote_control::RemoteControlDiscoveryGroups;

    let request = RemoteControlRequest::ReplaceInterfaceDiscoveryGroups {
        id: InterfaceId::new([0x41; INTERFACE_ID_LEN]),
        groups: RemoteControlDiscoveryGroups::new(
            DiscoveryGroupSet::try_from_slice(&[
                DiscoveryGroupId::parse("aa").expect("valid group"),
                DiscoveryGroupId::parse("bb").expect("valid group"),
            ])
            .expect("valid groups"),
        ),
    };
    let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
    let written = request.write_into(&mut encoded).expect("request fits");
    let group_body = 2 + INTERFACE_ID_LEN;

    let canonical = encoded.get(..written).unwrap_or_default();
    let mut unsorted = canonical.to_vec();
    assert_eq!(
        unsorted
            .get_mut(group_body + 2..group_body + 4)
            .map(|slot| slot.copy_from_slice(b"bb")),
        Some(())
    );
    assert_eq!(
        unsorted
            .get_mut(group_body + 5..group_body + 7)
            .map(|slot| slot.copy_from_slice(b"aa")),
        Some(())
    );
    assert_eq!(
        RemoteControlRequest::parse(&unsorted),
        Err(RemoteControlRequestParseError::Malformed)
    );

    let mut duplicate = canonical.to_vec();
    assert_eq!(
        duplicate
            .get_mut(group_body + 5..group_body + 7)
            .map(|slot| slot.copy_from_slice(b"aa")),
        Some(())
    );
    assert_eq!(
        RemoteControlRequest::parse(&duplicate),
        Err(RemoteControlRequestParseError::Malformed)
    );

    let mut trailing = canonical.to_vec();
    trailing.push(0);
    assert_eq!(
        RemoteControlRequest::parse(&trailing),
        Err(RemoteControlRequestParseError::Malformed)
    );

    let mut empty = canonical.to_vec();
    assert_eq!(empty.get_mut(group_body).map(|count| *count = 0), Some(()));
    empty.truncate(group_body + 1);
    assert_eq!(
        RemoteControlRequest::parse(&empty),
        Err(RemoteControlRequestParseError::Malformed)
    );
}

#[test]
fn desired_state_and_wifi_transaction_parsers_refuse_noncanonical_values() {
    let version = RemoteControlProtocolVersion::V1.wire_value();

    for kind in [
        RemoteControlRequestKind::SetSystemPower,
        RemoteControlRequestKind::SetGnssPower,
        RemoteControlRequestKind::SetDisplayVisibility,
        RemoteControlRequestKind::SetDisplayAutoOff,
        RemoteControlRequestKind::SetEspRadioMode,
    ] {
        assert_eq!(
            RemoteControlRequest::parse(&[version, kind.wire_value(), 0xFF]),
            Err(RemoteControlRequestParseError::Malformed),
        );
        assert_eq!(
            RemoteControlRequest::parse(&[version, kind.wire_value(), 0x01, 0x00]),
            Err(RemoteControlRequestParseError::Malformed),
        );
    }

    for kind in [
        RemoteControlRequestKind::ActivateWifiCredentials,
        RemoteControlRequestKind::ConfirmWifiCredentials,
        RemoteControlRequestKind::CancelWifiCredentials,
    ] {
        assert_eq!(
            RemoteControlRequest::parse(&[version, kind.wire_value(), 0, 0, 0, 0]),
            Err(RemoteControlRequestParseError::Malformed),
        );
        assert_eq!(
            RemoteControlRequest::parse(&[version, kind.wire_value(), 0, 0, 0, 1, 0]),
            Err(RemoteControlRequestParseError::Malformed),
        );
    }

    assert_eq!(
        RemoteControlResponse::parse(&[
            version,
            RemoteControlResponseKind::SetSystemPower.wire_value(),
            0xFF,
        ]),
        Err(RemoteControlResponseParseError::UnknownApplyOutcome { found: 0xFF }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[
            version,
            RemoteControlResponseKind::StageWifiCredentials.wire_value(),
            0x01,
            0,
            0,
            0,
            0,
        ]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[
            version,
            RemoteControlResponseKind::InspectWifiTransaction.wire_value(),
            0x03,
            0,
            0,
            0,
            1,
            121,
        ]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[
            version,
            RemoteControlResponseKind::InspectWifiTransaction.wire_value(),
            0xFF,
        ]),
        Err(RemoteControlResponseParseError::UnknownWifiTransactionStatus { found: 0xFF }),
    );
}

proptest! {
    #[test]
    fn every_successfully_parsed_request_round_trips_through_its_writer(
        bytes in proptest::collection::vec(any::<u8>(), 0..=256),
    ) {
        if let Ok(request) = RemoteControlRequest::parse(&bytes) {
            let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
            let written = request.write_into(&mut encoded).unwrap();
            let encoded = encoded
                .get(..written)
                .expect("request writer returned an out-of-bounds length");
            prop_assert_eq!(RemoteControlRequest::parse(encoded), Ok(request));
        }
    }

    #[test]
    fn every_successfully_parsed_response_round_trips_through_its_writer(
        bytes in proptest::collection::vec(any::<u8>(), 0..=256),
    ) {
        if let Ok(response) = RemoteControlResponse::parse(&bytes) {
            let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
            let written = response.write_into(&mut encoded).unwrap();
            let encoded = encoded
                .get(..written)
                .expect("response writer returned an out-of-bounds length");
            prop_assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
        }
    }
}
