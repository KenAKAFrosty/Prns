use super::launch::LaunchConfiguration;
use super::*;

#[test]
fn launch_configuration_has_no_enrollment_announce_or_arbitrary_command_surface() {
    let approved = br#"{"listen":"0.0.0.0:4242","tcp_mode":"Gateway","radio":"Disabled"}"#;
    assert!(serde_json::from_slice::<LaunchConfiguration>(approved).is_ok());
    for forbidden in [
        "controller_key",
        "announce_interval",
        "command",
        "arguments",
    ] {
        let mut configuration: serde_json::Value = serde_json::from_slice(approved).unwrap();
        configuration[forbidden] = serde_json::json!("injected");
        assert!(serde_json::from_value::<LaunchConfiguration>(configuration).is_err());
    }
    for missing in ["listen", "tcp_mode", "radio"] {
        let mut configuration: serde_json::Value = serde_json::from_slice(approved).unwrap();
        configuration.as_object_mut().unwrap().remove(missing);
        assert!(serde_json::from_value::<LaunchConfiguration>(configuration).is_err());
    }
}

#[test]
fn activation_requires_explicit_resource_and_trial_policy() {
    assert!(Options::try_parse_from(["manager", "--root", "/etc/hopspot", "status"]).is_err());
    let common = [
        "manager",
        "--root",
        "/etc/hopspot",
        "--max-compressed-bytes",
        "2097152",
        "--max-executable-bytes",
        "4194304",
        "--flash-reserve-bytes",
        "262144",
        "--ram-reserve-bytes",
        "8388608",
    ];
    assert!(Options::try_parse_from(common.into_iter().chain([
        "stage",
        "--package",
        "/tmp/package"
    ]))
    .is_err());
    assert!(Options::try_parse_from(common.into_iter().chain([
        "stage",
        "--package",
        "/tmp/package",
        "--trial-launches",
        "0"
    ]))
    .is_err());
    assert!(Options::try_parse_from(common.into_iter().chain([
        "stage",
        "--package",
        "/tmp/package",
        "--trial-launches",
        "3"
    ]))
    .is_ok());
}

#[test]
fn radio_trials_require_explicit_policy_and_exact_generation_confirmation() {
    let common = [
        "manager",
        "--root",
        "/etc/hopspot",
        "--max-compressed-bytes",
        "2097152",
        "--max-executable-bytes",
        "4194304",
        "--flash-reserve-bytes",
        "262144",
        "--ram-reserve-bytes",
        "8388608",
        "radio",
    ];
    for missing in ["--recovery-seconds", "--trial-boots"] {
        let arguments = [
            "prepare",
            "--profile",
            "/tmp/profile",
            "--recovery-seconds",
            "180",
            "--trial-boots",
            "1",
        ];
        let mut retained = Vec::new();
        let mut iter = arguments.into_iter();
        while let Some(argument) = iter.next() {
            if argument == missing {
                iter.next();
            } else {
                retained.push(argument);
            }
        }
        assert!(Options::try_parse_from(common.into_iter().chain(retained)).is_err());
    }
    assert!(Options::try_parse_from(common.into_iter().chain([
        "confirm",
        "--revision",
        "0",
        "--profile-sha256",
        &"a".repeat(64)
    ]))
    .is_err());
    assert!(Options::try_parse_from(common.into_iter().chain([
        "confirm",
        "--revision",
        "1",
        "--profile-sha256",
        &"a".repeat(64)
    ]))
    .is_ok());
    assert!(Options::try_parse_from(common.into_iter().chain(["shell", "reboot"])).is_err());
    assert!(Options::try_parse_from(common.into_iter().chain([
        "--qualification-radio-observation",
        "pause-after-wireless-replacement",
        "status",
    ]))
    .is_err());
}
