#[cfg(feature = "ios-restoration-probe")]
mod enabled {
    #[cfg(any(test, target_os = "ios"))]
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(target_os = "ios")]
    use std::sync::Once;

    #[cfg(target_os = "ios")]
    use log::{Level, LevelFilter, Log, Metadata, Record};

    #[cfg(target_os = "ios")]
    static INSTALL: Once = Once::new();
    #[cfg(target_os = "ios")]
    static LOGGER: RestorationLogger = RestorationLogger;
    #[cfg(target_os = "ios")]
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[cfg(target_os = "ios")]
    struct RestorationLogger;

    #[cfg(target_os = "ios")]
    impl Log for RestorationLogger {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.level() <= Level::Debug && is_restoration_target(metadata.target())
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

    #[cfg(target_os = "ios")]
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

    #[cfg(not(target_os = "ios"))]
    pub(super) fn install() {}

    #[cfg(any(test, target_os = "ios"))]
    fn is_restoration_target(target: &str) -> bool {
        matches!(
            target,
            "prns_ffi::bluetooth_auto::macos::central"
                | "prns_ffi::bluetooth_auto::macos::peripheral"
                | "prns_ffi::bluetooth_auto::macos::backend"
        )
    }

    #[cfg(any(test, target_os = "ios"))]
    fn classify(target: &str, message: &str) -> Option<&'static str> {
        if !is_restoration_target(target) {
            return None;
        }
        if target.ends_with("::central") {
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
        if target.ends_with("::peripheral")
            && message
                == "bluetooth: restored the published Prns GATT service from a background relaunch"
        {
            return Some("peripheral_service_restored");
        }
        if target.ends_with("::backend")
            && message.starts_with("bluetooth: resumed restored connection to ")
            && message.ends_with(", discovering Prns service")
        {
            return Some("central_session_resumed");
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

    #[cfg(target_os = "ios")]
    fn emit(sequence: u64, code: &'static str) {
        // SAFETY: `code` is one of the private static ASCII event codes above, and Swift copies
        // the bounded bytes during this call. The symbol is compiled into Debug iOS app builds
        // whenever this Rust feature is enabled.
        unsafe {
            prns_app_ios_restoration_probe_emit(sequence, code.as_ptr(), code.len());
        }
    }

    #[cfg(target_os = "ios")]
    unsafe extern "C" {
        fn prns_app_ios_restoration_probe_emit(sequence: u64, code_ptr: *const u8, code_len: usize);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const CENTRAL: &str = "prns_ffi::bluetooth_auto::macos::central";
        const PERIPHERAL: &str = "prns_ffi::bluetooth_auto::macos::peripheral";
        const BACKEND: &str = "prns_ffi::bluetooth_auto::macos::backend";

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
                    PERIPHERAL,
                    "bluetooth: restored the published Prns GATT service from a background relaunch",
                ),
                Some("peripheral_service_restored")
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
            let messages = [
                "central_state_restored",
                "central_data_buffer_overflow",
                "central_control_buffer_overflow",
                "peripheral_service_restored",
                "central_session_resumed",
                "logger_installed",
                "logger_unavailable",
            ];
            for code in messages {
                assert!(code.len() <= 64);
                assert!(code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
                assert!(!code.contains("0123456789"));
            }
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
