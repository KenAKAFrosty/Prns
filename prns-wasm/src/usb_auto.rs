use core::convert::TryFrom;

use js_sys::Array;
use personal_rns::interfaces::lora::{RadioProfile, PROFILE_WIRE_LEN};
use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;
use personal_rns::interfaces::usb_auto;
use wasm_bindgen::prelude::*;

use crate::input::interface_id_from_vec;
use crate::js_translation::{snapshot_body_to_js, usb_auto_message_to_js};
use crate::parameters::bitrate_bps_u32;

#[wasm_bindgen(js_name = usbAutoHostBitrateBps)]
pub fn usb_auto_host_bitrate_bps() -> u32 {
    bitrate_bps_u32(personal_rns::interfaces::usb_auto::HOST_USB_BITRATE_BPS)
}

#[wasm_bindgen(js_name = usbAutoHostHardwareMtu)]
pub fn usb_auto_host_hardware_mtu() -> usize {
    personal_rns::interfaces::usb_auto::HOST_USB_HW_MTU
}

#[wasm_bindgen(js_name = usbAutoWebUsbVendorId)]
pub fn usb_auto_web_usb_vendor_id() -> u16 {
    personal_rns::interfaces::usb_auto::WEBUSB_VENDOR_ID
}

#[wasm_bindgen(js_name = usbAutoWebUsbProductId)]
pub fn usb_auto_web_usb_product_id() -> u16 {
    personal_rns::interfaces::usb_auto::WEBUSB_PRODUCT_ID
}

#[wasm_bindgen(js_name = usbAutoNodeTagFor)]
pub fn usb_auto_node_tag_for(interface_id: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let interface_id = interface_id_from_vec(interface_id)?;
    Ok(usb_auto::node_tag_for(interface_id).0.to_vec())
}

/// Host hello advertising config-lane support alongside the host lane. The
/// config webUI (`/configure`) and the Reticulum-data session both go through
/// this; the config-lane bit is a strict superset, so a data-only peer simply
/// never sends `ConfigRequest`s and the bit stays inert.
#[wasm_bindgen(js_name = usbAutoHostHelloFrame)]
pub fn usb_auto_host_hello_frame() -> Result<Vec<u8>, JsValue> {
    write_usb_auto_frame(usb_auto::Message::Hello(
        usb_auto::Capabilities::host().with_config_lane(),
    ))
}

#[wasm_bindgen(js_name = usbAutoHostHelloAckFrame)]
pub fn usb_auto_host_hello_ack_frame(node_tag: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let tag = node_tag_from_vec(node_tag)?;
    write_usb_auto_frame(usb_auto::Message::HelloAck {
        tag,
        capabilities: usb_auto::Capabilities::host().with_config_lane(),
    })
}

#[wasm_bindgen(js_name = usbAutoDataFrame)]
pub fn usb_auto_data_frame(packet: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    write_usb_auto_frame(usb_auto::Message::Data(&packet))
}

/// Build a framed `ConfigRequest` carrying `action` (bytes from one of the
/// `usbAutoConfigAction*` builders) under `request_id`.
#[wasm_bindgen(js_name = usbAutoConfigRequestFrame)]
pub fn usb_auto_config_request_frame(request_id: u8, action: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    write_usb_auto_frame(usb_auto::Message::ConfigRequest {
        request_id,
        action: &action,
    })
}

/// Decode a snapshot body (the `body` field of an inbound `snapshot` message)
/// into a plain JS object for the `/configure` webUI.
#[wasm_bindgen(js_name = usbAutoSnapshotDecode)]
pub fn usb_auto_snapshot_decode(body: Vec<u8>) -> Result<JsValue, JsValue> {
    snapshot_body_to_js(&body)
        .map_err(|error| JsValue::from_str(&format!("snapshot decode failed: {error:?}")))
}

#[wasm_bindgen(js_name = usbAutoConfigActionSetLoRaProfile)]
pub fn usb_auto_config_action_set_lora_profile(
    frequency_hz: u32,
    spreading_factor: u8,
    bandwidth: u8,
    coding_rate: u8,
    tx_power_dbm: i32,
    preamble: u16,
    region_code: u8,
) -> Result<Vec<u8>, JsValue> {
    let mut wire = [0u8; PROFILE_WIRE_LEN];
    wire[..4].copy_from_slice(&frequency_hz.to_le_bytes());
    wire[4] = spreading_factor;
    wire[5] = bandwidth;
    wire[6] = coding_rate;
    wire[7] = tx_power_dbm as u8;
    wire[8..10].copy_from_slice(&preamble.to_le_bytes());
    wire[10] = region_code;
    // The 12th byte is a reserved zero; decode enforces it.
    let profile = RadioProfile::decode(&wire)
        .ok_or_else(|| JsValue::from_str("invalid LoRa profile for config action"))?;
    Ok(encode_config_action(
        usb_auto::ConfigAction::SetLoRaProfile(profile),
    ))
}

#[wasm_bindgen(js_name = usbAutoConfigActionResetLoRaProfile)]
pub fn usb_auto_config_action_reset_lora_profile() -> Vec<u8> {
    encode_config_action(usb_auto::ConfigAction::ResetLoRaProfile)
}

#[wasm_bindgen(js_name = usbAutoConfigActionToggleInterface)]
pub fn usb_auto_config_action_toggle_interface(interface_code: u8) -> Result<Vec<u8>, JsValue> {
    let interface = usb_auto::ConfigInterface::from_wire_code(interface_code)
        .ok_or_else(|| JsValue::from_str("unknown config interface code"))?;
    Ok(encode_config_action(
        usb_auto::ConfigAction::ToggleInterface(interface),
    ))
}

#[wasm_bindgen(js_name = usbAutoConfigActionSleep)]
pub fn usb_auto_config_action_sleep() -> Vec<u8> {
    encode_config_action(usb_auto::ConfigAction::Sleep)
}

#[wasm_bindgen(js_name = usbAutoConfigActionWake)]
pub fn usb_auto_config_action_wake() -> Vec<u8> {
    encode_config_action(usb_auto::ConfigAction::Wake)
}

#[wasm_bindgen(js_name = usbAutoConfigActionAnnounce)]
pub fn usb_auto_config_action_announce() -> Vec<u8> {
    encode_config_action(usb_auto::ConfigAction::Announce)
}

#[wasm_bindgen(js_name = usbAutoConfigActionRequestSnapshot)]
pub fn usb_auto_config_action_request_snapshot() -> Vec<u8> {
    encode_config_action(usb_auto::ConfigAction::RequestSnapshot)
}

fn encode_config_action(action: usb_auto::ConfigAction) -> Vec<u8> {
    let mut out = vec![0u8; usb_auto::MAX_CONFIG_ACTION_BYTES];
    let len = action.encode(&mut out);
    out.truncate(len);
    out
}

#[wasm_bindgen]
pub struct UsbAutoDecoder {
    inner: RnsSerialDecoder<{ usb_auto::MAX_MESSAGE_BYTES }>,
}

#[wasm_bindgen]
impl UsbAutoDecoder {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: RnsSerialDecoder::new(),
        }
    }

    pub fn feed(&mut self, chunk: Vec<u8>) -> Array {
        let messages = Array::new();
        for byte in chunk {
            let Ok(Some(frame)) = self.inner.feed(byte) else {
                continue;
            };
            if frame.is_empty() {
                continue;
            }
            if let Ok(message) = usb_auto::decode_message(frame) {
                messages.push(&usb_auto_message_to_js(message));
            }
        }
        messages
    }
}

impl Default for UsbAutoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

fn write_usb_auto_frame(message: usb_auto::Message<'_>) -> Result<Vec<u8>, JsValue> {
    let mut out = vec![0u8; usb_auto::MAX_FRAMED_BYTES];
    let len = message
        .write_framed(&mut out)
        .map_err(|error| JsValue::from_str(&format!("USB-auto frame encode failed: {error:?}")))?;
    out.truncate(len);
    Ok(out)
}

fn node_tag_from_vec(bytes: Vec<u8>) -> Result<usb_auto::NodeTag, JsValue> {
    let Ok(tag) = <[u8; usb_auto::NODE_TAG_LEN]>::try_from(bytes) else {
        return Err(JsValue::from_str("USB-auto node tag must be 8 bytes"));
    };
    Ok(usb_auto::NodeTag(tag))
}
