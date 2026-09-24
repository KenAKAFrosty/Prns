use super::*;
use clap::Parser;

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    radio: HaLowOptions,
}

#[test]
fn radio_is_opt_in_and_scope_and_limits_are_validated_before_binding() {
    let empty = Cli::try_parse_from(["test"]).unwrap();
    assert!(empty.radio.prepare().unwrap().is_none());
    for args in [
        vec!["--halow-device", "wlan0"],
        vec!["--halow-scope", "radio"],
        vec!["--halow-device", "wlan0", "--halow-scope", ""],
        vec![
            "--halow-device",
            "wlan0",
            "--halow-scope",
            "radio",
            "--halow-peers",
            "0",
        ],
        vec![
            "--halow-device",
            "wlan0",
            "--halow-scope",
            "radio",
            "--halow-idle-seconds",
            "0",
        ],
        vec![
            "--halow-device",
            "wlan0",
            "--halow-scope",
            "radio",
            "--halow-idle-seconds",
            "86401",
        ],
    ] {
        assert!(Cli::try_parse_from(std::iter::once("test").chain(args)).is_err());
    }
    assert!(scope(&"x".repeat(65)).is_err());
    let configured =
        Cli::try_parse_from(["test", "--halow-device", "wlan0", "--halow-scope", "radio"]).unwrap();
    assert_eq!(configured.radio.halow_peers.get(), 16);
    assert_eq!(configured.radio.halow_idle_seconds.get(), 900);
    #[cfg(not(target_os = "linux"))]
    assert!(matches!(
        configured.radio.prepare(),
        Err(Error::Unsupported)
    ));
}
