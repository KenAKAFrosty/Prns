//! Personal Reticulum daemon: drive the engine over a real USB-serial
//! interface — the host end of a cable to an ESP32-C6 (or any stock RNS
//! serial peer). Each step reads at most one de-framed packet off the wire,
//! lets the engine ingest it, and pumps any due egress back out.

use std::time::{Duration, Instant};

use personal_rns::engine::{
    DefaultEngineState, EngineDriver, InstantMillis, ReannounceSchedule, SelfAnnounceConfig,
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

/// Idle poll cadence between steps. The interface read itself blocks up to its
/// own short timeout, so this only adds a small floor when the link is busy.
const RUNTIME_POLL_INTERVAL: Duration = Duration::from_millis(5);
const USB_RECONNECT_INTERVAL: Duration = Duration::from_millis(500);

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

            std::thread::sleep(RUNTIME_POLL_INTERVAL);
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
}
