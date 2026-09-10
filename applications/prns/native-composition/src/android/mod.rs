//! Android platform glue for the same application-owned native node.
//!
//! The foreground service reserves one bridge before starting Kotlin's BLE pumps.
//! Node startup consumes that reservation; platform callbacks never acquire the
//! node supervisor lock. The service must quiesce old Kotlin callbacks before
//! stopping Rust and must not replace an adapter whose shutdown is incomplete.

mod bluetooth_jni;
mod lifecycle_jni;

use std::sync::{Arc, Mutex, OnceLock};

use personal_rns::interfaces::bluetooth_auto::RadioMode;
#[cfg(any(test, target_os = "android"))]
use prns_ffi::bluetooth_auto::android::AndroidBleBackend;
use prns_ffi::bluetooth_auto::android::AndroidBleBridge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BridgeState {
    Idle,
    Prepared,
    Attached,
}

struct BluetoothOwner {
    bridge: AndroidBleBridge,
    state: Mutex<BridgeState>,
}

impl BluetoothOwner {
    fn new() -> Self {
        Self {
            bridge: AndroidBleBridge::new(),
            state: Mutex::new(BridgeState::Idle),
        }
    }

    fn prepare(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if *state != BridgeState::Idle {
            return false;
        }
        self.bridge.set_radio_mode(RadioMode::Off);
        *state = BridgeState::Prepared;
        true
    }

    #[cfg(any(test, target_os = "android"))]
    fn take_prepared(self: &Arc<Self>) -> Option<BluetoothSession> {
        let mut state = self.state.lock().ok()?;
        if *state != BridgeState::Prepared {
            return None;
        }
        *state = BridgeState::Attached;
        Some(BluetoothSession {
            owner: Arc::clone(self),
        })
    }

    fn release(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if *state == BridgeState::Attached {
            return false;
        }
        self.bridge.set_radio_mode(RadioMode::Off);
        *state = BridgeState::Idle;
        true
    }
}

fn owner() -> &'static Arc<BluetoothOwner> {
    static OWNER: OnceLock<Arc<BluetoothOwner>> = OnceLock::new();
    OWNER.get_or_init(|| Arc::new(BluetoothOwner::new()))
}

fn ble_bridge() -> AndroidBleBridge {
    owner().bridge.clone()
}

/// Reservation retained by the existing Rust worker, never a second node owner.
#[cfg(any(test, target_os = "android"))]
pub(crate) struct BluetoothSession {
    owner: Arc<BluetoothOwner>,
}

#[cfg(any(test, target_os = "android"))]
impl BluetoothSession {
    pub(crate) fn bridge(&self) -> AndroidBleBridge {
        self.owner.bridge.clone()
    }

    pub(crate) fn interface(
        &self,
        identity: personal_rns::interfaces::bluetooth_auto::BleIdentity,
    ) -> personal_rns::bluetooth_auto::BluetoothAuto<
        AndroidBleBackend,
        { AndroidBleBackend::MAX_PEERS },
    > {
        use personal_rns::interfaces::bluetooth_auto::{
            AndroidHost, Endpoint, LinkCapabilities, BLE_HW_MTU,
        };

        let bridge = self.bridge();
        bridge.set_local_identity(identity);
        personal_rns::bluetooth_auto::BluetoothAuto::new(
            AndroidBleBackend::new(bridge),
            identity,
            Endpoint::Android(AndroidHost::Android),
            LinkCapabilities {
                l2cap: None,
                link_mtu: BLE_HW_MTU as u16,
            },
        )
    }
}

#[cfg(any(test, target_os = "android"))]
impl Drop for BluetoothSession {
    fn drop(&mut self) {
        if let Ok(mut state) = self.owner.state.lock() {
            self.owner.bridge.set_radio_mode(RadioMode::Off);
            *state = BridgeState::Idle;
        }
    }
}

#[cfg(target_os = "android")]
pub(crate) fn take_prepared_bluetooth() -> Option<BluetoothSession> {
    owner().take_prepared()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn bridge_reservation_is_single_use_and_cannot_replace_a_live_generation() {
        let owner = Arc::new(BluetoothOwner::new());
        assert!(owner.take_prepared().is_none());
        assert!(owner.prepare());
        assert!(!owner.prepare());
        let session = owner.take_prepared().expect("prepared session");
        assert!(owner.take_prepared().is_none());
        assert!(!owner.prepare());
        assert!(!owner.release());
        session.bridge().set_radio_mode(RadioMode::On);
        drop(session);
        assert_eq!(owner.bridge.radio_state(), 0);
        assert!(owner.prepare());
        assert!(owner.release());
        assert!(owner.release());
    }

    #[test]
    fn releasing_unused_preparation_clears_pending_platform_state() {
        let owner = Arc::new(BluetoothOwner::new());
        assert!(owner.prepare());
        assert!(owner.bridge.push_dial([3; 6]));
        assert!(owner.release());
        assert!(!owner.bridge.next_dial(&mut [0; 6]));
        assert!(owner.prepare());
        let session = owner.take_prepared().expect("new session");
        assert_eq!(session.bridge().radio_state(), 0);
    }

    #[tokio::test]
    async fn missing_bluetooth_psm_does_not_block_tcp_or_node_shutdown() {
        use personal_rns::interfaces::bluetooth_auto::BleIdentity;
        use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
        use personal_rns::prelude::{GrowableHeap, ManuallyAttached, PrnsNode, PrnsNodeRecipe};
        use personal_rns::remote_control::RemoteControlService;
        use personal_rns::runtime::{NoPersistence, PreConfiguredDestination};
        use std::time::Duration;

        let owner = Arc::new(BluetoothOwner::new());
        assert!(owner.prepare());
        let session = owner.take_prepared().expect("reserved session");
        let bluetooth = session.interface(BleIdentity::new([0x51; 16]));
        let status = bluetooth.status();
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: personal_rns::request_endpoints![],
            remote_control: RemoteControlService::Unavailable,
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
            on_event: |_event, _state: &()| {},
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("TCP fixture");
        let target = listener
            .local_addr()
            .expect("TCP fixture address")
            .to_string();
        let handle = node.handle();
        handle.supervise(bluetooth);
        let _tcp = handle.attach(personal_rns::tcp::TcpClientInterface::new(target));
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let run = node.run_until(async {
            let _ = stopped.await;
        });
        tokio::pin!(run);
        let _connection = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::select! {
                accepted = listener.accept() => accepted,
                _ = &mut run => Err(std::io::Error::other("node stopped before TCP connection")),
            }
        })
        .await
        .expect("TCP progresses while BLE has no PSM")
        .expect("TCP connection");
        assert_eq!(status.connection(), ConnectionState::Initializing);
        shutdown.send(()).expect("request shutdown");
        tokio::time::timeout(Duration::from_secs(3), &mut run)
            .await
            .expect("node stops while BLE is waiting")
            .expect("node stopped cleanly");
        drop(session);
        assert_eq!(owner.bridge.radio_state(), 0);
        assert!(owner.prepare());
        assert!(owner.release());
    }
}
