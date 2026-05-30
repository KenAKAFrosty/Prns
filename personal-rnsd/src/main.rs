//! Personal Reticulum daemon: drive the engine over a real USB-serial
//! interface — the host end of a cable to an ESP32-C6 (or any stock RNS
//! serial peer). Each step reads at most one de-framed packet off the wire,
//! lets the engine ingest it, and pumps any due egress back out.

use std::time::{Duration, Instant};

use personal_rns::engine::{step_engine, DefaultEngineState, InstantMillis};
use personal_rns::interfaces::{ConnectionState, Interface, InterfaceId};
use personal_rns::wire::MTU;
use personal_rnsd::{SerialUsbInterface, UsbHost};

/// Stable id for the daemon's USB-serial interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);

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
    let mut state: DefaultEngineState = DefaultEngineState::default();
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
            let mut host = UsbHost::for_runtime_step(clock, &mut iface, inbound.as_slice());
            step_engine(&mut state, &mut host).expect("usb host clock/entropy step cannot fail");

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
