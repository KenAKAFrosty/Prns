use super::*;

fn profile() -> RadioProfile {
    serde_json::from_str(include_str!("../../openwrt/radio-profile.example.json")).unwrap()
}

#[test]
fn profile_requires_an_explicit_qualified_region_and_preset() {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("../../openwrt/radio-profile.example.json")).unwrap();
    for missing in ["binding", "channel", "preset", "mesh_paths", "mesh_id"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(missing);
        assert!(serde_json::from_value::<RadioProfile>(invalid).is_err());
    }
    for (field, unsupported) in [("channel", "Eu868Mhz8Mhz"), ("preset", "Auto")] {
        let mut invalid = value.clone();
        invalid[field] = unsupported.into();
        assert!(serde_json::from_value::<RadioProfile>(invalid).is_err());
    }
    let mut invalid = value;
    invalid["announce_interval"] = 1.into();
    assert!(serde_json::from_value::<RadioProfile>(invalid).is_err());
}

#[test]
fn a_vendor_upgrade_requires_requalification_before_planning() {
    for board in [Board::ThinkNodeG4, Board::HeltecHtHd01V2] {
        assert_eq!(
            profile().plan(&board, b"changed vendor adapter"),
            Err(RadioProfileError::BootAdapter)
        );
    }
}

#[test]
fn binding_and_mesh_identifiers_cannot_inject_uci_commands() {
    for bad in [
        "",
        "radio1.country",
        "@wifi-device[0]",
        "radio1\ncommit",
        "';exec",
    ] {
        assert_eq!(
            UciSection::try_from(bad.to_owned()),
            Err(RadioProfileError::Section)
        );
    }
    for bad in ["", "wlan0\n", "wlan0.1", "1234567890123456"] {
        assert_eq!(
            RadioDevice::try_from(bad.to_owned()),
            Err(RadioProfileError::Device)
        );
    }
    for bad in [
        "",
        "';commit",
        "two words",
        "123456789012345678901234567890123",
    ] {
        assert_eq!(
            MeshId::try_from(bad.to_owned()),
            Err(RadioProfileError::MeshId)
        );
    }
}

#[test]
fn plan_changes_only_the_bound_radio_and_mesh_policy_without_committing() {
    let plan = profile().qualified_plan(&Board::ThinkNodeG4, G4_BOOT_ADAPTER_SHA256.to_owned());
    assert_eq!(
        plan.uci_batch,
        concat!(
            "set wireless.radio1.country='US'\n",
            "set wireless.radio1.channel='44'\n",
            "set wireless.radio1.disabled='0'\n",
            "set wireless.radio1.enable_fixed_rate='1'\n",
            "set wireless.radio1.fixed_mcs='2'\n",
            "set wireless.radio1.fixed_bw='3'\n",
            "set wireless.radio1.fixed_ss='1'\n",
            "set wireless.radio1.fixed_guard='0'\n",
            "set wireless.radio1.enable_ps='0'\n",
            "set wireless.radio1.txpower='18'\n",
            "del_list wireless.radio1.s1g_capab='[SHORT-GI-NONE]'\n",
            "add_list wireless.radio1.s1g_capab='[SHORT-GI-NONE]'\n",
            "set wireless.default_radio1.ifname='wlan0'\n",
            "set wireless.default_radio1.mode='mesh'\n",
            "set wireless.default_radio1.mesh_id='Hopspot'\n",
            "set wireless.default_radio1.encryption='none'\n",
            "set wireless.default_radio1.powersave='0'\n",
            "set wireless.default_radio1.wds='0'\n",
            "delete wireless.default_radio1.key\n",
            "delete wireless.default_radio1.network\n",
            "set mesh11sd.mesh_params.mesh_fwding='0'\n",
            "set mesh11sd.mesh_params.mesh_hwmp_rootmode='2'\n",
            "set mesh11sd.mesh_params.mesh_gate_announcements='0'\n",
        )
    );
    for line in plan.uci_batch.lines() {
        let key = line
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split('=')
            .next()
            .unwrap();
        assert!(
            key.starts_with("wireless.radio1.")
                || key.starts_with("wireless.default_radio1.")
                || key.starts_with("mesh11sd.mesh_params.")
        );
    }
}

#[test]
fn candidate_projection_rejects_partial_batches_credentials_and_duplicate_guard_options() {
    let plan = profile().qualified_plan(&Board::ThinkNodeG4, G4_BOOT_ADAPTER_SHA256.to_owned());
    let projection = concat!(
        "wireless.radio1.country='US'\n",
        "wireless.radio1.channel='44'\n",
        "wireless.radio1.disabled='0'\n",
        "wireless.radio1.enable_fixed_rate='1'\n",
        "wireless.radio1.fixed_mcs='2'\n",
        "wireless.radio1.fixed_bw='3'\n",
        "wireless.radio1.fixed_ss='1'\n",
        "wireless.radio1.fixed_guard='0'\n",
        "wireless.radio1.enable_ps='0'\n",
        "wireless.radio1.txpower='18'\n",
        "wireless.radio1.s1g_capab='[OTHER]' '[SHORT-GI-NONE]'\n",
        "wireless.default_radio1.ifname='wlan0'\n",
        "wireless.default_radio1.mode='mesh'\n",
        "wireless.default_radio1.mesh_id='Hopspot'\n",
        "wireless.default_radio1.encryption='none'\n",
        "wireless.default_radio1.powersave='0'\n",
        "wireless.default_radio1.wds='0'\n",
        "mesh11sd.mesh_params.mesh_fwding='0'\n",
        "mesh11sd.mesh_params.mesh_hwmp_rootmode='2'\n",
        "mesh11sd.mesh_params.mesh_gate_announcements='0'\n",
    );
    assert_eq!(plan.verify_uci_projection(projection), Ok(()));
    for damaged in [
        projection.replace("fixed_mcs='2'", "fixed_mcs='4'"),
        format!("{projection}wireless.default_radio1.network='ahwlan'\n"),
        projection.replace("mesh_hwmp_rootmode='2'", "mesh_hwmp_rootmode='0'"),
        projection.replace("'[SHORT-GI-NONE]'", "'[SHORT-GI-NONE]' '[SHORT-GI-NONE]'"),
        format!("{projection}wireless.default_radio1.key='test credential'\n"),
    ] {
        assert_eq!(
            plan.verify_uci_projection(&damaged),
            Err(RadioProfileError::Projection)
        );
    }
}
