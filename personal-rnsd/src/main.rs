//! Personal Reticulum daemon: drive the engine over a real USB-serial
//! interface — the host end of a cable to an ESP32-C6 (or any stock RNS
//! serial peer). Each step reads at most one de-framed packet off the wire,
//! lets the engine ingest it, and pumps any due egress back out.

use std::time::{Duration, Instant};

use personal_rns::engine::{
    DefaultEngineState, EngineDriver, InstantMillis, NextScheduledWakeup, ReannounceSchedule,
    SelfAnnounceConfig,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::{ConnectionState, Interface, InterfaceId};
use personal_rns::wire::MTU;
use personal_rnsd::{SerialUsbInterface, UsbHostExampleEngineDriver};

/// Stable id for the daemon's USB-serial interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);

/// The destination this daemon announces itself as. `personal.node` is the
/// node-level aspect; the engine derives its hash via `expand_name`.
const SELF_ANNOUNCE_APP_NAME: &str = "personal";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["node"];
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-rnsd";

/// The daemon's identity secret key (the 64 bytes that *are* its X25519 ‖
/// Ed25519 private keys). Handed to the engine through a [`Zeroizing`] buffer so
/// it is wiped from this stack frame once construction copies it in.
fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);

    #[cfg(feature = "fixture-identity")]
    {
        // Deterministic bring-up / HITL identity: the same X25519 0x22 ‖
        // Ed25519 0x11 keypair the personal-rns oracle vectors pin, so the
        // daemon announces the known `personal.node` destination
        // (c3cfae69b36bb6e3bbfd96a3b5867a59). Never ship this — every
        // fixture-identity node shares one identity.
        key[..32].fill(0x22);
        key[32..].fill(0x11);
    }

    #[cfg(not(feature = "fixture-identity"))]
    {
        // A fresh OS-CSPRNG identity each run. Persisting it across restarts
        // (file / keyring, the daemon-key story sketched in the engine roadmap)
        // lands with the storage work; until then, a restart is a new node.
        getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    }

    key
}

const USB_RECONNECT_INTERVAL: Duration = Duration::from_millis(500);

/// Upper bound on how long a single read may block before the loop loops back to
/// re-check the link's health (so an unplug is noticed and we reconnect).
/// Caps `WakeOnDeadline` so a far-off deadline or an idle engine still yields to
/// that check. A USB host is mains-powered, so this cap costs nothing here; the
/// deep-sleep payoff is the embedded story.
const MAX_BLOCKING_WAIT: Duration = Duration::from_secs(1);

// REVISIT(host-runtime-api): `WaitMode` + the loop below are an AD-HOC,
// daemon-local wiring of a behavior that is NOT daemon-specific — every host
// (this daemon, the C6 spike loop, a future tokio/embassy runtime) re-derives
// "how long do I wait between steps" by hand. That is the same shape as the
// identity-custody work we dropped: a technique proven inside `personal-rnsd`
// rather than baked into the shared, structured Runtime/Manifold/EngineDriver
// layer where it belongs (one implementation, every host). This keeps coming
// up — lift `WaitMode` (and the deadline→wait translation) into that layer soon
// so hosts *configure* the runtime instead of re-implementing its loop.
//
/// How the runtime loop decides how long to wait between engine steps. The
/// engine is passive and (by design) has no reason to wake except its own
/// scheduled work or an inbound packet, so the default drives the loop straight
/// off [`EngineState::next_wakeup`] — the read blocks until that deadline or a
/// packet, whichever comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitMode {
    /// Wake on the engine's next scheduled deadline, or an inbound packet — the
    /// default; the loop only runs when there is something to do.
    WakeOnDeadline,
    /// Always wait a fixed period regardless of schedule. The pre-deadline poll,
    /// kept for comparison and deterministic tests.
    FixedInterval(Duration),
}

impl WaitMode {
    fn read_timeout(self, next: NextScheduledWakeup, now: InstantMillis) -> Duration {
        match self {
            WaitMode::FixedInterval(period) => period,
            WaitMode::WakeOnDeadline => match next {
                NextScheduledWakeup::Immediate => Duration::ZERO,
                NextScheduledWakeup::At(deadline) => {
                    Duration::from_millis(deadline.0.saturating_sub(now.0)).min(MAX_BLOCKING_WAIT)
                }
                NextScheduledWakeup::Idle => MAX_BLOCKING_WAIT,
            },
        }
    }
}

/// `RNSD_POLL_INTERVAL_MS=<n>` forces the legacy fixed-interval poll (for
/// comparison / tests); unset uses the deadline-driven default.
fn wait_mode_from_env() -> WaitMode {
    match std::env::var("RNSD_POLL_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(ms) => WaitMode::FixedInterval(Duration::from_millis(ms)),
        None => WaitMode::WakeOnDeadline,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsbLifecycleAction {
    Continue,
    Reopen { marker: &'static str },
}

fn usb_lifecycle_action(state: ConnectionState) -> UsbLifecycleAction {
    match state {
        ConnectionState::Connected | ConnectionState::Degraded => UsbLifecycleAction::Continue,
        ConnectionState::Initializing | ConnectionState::Reconnecting => {
            UsbLifecycleAction::Reopen {
                marker: "RNSD_USB_NOT_ROUTABLE",
            }
        }
        ConnectionState::Failed => UsbLifecycleAction::Reopen {
            marker: "RNSD_USB_FAILED",
        },
        ConnectionState::Disconnected => UsbLifecycleAction::Reopen {
            marker: "RNSD_USB_DISCONNECTED",
        },
    }
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: personal-rnsd <serial-device>   (e.g. /dev/ttyACM0)");
        std::process::exit(2);
    };

    let clock = Instant::now();

    // Build an announcing node: it both forwards others' announces AND emits its
    // own `personal.node` announce on the schedule (default 6h), the first one as
    // soon as the interface is registered below.
    let identity_secret_key = load_identity_secret_key();
    let mut state: DefaultEngineState = DefaultEngineState::announcing(
        &identity_secret_key,
        SelfAnnounceConfig {
            app_name: SELF_ANNOUNCE_APP_NAME,
            aspects: SELF_ANNOUNCE_ASPECTS,
            app_data: SELF_ANNOUNCE_APP_DATA,
            schedule: ReannounceSchedule::default(),
        },
    )
    .expect("static self-announce config is valid");
    // The engine now owns the keys; wipe our copy promptly.
    drop(identity_secret_key);

    if let Some(destination) = state.self_announced_destination() {
        let hex: String = destination
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        println!("RNSD_SELF_ANNOUNCE_DEST {hex} name=personal.node");
    }

    let wait_mode = wait_mode_from_env();
    println!("RNSD_WAIT_MODE {wait_mode:?}");

    let mut announced_routes = 0;

    loop {
        let mut iface = match SerialUsbInterface::open(USB_INTERFACE_ID, &path) {
            Ok(iface) => iface,
            Err(e) => {
                eprintln!("RNSD_USB_OPEN_ERR {path}: {e:?}");
                std::thread::sleep(USB_RECONNECT_INTERVAL);
                continue;
            }
        };
        println!("RNSD_USB_OPEN_OK {path}");

        state
            .register_routable_interface(&iface)
            .expect("opened USB interface is connected and transmits");

        loop {
            let now = InstantMillis(clock.elapsed().as_millis() as u64);

            // Read at most one de-framed packet into per-step scratch. The
            // returned packet borrows `scratch` (not the interface), so the
            // interface is free again for the host to transmit egress on.
            let mut scratch = [0u8; MTU];
            let inbound = match iface.read_inbound(&mut scratch, now) {
                Ok(packet) => packet,
                Err(e) => {
                    eprintln!("RNSD_USB_READ_ERR {e:?}");
                    None
                }
            };

            // `Option::as_slice` lends the 0-or-1 packet as the borrowed batch the
            // engine seam expects — no allocation, borrows `inbound`/`scratch`.
            let mut driver =
                UsbHostExampleEngineDriver::for_runtime_step(clock, &mut iface, inbound.as_slice());
            driver
                .step(&mut state)
                .expect("usb driver clock/entropy step cannot fail");

            // A growing route count means the engine just learned a path from an
            // ingested announce — the proof the cable carried a real one.
            if state.route_count() > announced_routes {
                announced_routes = state.route_count();
                println!(
                    "RNSD_USB_RX_ANNOUNCE routes={} ingested={}",
                    state.route_count(),
                    state.ingested_packet_count()
                );
            }

            match usb_lifecycle_action(iface.state()) {
                UsbLifecycleAction::Continue => {}
                UsbLifecycleAction::Reopen { marker } => {
                    eprintln!("{marker}");
                    break;
                }
            }

            // Deadline-driven wait: size the next read's blocking window to the
            // engine's next scheduled work (or a packet, whichever lands first).
            // No fixed sleep — the engine has no reason to be woken otherwise.
            let now = InstantMillis(clock.elapsed().as_millis() as u64);
            let timeout = wait_mode.read_timeout(state.next_wakeup(now), now);
            iface.set_read_timeout(timeout);
        }

        std::thread::sleep(USB_RECONNECT_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_lifecycle_action_keeps_routable_states_running() {
        assert_eq!(
            usb_lifecycle_action(ConnectionState::Connected),
            UsbLifecycleAction::Continue
        );
        assert_eq!(
            usb_lifecycle_action(ConnectionState::Degraded),
            UsbLifecycleAction::Continue
        );
    }

    #[test]
    fn usb_lifecycle_action_reopens_on_non_routable_states() {
        assert_eq!(
            usb_lifecycle_action(ConnectionState::Initializing),
            UsbLifecycleAction::Reopen {
                marker: "RNSD_USB_NOT_ROUTABLE"
            }
        );
        assert_eq!(
            usb_lifecycle_action(ConnectionState::Reconnecting),
            UsbLifecycleAction::Reopen {
                marker: "RNSD_USB_NOT_ROUTABLE"
            }
        );
        assert_eq!(
            usb_lifecycle_action(ConnectionState::Failed),
            UsbLifecycleAction::Reopen {
                marker: "RNSD_USB_FAILED"
            }
        );
        assert_eq!(
            usb_lifecycle_action(ConnectionState::Disconnected),
            UsbLifecycleAction::Reopen {
                marker: "RNSD_USB_DISCONNECTED"
            }
        );
    }

    #[test]
    fn fixed_interval_wait_ignores_the_schedule() {
        let mode = WaitMode::FixedInterval(Duration::from_millis(5));
        let now = InstantMillis(1_000);
        assert_eq!(
            mode.read_timeout(NextScheduledWakeup::Idle, now),
            Duration::from_millis(5)
        );
        assert_eq!(
            mode.read_timeout(NextScheduledWakeup::Immediate, now),
            Duration::from_millis(5)
        );
        assert_eq!(
            mode.read_timeout(NextScheduledWakeup::At(InstantMillis(9_999)), now),
            Duration::from_millis(5),
        );
    }

    #[test]
    fn wake_on_deadline_sizes_the_wait_to_the_next_obligation() {
        let mode = WaitMode::WakeOnDeadline;
        let now = InstantMillis(1_000);

        // Work due now → don't block.
        assert_eq!(
            mode.read_timeout(NextScheduledWakeup::Immediate, now),
            Duration::ZERO
        );
        // A near deadline → exactly the gap until it.
        assert_eq!(
            mode.read_timeout(NextScheduledWakeup::At(InstantMillis(1_200)), now),
            Duration::from_millis(200),
        );
        // A deadline already in the past → zero (saturating), never a panic.
        assert_eq!(
            mode.read_timeout(NextScheduledWakeup::At(InstantMillis(500)), now),
            Duration::ZERO,
        );
        // A far deadline and a fully idle engine both cap at the lifecycle bound.
        assert_eq!(
            mode.read_timeout(NextScheduledWakeup::At(InstantMillis(u64::MAX)), now),
            MAX_BLOCKING_WAIT,
        );
        assert_eq!(
            mode.read_timeout(NextScheduledWakeup::Idle, now),
            MAX_BLOCKING_WAIT
        );
    }
}
