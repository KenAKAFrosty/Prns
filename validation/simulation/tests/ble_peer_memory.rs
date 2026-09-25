use std::future::pending;
use std::mem::size_of_val;

use personal_rns::interfaces::bluetooth_auto::{
    BleIdentity, BleSink, BleSource, BLE_HW_MTU, BLE_WIRE_FRAME_LEN,
};
use personal_rns::interfaces::FrameSink;
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_interfaces_tokio::bluetooth_auto::BluetoothPeer;

struct IdleSource;
struct IdleSink;
struct IdleSeam(Vec<u8>);

impl BleSource for IdleSource {
    type Error = std::convert::Infallible;

    async fn recv_frame(&mut self, _out: &mut [u8]) -> Result<usize, Self::Error> {
        pending().await
    }
}

impl BleSink for IdleSink {
    type Error = std::convert::Infallible;

    async fn send_frame(&mut self, _frame: &[u8]) -> Result<(), Self::Error> {
        pending().await
    }
}

impl InterfaceSeam for IdleSeam {
    fn fill_random(&mut self, bytes: &mut [u8]) {
        bytes.fill(0);
    }

    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        &mut self.0
    }

    async fn commit_inbound(&mut self) {
        self.0.clear();
    }

    async fn next_outbound(&mut self) -> &[u8] {
        pending().await
    }
}

#[test]
fn peer_future_stays_transport_sized() {
    const FIXTURE_STATE_BUDGET: usize = 8 * 1024;
    let peer = BluetoothPeer::new(BleIdentity::new([1; 16]), IdleSource, IdleSink);
    let future = peer.run(IdleSeam(Vec::new()));
    assert!(size_of_val(&future) <= BLE_WIRE_FRAME_LEN + FIXTURE_STATE_BUDGET);
}

#[test]
#[ignore = "manual before/after future-layout measurement, not RSS or a node-capacity benchmark"]
fn measure_bluetooth_peer_future() {
    let peer = BluetoothPeer::new(BleIdentity::new([1; 16]), IdleSource, IdleSink);
    let future = peer.run(IdleSeam(Vec::new()));
    println!(
        "ble_hw_mtu={BLE_HW_MTU} peer_future_bytes={}",
        size_of_val(&future)
    );
}
