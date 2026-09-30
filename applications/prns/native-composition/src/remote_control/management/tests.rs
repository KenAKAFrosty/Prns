#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use personal_rns::engine::SendRequestFailure;
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceMode, INTERFACE_ID_LEN,
};
use prns_core::capabilities::power::{
    BatteryPercent, ChargingState, ExternalPowerState, PowerSnapshot,
};

fn id(byte: u8) -> InterfaceId {
    InterfaceId::new([byte; INTERFACE_ID_LEN])
}

#[test]
fn controller_pages_preserve_hashes_and_reject_cross_page_regression() {
    // A bounded upstream inventory body: two ordered hashes and its keyset continuation.
    let mut body = vec![2];
    body.extend_from_slice(&[2; 16]);
    body.extend_from_slice(&[3; 16]);
    body.push(1);
    body.extend_from_slice(&[3; 16]);
    let inventory = core::RemoteControlControllerInventory::parse_body(&body).expect("valid page");
    let page = project_controllers(inventory.clone(), identity_hash(&[1; 16])).expect("progress");
    assert_eq!(page.identities, vec![vec![2; 16], vec![3; 16]]);
    assert_eq!(page.next, Some(vec![3; 16]));
    for after in [2, 3, 4] {
        assert_eq!(
            project_controllers(inventory.clone(), identity_hash(&[after; 16]))
                .unwrap_err()
                .0,
            RemoteManagementFailureStage::Response
        );
    }
    assert_eq!(
        project_controllers(
            core::RemoteControlControllerInventory::empty(),
            identity_hash(&[3; 16])
        )
        .expect("empty final page"),
        RemoteControllerPage {
            identities: vec![],
            next: None
        }
    );
}

#[test]
fn controller_queries_and_changes_validate_identity_length_and_capability() {
    for invalid in [vec![], vec![1; 15], vec![1; 17]] {
        assert!(prepare_query(RemoteNodeQuery::Controllers {
            after: Some(invalid.clone())
        })
        .is_err());
        assert!(prepare_change(&RemoteNodeChange::RevokeController {
            controller_identity_fingerprint: invalid
        })
        .is_err());
    }
    assert!(prepare_query(RemoteNodeQuery::Controllers { after: None }).is_ok());
    assert!(prepare_query(RemoteNodeQuery::Controllers {
        after: Some(vec![1; 16])
    })
    .is_ok());
    assert_eq!(
        prepare_change(&RemoteNodeChange::RevokeController {
            controller_identity_fingerprint: vec![1; 16]
        })
        .expect("valid identity")
        .kind(),
        core::RemoteControlRequestKind::RevokeController
    );
}

#[test]
fn controller_revoke_outcomes_preserve_refusal_and_busy() {
    use self::core::RemoteControlRevokeControllerOutcome as Outcome;
    assert_eq!(revoke_status(Outcome::Applied), RemoteChangeStatus::Applied);
    assert_eq!(
        revoke_status(Outcome::NotFound),
        RemoteChangeStatus::Unchanged
    );
    for (outcome, expected) in [
        (Outcome::Forbidden, RemoteManagementFailureStage::Permission),
        (Outcome::Busy, RemoteManagementFailureStage::Busy),
        (Outcome::Failed, RemoteManagementFailureStage::Request),
    ] {
        assert!(
            matches!(revoke_status(outcome), RemoteChangeStatus::Failed { stage, .. } if stage == expected)
        );
    }
}

fn entry(byte: u8) -> core::RemoteControlInterfaceEntry {
    core::RemoteControlInterfaceEntry {
        id: id(byte),
        kind: InterfaceKind::BluetoothAuto,
        mode: InterfaceMode::Full,
        connection: ConnectionState::Connected,
        enabled: true,
        tx_bytes: u64::MAX,
        rx_bytes: u64::MAX - 1,
        links: 2,
        rate_bytes_per_sec: 16,
    }
}

fn lora() -> RemoteLoRaProfile {
    RemoteLoRaProfile {
        region: RemoteLoRaRegion::Us915,
        frequency_hz: 915_000_000,
        spreading_factor: 7,
        bandwidth_hz: 125_000,
        coding_rate: 5,
        tx_power_dbm: 14,
        preamble_symbols: 8,
    }
}

#[test]
fn interface_page_preserves_counters_cursor_and_detects_nonprogress() {
    let mut inventory = core::RemoteControlInterfaceInventory::empty();
    inventory.push(entry(2)).expect("entry fits");
    inventory.push(entry(3)).expect("entry fits");
    inventory
        .set_continuation(core::RemoteControlInterfaceContinuation::More(
            core::RemoteControlInterfaceCursor::after(id(3)),
        ))
        .expect("valid continuation");
    let page = project_interfaces(inventory.clone(), Some(id(1))).expect("progress");
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].tx_bytes, u64::MAX);
    assert_eq!(page.entries[0].rx_bytes, u64::MAX - 1);
    assert_eq!(page.next, Some(id(3).as_bytes().to_vec()));
    for after in [2, 3, 4] {
        assert_eq!(
            project_interfaces(inventory.clone(), Some(id(after)))
                .unwrap_err()
                .0,
            RemoteManagementFailureStage::Response
        );
    }
    assert_eq!(
        project_interfaces(core::RemoteControlInterfaceInventory::empty(), Some(id(3)))
            .expect("empty final page"),
        RemoteInterfacePage {
            entries: vec![],
            next: None
        }
    );
}

#[test]
fn peer_read_distinguishes_unknown_interface_and_wrong_parent() {
    assert_eq!(
        project_peers(
            core::RemoteControlInterfacePeersOutcome::UnknownInterface,
            id(1),
            None
        )
        .unwrap_err()
        .0,
        RemoteManagementFailureStage::UnknownInterface
    );
    assert_eq!(
        project_peers(
            core::RemoteControlInterfacePeersOutcome::Page(
                core::RemoteControlInterfacePeerPage::empty(id(2))
            ),
            id(1),
            None
        )
        .unwrap_err()
        .0,
        RemoteManagementFailureStage::Response
    );
    let empty = project_peers(
        core::RemoteControlInterfacePeersOutcome::Page(
            core::RemoteControlInterfacePeerPage::empty(id(1)),
        ),
        id(1),
        None,
    )
    .expect("valid empty peers");
    assert!(empty.entries.is_empty());
}

#[test]
fn power_does_not_fabricate_battery_or_charging_state() {
    assert_eq!(
        project_power(PowerSnapshot::UNKNOWN),
        RemoteNodePower {
            battery_percent: None,
            external_power: RemoteExternalPower::Unknown
        }
    );
    for (charging, expected) in [
        (ChargingState::Unknown, RemoteExternalPower::Present),
        (ChargingState::Charging, RemoteExternalPower::Charging),
        (ChargingState::Idle, RemoteExternalPower::Idle),
    ] {
        assert_eq!(
            project_power(PowerSnapshot::new(
                Some(BatteryPercent::saturating(62)),
                ExternalPowerState::Present { charging }
            )),
            RemoteNodePower {
                battery_percent: Some(62),
                external_power: expected
            }
        );
    }
}

#[test]
fn configuration_projects_structured_lora_and_distinguishes_absence() {
    let requested = lora();
    let parsed = validation::lower_lora(&requested).expect("valid profile");
    let mut card = core::RemoteControlInterfaceCard::empty();
    card.set_config(parsed.as_str().expect("canonical profile"))
        .expect("profile fits");
    let RemoteInterfaceConfiguration::Available { card } =
        project_config(core::RemoteControlInterfaceConfigOutcome::Card(card))
    else {
        panic!("available card");
    };
    assert_eq!(card.lora_profile, Some(requested));
    assert_eq!(
        project_config(core::RemoteControlInterfaceConfigOutcome::UnknownInterface),
        RemoteInterfaceConfiguration::UnknownInterface
    );
    assert_eq!(
        project_groups(core::RemoteControlDiscoveryGroupsInventoryOutcome::Unsupported),
        RemoteDiscoveryGroups::Unavailable
    );
    assert_eq!(
        project_groups(core::RemoteControlDiscoveryGroupsInventoryOutcome::UnknownInterface),
        RemoteDiscoveryGroups::UnknownInterface
    );
}

#[test]
fn lora_uses_upstream_region_and_modulation_validation() {
    assert!(validation::lower_lora(&lora()).is_ok());
    let mut invalid = lora();
    invalid.frequency_hz = 1;
    assert!(validation::lower_lora(&invalid).is_err());
    invalid = lora();
    invalid.tx_power_dbm = 100;
    assert!(validation::lower_lora(&invalid).is_err());
    invalid = lora();
    invalid.preamble_symbols = 0;
    assert!(validation::lower_lora(&invalid).is_err());
    invalid = lora();
    invalid.spreading_factor = 4;
    assert!(validation::lower_lora(&invalid).is_err());
    invalid = lora();
    invalid.bandwidth_hz = 42;
    assert!(validation::lower_lora(&invalid).is_err());
    invalid = lora();
    invalid.coding_rate = 4;
    assert!(validation::lower_lora(&invalid).is_err());
}

#[test]
fn invalid_ids_and_unbounded_groups_never_enter_a_write_operation() {
    let mut input = ChangeRemoteNodeInput {
        target_identity_fingerprint: vec![1; 16],
        change: RemoteNodeChange::DiscoveryGroups {
            interface_id: id(1).as_bytes().to_vec(),
            groups: vec!["reticulum".to_owned()],
        },
    };
    assert_eq!(validate_change_input(&input), Ok(()));
    input.target_identity_fingerprint.push(0);
    assert!(validate_change_input(&input).is_err());
    input.target_identity_fingerprint.pop();
    for groups in [
        vec![],
        vec!["x".to_owned(); 5],
        vec!["a".to_owned(), "a".to_owned()],
        vec!["é".repeat(17)],
    ] {
        input.change = RemoteNodeChange::DiscoveryGroups {
            interface_id: id(1).as_bytes().to_vec(),
            groups,
        };
        assert!(validate_change_input(&input).is_err());
    }
    input.change = RemoteNodeChange::InterfacePower {
        interface_id: vec![1],
        enabled: true,
    };
    assert!(validate_change_input(&input).is_err());
    assert!(prepare_query(RemoteNodeQuery::Interfaces {
        after: Some(vec![1])
    })
    .is_err());
    assert!(prepare_query(RemoteNodeQuery::Peers {
        interface_id: id(1).as_bytes().to_vec(),
        after: Some(vec![1])
    })
    .is_err());
}

#[test]
fn writes_never_turn_lost_responses_into_safe_retry_failures() {
    for (failure, reason) in [
        (
            SendRequestFailure::Timeout,
            RemoteControlAnnounceUnknownReason::Timeout,
        ),
        (
            SendRequestFailure::LinkClosed,
            RemoteControlAnnounceUnknownReason::ConnectionLost,
        ),
        (
            SendRequestFailure::WriteFailed,
            RemoteControlAnnounceUnknownReason::DeliveryUnconfirmed,
        ),
        (
            SendRequestFailure::ResponseTooLarge,
            RemoteControlAnnounceUnknownReason::ResponseInvalid,
        ),
    ] {
        assert_eq!(
            mutation_failure(RemoteControlTargetOperationError::Exchange(
                RemoteControlError::Request(SendError::Failed(failure))
            )),
            RemoteChangeStatus::OutcomeUnknown { reason }
        );
    }
    assert!(matches!(
        mutation_failure(RemoteControlTargetOperationError::NotPermitted(
            core::RemoteControlRequestKind::SetGnssPower
        )),
        RemoteChangeStatus::Failed {
            stage: RemoteManagementFailureStage::Permission,
            ..
        }
    ));
    assert!(matches!(
        mutation_failure(RemoteControlTargetOperationError::Exchange(
            RemoteControlError::Request(SendError::Busy)
        )),
        RemoteChangeStatus::Failed {
            stage: RemoteManagementFailureStage::Busy,
            ..
        }
    ));
}

#[test]
fn remote_failure_stages_preserve_save_and_rollback_failures() {
    for (error, expected) in [
        (
            core::RemoteControlProtocolError::Busy {
                request: core::RemoteControlRequestKind::SetGnssPower,
            },
            RemoteManagementFailureStage::Busy,
        ),
        (
            core::RemoteControlProtocolError::PersistenceFailed {
                request: core::RemoteControlRequestKind::SetGnssPower,
            },
            RemoteManagementFailureStage::Persistence,
        ),
        (
            core::RemoteControlProtocolError::RollbackFailed {
                request: core::RemoteControlRequestKind::SetGnssPower,
            },
            RemoteManagementFailureStage::Rollback,
        ),
    ] {
        assert!(
            matches!(mutation_failure(RemoteControlTargetOperationError::Exchange(RemoteControlError::Remote(error))), RemoteChangeStatus::Failed { stage, .. } if stage == expected)
        );
    }
    assert_eq!(
        apply_status(core::RemoteControlApplyOutcome::Scheduled),
        RemoteChangeStatus::Scheduled
    );
    assert_eq!(
        apply_status(core::RemoteControlApplyOutcome::Unchanged),
        RemoteChangeStatus::Unchanged
    );
}

#[test]
fn every_change_requires_its_specific_live_capability() {
    let id = id(1).as_bytes().to_vec();
    let changes = [
        (
            RemoteNodeChange::InterfacePower {
                interface_id: id.clone(),
                enabled: true,
            },
            core::RemoteControlRequestKind::SetInterfacePower,
        ),
        (
            RemoteNodeChange::InterfaceMode {
                interface_id: id.clone(),
                mode: RemoteInterfaceMode::Roaming,
            },
            core::RemoteControlRequestKind::SetInterfaceMode,
        ),
        (
            RemoteNodeChange::InterfaceGroup {
                interface_id: id.clone(),
                group: "reticulum".to_owned(),
            },
            core::RemoteControlRequestKind::SetInterfaceGroup,
        ),
        (
            RemoteNodeChange::InterfaceLoRa {
                interface_id: id.clone(),
                profile: lora(),
            },
            core::RemoteControlRequestKind::SetInterfaceLoRaProfile,
        ),
        (
            RemoteNodeChange::DiscoveryGroups {
                interface_id: id.clone(),
                groups: vec!["reticulum".to_owned()],
            },
            core::RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
        ),
        (
            RemoteNodeChange::GnssPower { enabled: true },
            core::RemoteControlRequestKind::SetGnssPower,
        ),
        (
            RemoteNodeChange::DisplayVisibility { visible: true },
            core::RemoteControlRequestKind::SetDisplayVisibility,
        ),
        (
            RemoteNodeChange::DisplayAutoOff { enabled: true },
            core::RemoteControlRequestKind::SetDisplayAutoOff,
        ),
        (
            RemoteNodeChange::SystemPower { awake: true },
            core::RemoteControlRequestKind::SetSystemPower,
        ),
        (
            RemoteNodeChange::StationUplink {
                interface_id: id,
                enabled: true,
            },
            core::RemoteControlRequestKind::SetStationUplink,
        ),
        (
            RemoteNodeChange::RadioMode {
                mode: RemoteRadioMode::Bluetooth,
            },
            core::RemoteControlRequestKind::SetEspRadioMode,
        ),
        (
            RemoteNodeChange::SleepRadios,
            core::RemoteControlRequestKind::SleepRadios,
        ),
        (
            RemoteNodeChange::WakeRadios,
            core::RemoteControlRequestKind::WakeRadios,
        ),
        (
            RemoteNodeChange::RevokeController {
                controller_identity_fingerprint: vec![1; 16],
            },
            core::RemoteControlRequestKind::RevokeController,
        ),
    ];
    for (change, kind) in changes {
        assert_eq!(prepare_change(&change).expect("valid fixture").kind(), kind);
        assert!(require(
            &core::RemoteControlRequestSet::only(core::RemoteControlRequestKind::Describe),
            kind
        )
        .is_err());
        assert!(require(&core::RemoteControlRequestSet::only(kind), kind).is_ok());
    }
}
