use core::convert::TryFrom;

use js_sys::Array;
use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;
use personal_rns::interfaces::usb_auto;
use wasm_bindgen::prelude::*;

use crate::input::interface_id_from_vec;
use crate::js_translation::usb_auto_message_to_js;
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

#[wasm_bindgen(js_name = usbAutoHostHelloFrame)]
pub fn usb_auto_host_hello_frame() -> Result<Vec<u8>, JsValue> {
    write_usb_auto_frame(usb_auto::Message::Hello(usb_auto::Capabilities::host()))
}

#[wasm_bindgen(js_name = usbAutoHostHelloAckFrame)]
pub fn usb_auto_host_hello_ack_frame(node_tag: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let tag = node_tag_from_vec(node_tag)?;
    write_usb_auto_frame(usb_auto::Message::HelloAck {
        tag,
        capabilities: usb_auto::Capabilities::host(),
    })
}

#[wasm_bindgen(js_name = usbAutoDataFrame)]
pub fn usb_auto_data_frame(packet: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    write_usb_auto_frame(usb_auto::Message::Data(&packet))
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
