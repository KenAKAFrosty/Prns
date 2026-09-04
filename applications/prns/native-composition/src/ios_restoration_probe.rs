#[cfg(any(feature = "ios-restoration-probe", test))]
mod enabled {
    #[cfg(any(test, target_os = "ios"))]
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    use std::sync::Once;

    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    use log::{Level, LevelFilter, Log, Metadata, Record};

    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    static INSTALL: Once = Once::new();
    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    static LOGGER: RestorationLogger = RestorationLogger;
    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    struct RestorationLogger;

    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    impl Log for RestorationLogger {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.level() <= Level::Debug && is_bluetooth_target(metadata.target())
        }

        fn log(&self, record: &Record<'_>) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let message = record.args().to_string();
            if let Some(code) = classify(record.target(), &message) {
                emit(next_sequence(&SEQUENCE), code);
            }
        }

        fn flush(&self) {}
    }

    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    pub(super) fn install() {
        INSTALL.call_once(|| {
            if log::set_logger(&LOGGER).is_ok() {
                log::set_max_level(LevelFilter::Debug);
                emit(next_sequence(&SEQUENCE), "logger_installed");
            } else {
                emit(next_sequence(&SEQUENCE), "logger_unavailable");
            }
        });
    }

    #[cfg(all(feature = "ios-restoration-probe", not(target_os = "ios")))]
    pub(super) fn install() {}

    #[cfg(any(test, target_os = "ios"))]
    fn is_bluetooth_target(target: &str) -> bool {
        matches!(
            target,
            "prns_ffi::bluetooth_auto::macos::central" | "prns_ffi::bluetooth_auto::macos::backend"
        )
    }

    #[cfg(any(test, target_os = "ios"))]
    fn classify(target: &str, message: &str) -> Option<&'static str> {
        // Mirror existing log sites without exporting their peer IDs, RSSI, or error payloads.
        // A scan-start code records the request; a sighting is emitted only after admission,
        // so absent sightings do not prove that CoreBluetooth delivered no callbacks.
        if !is_bluetooth_target(target) {
            return None;
        }
        if target.ends_with("::central") {
            match message {
                "bluetooth: dial connected over LE, discovering Prns service" => {
                    return Some("central_connected");
                }
                "bluetooth: native control characteristic found, subscribing" => {
                    return Some("central_control_subscribing");
                }
                _ => {}
            }
            for (prefix, code) in [
                ("bluetooth: dial connect FAILED: ", "central_connect_failed"),
                (
                    "bluetooth: service discovery FAILED: ",
                    "central_service_discovery_failed",
                ),
                (
                    "bluetooth: characteristic discovery FAILED: ",
                    "central_characteristic_discovery_failed",
                ),
                (
                    "bluetooth: subscribe FAILED: ",
                    "central_subscription_failed",
                ),
                (
                    "bluetooth: central-role peripheral disconnected: ",
                    "central_disconnected",
                ),
            ] {
                if message.starts_with(prefix) {
                    return Some(code);
                }
            }
            if message.starts_with("bluetooth: ")
                && message.ends_with(" subscribed — native control ready")
            {
                return Some("central_control_subscribed");
            }
            if message.starts_with("bluetooth: ")
                && message.ends_with(" subscribed — Columba data path ready")
            {
                return Some("central_columba_subscribed");
            }
            if message.starts_with("bluetooth: restored peripheral ")
                && message.ends_with(" from a background relaunch — re-adopting")
            {
                return Some("central_state_restored");
            }
            if message.starts_with("bluetooth: restored GATT notification buffer exceeded for ") {
                return Some("central_data_buffer_overflow");
            }
            if message.starts_with("bluetooth: restored control buffer exceeded for ") {
                return Some("central_control_buffer_overflow");
            }
        }
        if target.ends_with("::backend") {
            match message {
                "bluetooth: central-only CoreBluetooth manager powered; no local peripheral capability" =>
                {
                    return Some("central_manager_ready");
                }
                "bluetooth: timed out waiting for central power — is Bluetooth on and permission granted?" =>
                {
                    return Some("central_manager_timeout");
                }
                "bluetooth: CoreBluetooth logical radio resources up" => {
                    return Some("central_radio_enabled");
                }
                "bluetooth: CoreBluetooth logical radio resources down" => {
                    return Some("central_radio_disabled");
                }
                "bluetooth: scanning for Prns peers" => return Some("central_scan_started"),
                "bluetooth: restarted Prns scan so late-arriving peers can be sighted" => {
                    return Some("central_scan_restarted");
                }
                "bluetooth: scanning stopped — at connection capacity" => {
                    return Some("central_scan_stopped");
                }
                _ => {}
            }
            if message.starts_with("bluetooth: sighted Prns peer ") {
                return Some("central_peer_sighted");
            }
            for (prefix, suffix, code) in [
                (
                    "bluetooth: dialing ",
                    " over LE (central role)",
                    "central_dial_started",
                ),
                (
                    "bluetooth: yielding dial to ",
                    " — peer is already connected system-wide outside this manager's restored state",
                    "central_dial_system_connection_yielded",
                ),
                (
                    "bluetooth: yielding dial to ",
                    " — this peer already owns an inbound peripheral session",
                    "central_dial_inbound_session_yielded",
                ),
                (
                    "bluetooth: dial to ",
                    " — peripheral not yet sighted",
                    "central_dial_missing_peripheral",
                ),
                (
                    "bluetooth: dial to ",
                    " did not reach control-ready",
                    "central_dial_failed",
                ),
                (
                    "bluetooth: dial to ",
                    " closed before reaching control-ready",
                    "central_dial_failed",
                ),
                (
                    "bluetooth: dial to ",
                    " timed out before reaching control-ready",
                    "central_dial_timeout",
                ),
                (
                    "bluetooth: resumed restored connection to ",
                    ", discovering Prns service",
                    "central_session_resumed",
                ),
                (
                    "bluetooth: resumed pending connection to ",
                    ", awaiting CoreBluetooth completion",
                    "central_pending_connection_resumed",
                ),
            ] {
                if message.starts_with(prefix) && message.ends_with(suffix) {
                    return Some(code);
                }
            }
        }
        None
    }

    #[cfg(any(test, target_os = "ios"))]
    fn next_sequence(counter: &AtomicU64) -> u64 {
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            })
            .map_or_else(|value| value, |previous| previous.saturating_add(1))
    }

    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    fn emit(sequence: u64, code: &'static str) {
        // SAFETY: `code` is one of the private static ASCII event codes above, and Swift copies
        // the bounded bytes during this call. The symbol is compiled into Debug iOS app builds
        // whenever this Rust feature is enabled.
        unsafe {
            prns_app_ios_restoration_probe_emit(sequence, code.as_ptr(), code.len());
        }
    }

    #[cfg(all(feature = "ios-restoration-probe", target_os = "ios"))]
    unsafe extern "C" {
        fn prns_app_ios_restoration_probe_emit(sequence: u64, code_ptr: *const u8, code_len: usize);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const CENTRAL: &str = "prns_ffi::bluetooth_auto::macos::central";
        const BACKEND: &str = "prns_ffi::bluetooth_auto::macos::backend";
        const DIAGNOSTIC_CASES: &[(&str, &str, &str)] = &[
            (
                CENTRAL,
                "bluetooth: dial connected over LE, discovering Prns service",
                "central_connected",
            ),
            (
                CENTRAL,
                "bluetooth: native control characteristic found, subscribing",
                "central_control_subscribing",
            ),
            (
                CENTRAL,
                "bluetooth: dial connect FAILED: private-error",
                "central_connect_failed",
            ),
            (
                CENTRAL,
                "bluetooth: service discovery FAILED: private-error",
                "central_service_discovery_failed",
            ),
            (
                CENTRAL,
                "bluetooth: characteristic discovery FAILED: private-error",
                "central_characteristic_discovery_failed",
            ),
            (
                CENTRAL,
                "bluetooth: subscribe FAILED: private-error",
                "central_subscription_failed",
            ),
            (
                CENTRAL,
                "bluetooth: central-role peripheral disconnected: private-error",
                "central_disconnected",
            ),
            (
                CENTRAL,
                "bluetooth: private-peer subscribed — native control ready",
                "central_control_subscribed",
            ),
            (
                CENTRAL,
                "bluetooth: private-peer subscribed — Columba data path ready",
                "central_columba_subscribed",
            ),
            (
                BACKEND,
                "bluetooth: central-only CoreBluetooth manager powered; no local peripheral capability",
                "central_manager_ready",
            ),
            (
                BACKEND,
                "bluetooth: timed out waiting for central power — is Bluetooth on and permission granted?",
                "central_manager_timeout",
            ),
            (
                BACKEND,
                "bluetooth: CoreBluetooth logical radio resources up",
                "central_radio_enabled",
            ),
            (
                BACKEND,
                "bluetooth: CoreBluetooth logical radio resources down",
                "central_radio_disabled",
            ),
            (
                BACKEND,
                "bluetooth: scanning for Prns peers",
                "central_scan_started",
            ),
            (
                BACKEND,
                "bluetooth: restarted Prns scan so late-arriving peers can be sighted",
                "central_scan_restarted",
            ),
            (
                BACKEND,
                "bluetooth: scanning stopped — at connection capacity",
                "central_scan_stopped",
            ),
            (
                BACKEND,
                "bluetooth: sighted Prns peer private-peer rssi=Some(-47)",
                "central_peer_sighted",
            ),
            (
                BACKEND,
                "bluetooth: dialing private-peer over LE (central role)",
                "central_dial_started",
            ),
            (
                BACKEND,
                "bluetooth: yielding dial to private-peer — peer is already connected system-wide outside this manager's restored state",
                "central_dial_system_connection_yielded",
            ),
            (
                BACKEND,
                "bluetooth: yielding dial to private-peer — this peer already owns an inbound peripheral session",
                "central_dial_inbound_session_yielded",
            ),
            (
                BACKEND,
                "bluetooth: dial to private-peer — peripheral not yet sighted",
                "central_dial_missing_peripheral",
            ),
            (
                BACKEND,
                "bluetooth: dial to private-peer did not reach control-ready",
                "central_dial_failed",
            ),
            (
                BACKEND,
                "bluetooth: dial to private-peer closed before reaching control-ready",
                "central_dial_failed",
            ),
            (
                BACKEND,
                "bluetooth: dial to private-peer timed out before reaching control-ready",
                "central_dial_timeout",
            ),
            (
                BACKEND,
                "bluetooth: resumed pending connection to private-peer, awaiting CoreBluetooth completion",
                "central_pending_connection_resumed",
            ),
        ];

        #[test]
        fn ordinary_bluetooth_messages_become_payload_free_codes() {
            for &(target, message, code) in DIAGNOSTIC_CASES {
                assert_eq!(classify(target, message), Some(code), "{message}");
                let other_payload = message
                    .replace("private-peer", "another-private-peer")
                    .replace("private-error", "another-private-error");
                assert_eq!(classify(target, &other_payload), Some(code));
                let other_target = if target == CENTRAL { BACKEND } else { CENTRAL };
                assert_eq!(classify(other_target, message), None);
            }
        }

        #[test]
        fn restoration_messages_become_payload_free_codes() {
            assert_eq!(
                classify(
                    CENTRAL,
                    "bluetooth: restored peripheral [01, 23, 45, 67, 89, ab] from a background relaunch — re-adopting",
                ),
                Some("central_state_restored")
            );
            assert_eq!(
                classify(
                    BACKEND,
                    "bluetooth: resumed restored connection to [01, 23, 45, 67, 89, ab], discovering Prns service",
                ),
                Some("central_session_resumed")
            );
            assert_eq!(
                classify(
                    CENTRAL,
                    "bluetooth: restored GATT notification buffer exceeded for [01, 23, 45, 67, 89, ab]",
                ),
                Some("central_data_buffer_overflow")
            );
            assert_eq!(
                classify(
                    CENTRAL,
                    "bluetooth: restored control buffer exceeded for [01, 23, 45, 67, 89, ab]",
                ),
                Some("central_control_buffer_overflow")
            );
        }

        #[test]
        fn unrelated_targets_and_messages_are_rejected() {
            assert_eq!(
                classify(
                    "other::bluetooth_auto::macos::central",
                    "bluetooth: restored peripheral secret from a background relaunch — re-adopting",
                ),
                None
            );
            assert_eq!(
                classify(CENTRAL, "bluetooth: scanning for Prns peers"),
                None
            );
        }

        #[test]
        fn event_codes_are_bounded_ascii_and_do_not_retain_peer_data() {
            let codes = [
                "central_state_restored",
                "central_data_buffer_overflow",
                "central_control_buffer_overflow",
                "central_session_resumed",
                "logger_installed",
                "logger_unavailable",
            ];
            for code in codes
                .into_iter()
                .chain(DIAGNOSTIC_CASES.iter().map(|(_, _, code)| *code))
            {
                assert!(code.len() <= 64);
                assert!(code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
                assert!(!code.contains("0123456789"));
                assert!(!code.contains("private"));
            }
        }

        #[test]
        fn swift_allowlist_accepts_exactly_the_tested_event_codes() {
            let swift = include_str!("../../../sdk/expo/ios/PrnsAppRestorationProbe.swift");
            let allowlist = swift
                .split_once("private let prnsRestorationEvents: Set<String> = [")
                .expect("Swift must retain the explicit event allowlist")
                .1
                .split_once(']')
                .expect("Swift event allowlist must close")
                .0;
            let actual = allowlist
                .lines()
                .filter_map(|line| line.trim().strip_prefix('"')?.strip_suffix("\","))
                .collect::<std::collections::BTreeSet<_>>();
            let expected = [
                "central_state_restored",
                "central_data_buffer_overflow",
                "central_control_buffer_overflow",
                "central_session_resumed",
                "logger_installed",
                "logger_unavailable",
            ]
            .into_iter()
            .chain(DIAGNOSTIC_CASES.iter().map(|(_, _, code)| *code))
            .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(actual, expected);
            assert!(swift.starts_with("#if DEBUG\n"));
            assert!(swift.trim_end().ends_with("#endif"));
        }

        #[test]
        fn sequence_is_monotonic_and_saturates() {
            let counter = AtomicU64::new(0);
            assert_eq!(next_sequence(&counter), 1);
            assert_eq!(next_sequence(&counter), 2);
            counter.store(u64::MAX, Ordering::Relaxed);
            assert_eq!(next_sequence(&counter), u64::MAX);
            assert_eq!(next_sequence(&counter), u64::MAX);
        }
    }
}

pub(crate) fn install() {
    #[cfg(feature = "ios-restoration-probe")]
    enabled::install();
}
