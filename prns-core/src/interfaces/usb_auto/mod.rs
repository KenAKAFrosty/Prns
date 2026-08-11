mod policy;
mod protocol;

pub use policy::{
    device_descriptor, host_descriptor, DEVICE_DEFAULTS, DEVICE_USB_BITRATE_BPS, DEVICE_USB_HW_MTU,
    HOST_DEFAULTS, HOST_USB_BITRATE_BPS, HOST_USB_HW_MTU,
};
pub use protocol::{
    decode_message, host_react, node_tag_for, Capabilities, Decoder, HostInbound, MalformedMessage,
    Message, NodeTag, PeerProfile, VitalsReport, WriteError, ANDROID_ACCESSORY_DESCRIPTION,
    ANDROID_ACCESSORY_MANUFACTURER, ANDROID_ACCESSORY_MODEL, ANDROID_ACCESSORY_SERIAL,
    ANDROID_ACCESSORY_URI, ANDROID_ACCESSORY_VERSION, MAGIC, MAX_DATA_BYTES, MAX_FRAMED_BYTES,
    MAX_MESSAGE_BYTES, NODE_TAG_LEN, PROTOCOL_VERSION, READ_CHUNK_BYTES, WEBUSB_PRODUCT_ID,
    WEBUSB_VENDOR_ID,
};
#[cfg(any(test, feature = "embassy-host"))]
pub use protocol::{react_to, InboundReaction};
