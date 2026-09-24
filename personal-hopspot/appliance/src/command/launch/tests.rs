use super::*;

const KEY: &str = concat!(
    "abababababababababababababababab",
    "abababababababababababababababab",
    "abababababababababababababababab",
    "abababababababababababababababab",
);

fn configuration() -> LaunchConfiguration {
    serde_json::from_str(
        r#"{"listen":"[::]:4347","tcp_mode":"Gateway","radio":{"HaLow":{"device":"wlan0","scope":"primary-halow"}}}"#,
    )
    .unwrap()
}

#[test]
fn a_service_restart_cannot_repeat_enrollment_or_qualification_policy() {
    assert_eq!(
        configuration()
            .arguments(Path::new("/etc/hopspot/state"), Purpose::Service)
            .unwrap(),
        [
            "--state-dir",
            "/etc/hopspot/state",
            "--listen",
            "[::]:4347",
            "--tcp-mode",
            "gateway",
            "--halow-device",
            "wlan0",
            "--halow-scope",
            "primary-halow",
        ]
    );
}

#[test]
fn qualification_delivers_only_the_selected_initial_grant_and_a_bounded_lifetime() {
    for (access, flag) in [
        (ControllerAccess::Inspection, "--controller-public-key"),
        (
            ControllerAccess::ApplicationProbe,
            "--app-controller-public-key",
        ),
        (
            ControllerAccess::InterfaceWatch,
            "--watch-controller-public-key",
        ),
    ] {
        let arguments = configuration()
            .arguments(
                Path::new("/etc/hopspot/state"),
                Purpose::Qualification {
                    controller: KEY.parse().unwrap(),
                    access,
                    window: "180".parse().unwrap(),
                },
            )
            .unwrap();
        let expected = [
            "--state-dir",
            "/etc/hopspot/state",
            "--listen",
            "[::]:4347",
            "--tcp-mode",
            "gateway",
            "--halow-device",
            "wlan0",
            "--halow-scope",
            "primary-halow",
            flag,
            KEY,
            "--run-for",
            "180",
        ];
        assert_eq!(arguments, expected);
    }
}

#[test]
fn qualification_refuses_missing_or_unbounded_policy_and_malformed_public_keys() {
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
        "qualify",
        "--config",
        "/etc/hopspot/config.json",
        "--ram-directory",
        "/tmp/hopspot",
    ];
    let policy = [
        "--controller-public-key",
        KEY,
        "--controller-access",
        "application-probe",
        "--run-for",
        "180",
    ];
    assert!(Options::try_parse_from(common.into_iter().chain(policy)).is_ok());
    for missing in [
        "--controller-public-key",
        "--controller-access",
        "--run-for",
    ] {
        let retained = policy
            .as_chunks::<2>()
            .0
            .iter()
            .filter(|pair| pair[0] != missing);
        assert!(
            Options::try_parse_from(common.into_iter().chain(retained.flatten().copied())).is_err()
        );
    }
    for invalid in ["", "0", "301", "65536", "-1"] {
        assert!(invalid.parse::<QualificationWindow>().is_err());
    }
    for invalid in ["", "21", &"x".repeat(128), &format!("{KEY}00")] {
        assert!(invalid.parse::<ControllerPublicKey>().is_err());
    }
    assert_eq!(
        KEY.to_uppercase().parse::<ControllerPublicKey>().unwrap().0,
        KEY
    );
}
