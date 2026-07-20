use core::convert::TryFrom;

use js_sys::{Array, Uint8Array};
use personal_rns::interfaces::bluetooth_auto::core as bluetooth_core;
use wasm_bindgen::prelude::*;

use crate::js_translation::bluetooth_control_to_js;
use crate::parameters::bitrate_bps_u32;

const WEB_BLUETOOTH_GATT_COMPATIBILITY_ENDPOINT: bluetooth_core::Endpoint =
    bluetooth_core::Endpoint::Android(bluetooth_core::AndroidHost::Android);
const WEB_BLUETOOTH_GATT_FRAGMENT_PAYLOAD: usize = 120;
const WEB_BLUETOOTH_REASSEMBLY_CAP: usize = 600;

#[wasm_bindgen(js_name = bluetoothServiceUuid)]
pub fn bluetooth_service_uuid() -> String {
    uuid_string(bluetooth_core::BLE_SERVICE_UUID_BYTES)
}

#[wasm_bindgen(js_name = bluetoothControlUuid)]
pub fn bluetooth_control_uuid() -> String {
    uuid_string(uuid_bytes(bluetooth_core::NATIVE_CONTROL_UUID))
}

#[wasm_bindgen(js_name = bluetoothDataUuid)]
pub fn bluetooth_data_uuid() -> String {
    uuid_string(uuid_bytes(bluetooth_core::NATIVE_DATA_UUID))
}

#[wasm_bindgen(js_name = bluetoothBitrateBps)]
pub fn bluetooth_bitrate_bps() -> u32 {
    bitrate_bps_u32(bluetooth_core::BLE_BITRATE_GUESS_BPS)
}

#[wasm_bindgen(js_name = bluetoothHardwareMtu)]
pub fn bluetooth_hardware_mtu() -> usize {
    bluetooth_core::BLE_HW_MTU
}

#[wasm_bindgen(js_name = bluetoothDialerHello)]
pub fn bluetooth_dialer_hello(identity: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let local = web_bluetooth_local(identity)?;
    write_bluetooth_control(bluetooth_core::Control::Hello {
        identity: local.identity,
        endpoint: local.endpoint,
        capabilities: local.capabilities,
        peer_rssi: None,
    })
}

#[wasm_bindgen(js_name = bluetoothDecodeControl)]
pub fn bluetooth_decode_control(bytes: Vec<u8>) -> Result<JsValue, JsValue> {
    let control = bluetooth_core::Control::decode(&bytes)
        .ok_or_else(|| JsValue::from_str("malformed Bluetooth control frame"))?;
    Ok(bluetooth_control_to_js(control))
}

#[wasm_bindgen(js_name = bluetoothDataFragments)]
pub fn bluetooth_data_fragments(packet: Vec<u8>) -> Array {
    let fragments = Array::new();
    let mut out = [0u8; bluetooth_core::FRAGMENT_HEADER_LEN + WEB_BLUETOOTH_GATT_FRAGMENT_PAYLOAD];
    for fragment in bluetooth_core::fragments_of(&packet, WEB_BLUETOOTH_GATT_FRAGMENT_PAYLOAD) {
        if let Some(len) = fragment.encode(&mut out) {
            fragments.push(&Uint8Array::from(&out[..len]));
        }
    }
    fragments
}

#[wasm_bindgen]
pub struct BluetoothReassembler {
    inner: bluetooth_core::Reassembler<WEB_BLUETOOTH_REASSEMBLY_CAP>,
}

#[wasm_bindgen]
impl BluetoothReassembler {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: bluetooth_core::Reassembler::new(),
        }
    }

    pub fn absorb(&mut self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        let fragment = bluetooth_core::Fragment::decode(&bytes)?;
        self.inner.absorb(&fragment).map(<[u8]>::to_vec)
    }
}

impl Default for BluetoothReassembler {
    fn default() -> Self {
        Self::new()
    }
}

fn write_bluetooth_control(control: bluetooth_core::Control) -> Result<Vec<u8>, JsValue> {
    let mut out = vec![0u8; bluetooth_core::CONTROL_MAX_LEN];
    let len = control
        .encode(&mut out)
        .ok_or_else(|| JsValue::from_str("Bluetooth control encode failed"))?;
    out.truncate(len);
    Ok(out)
}

fn web_bluetooth_local(identity: Vec<u8>) -> Result<bluetooth_core::LocalPeer, JsValue> {
    let identity = bluetooth_identity_from_vec(identity)?;
    Ok(bluetooth_core::LocalPeer {
        identity,
        endpoint: WEB_BLUETOOTH_GATT_COMPATIBILITY_ENDPOINT,
        capabilities: bluetooth_core::LinkCapabilities {
            l2cap: None,
            link_mtu: bluetooth_core::BLE_HW_MTU as u16,
        },
    })
}

fn bluetooth_identity_from_vec(bytes: Vec<u8>) -> Result<bluetooth_core::BleIdentity, JsValue> {
    let Ok(identity) = <[u8; 16]>::try_from(bytes) else {
        return Err(JsValue::from_str("Bluetooth identity must be 16 bytes"));
    };
    Ok(bluetooth_core::BleIdentity::new(identity))
}

fn uuid_bytes(uuid: bluetooth_core::BleUuid) -> [u8; 16] {
    match uuid {
        bluetooth_core::BleUuid::Bit128(bytes) => bytes,
        bluetooth_core::BleUuid::Bit16(short) => {
            let mut bytes = [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b,
                0x34, 0xfb,
            ];
            bytes[2..4].copy_from_slice(&short.to_be_bytes());
            bytes
        }
    }
}

fn uuid_string(bytes: [u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}
