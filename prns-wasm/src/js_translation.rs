use js_sys::{Object, Reflect, Uint8Array};
use personal_rns::engine::{FanTarget, Journaled, RouteRemovalCause};
use personal_rns::interfaces::bluetooth_auto as bluetooth_contract;
use personal_rns::interfaces::usb_auto;
use personal_rns::interfaces::InterfaceKind;
use personal_rns::routing::delivery::Delivery;
use wasm_bindgen::prelude::*;

use crate::runtime::{OutboundFrame, OutboundTarget};

pub(crate) fn journaled_to_js(journaled: Journaled<'_>) -> JsValue {
    let object = Object::new();
    match journaled {
        Journaled::AnnounceHeard { observation, .. } => {
            set_str(&object, "type", "announce");
            set_bytes(&object, "destination", observation.destination.as_bytes());
            set_u32(&object, "hops", u32::from(observation.hops.0));
            set_bytes(
                &object,
                "sourceInterface",
                observation.source_interface.as_bytes(),
            );
        }
        Journaled::SelfRatchetRotated { destination } => {
            set_str(&object, "type", "selfRatchetRotated");
            set_bytes(&object, "destination", destination.as_bytes());
        }
        Journaled::AnnounceHeldDropped {
            destination,
            source_interface,
            cause,
        } => {
            set_str(&object, "type", "announceHeldDropped");
            set_bytes(&object, "destination", destination.as_bytes());
            set_bytes(&object, "sourceInterface", source_interface.as_bytes());
            set_str(&object, "cause", &format!("{cause:?}"));
        }
        Journaled::CommandSettled { id, settlement } => {
            set_str(&object, "type", "commandSettled");
            set_u64(&object, "id", id.0);
            set_str(&object, "settlement", &format!("{settlement:?}"));
        }
        Journaled::LinkEstablished(link) => {
            set_str(&object, "type", "linkEstablished");
            set_str(&object, "detail", &format!("{link:?}"));
        }
        Journaled::PeerIdentified { link_id, identity } => {
            set_str(&object, "type", "peerIdentified");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_bytes(&object, "identity", identity.as_bytes());
        }
        Journaled::RequestReceived {
            link_id,
            path_hash,
            data,
            ..
        } => {
            set_str(&object, "type", "request");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "pathHash", &format!("{path_hash:?}"));
            set_bytes(&object, "data", data);
        }
        Journaled::ResponseReceived {
            command_id,
            link_id,
            data,
            ..
        } => {
            set_str(&object, "type", "response");
            set_u64(&object, "commandId", command_id.0);
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_bytes(&object, "data", data);
        }
        Journaled::ResponseSegmentReceived {
            command_id,
            link_id,
            segment_index,
            total_segments,
            data,
            ..
        } => {
            set_str(&object, "type", "responseSegment");
            set_u64(&object, "commandId", command_id.0);
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_u64(&object, "segmentIndex", segment_index);
            set_u64(&object, "totalSegments", total_segments);
            set_bytes(&object, "data", data);
        }
        Journaled::ChannelMessageReceived {
            link_id,
            message_type,
            data,
        } => {
            set_str(&object, "type", "channelMessage");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "messageType", &format!("{message_type:?}"));
            set_bytes(&object, "data", data);
        }
        Journaled::Delivered(Delivery::Single(delivery)) => {
            set_str(&object, "type", "singleDelivery");
            set_bytes(&object, "destination", delivery.destination.as_bytes());
            set_bytes(&object, "plaintext", delivery.plaintext);
            set_bytes(
                &object,
                "sourceInterface",
                delivery.source_interface.as_bytes(),
            );
        }
        Journaled::Delivered(Delivery::Plain(delivery)) => {
            set_str(&object, "type", "delivered");
            set_str(&object, "detail", &format!("{delivery:?}"));
        }
        Journaled::Delivered(Delivery::Group(delivery)) => {
            set_str(&object, "type", "delivered");
            set_str(&object, "detail", &format!("{delivery:?}"));
        }
        Journaled::Delivered(Delivery::Link(delivery)) => {
            set_str(&object, "type", "delivered");
            set_str(&object, "detail", &format!("{delivery:?}"));
        }
        Journaled::LinkClosed { link_id, reason } => {
            set_str(&object, "type", "linkClosed");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "reason", &format!("{reason:?}"));
        }
        Journaled::LinkInterfaceMismatch {
            link_id,
            attached_interface,
            arrived_on,
        } => {
            set_str(&object, "type", "linkInterfaceMismatch");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_bytes(&object, "attachedInterface", attached_interface.as_bytes());
            set_bytes(&object, "arrivedOn", arrived_on.as_bytes());
        }
        Journaled::ResourceReceived {
            link_id,
            hash,
            metadata,
            data,
        } => {
            set_str(&object, "type", "resourceReceived");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "hash", &format!("{hash:?}"));
            if let Some(metadata) = metadata {
                set_bytes(&object, "metadata", metadata);
            }
            set_bytes(&object, "data", data);
        }
        Journaled::ResourceFailed {
            link_id,
            hash,
            cause,
        } => {
            set_str(&object, "type", "resourceFailed");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "hash", &format!("{hash:?}"));
            set_str(&object, "cause", &format!("{cause:?}"));
        }
        Journaled::ResourceNeedsDecompression {
            link_id,
            hash,
            stream,
            uncompressed_data_bytes,
        } => {
            set_str(&object, "type", "resourceNeedsDecompression");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "hash", &format!("{hash:?}"));
            set_bytes(&object, "stream", stream);
            set_u64(&object, "uncompressedDataBytes", uncompressed_data_bytes);
        }
        Journaled::ResourceSegmentReceived {
            link_id,
            original_hash,
            segment_index,
            total_segments,
            metadata,
            data,
        } => {
            set_str(&object, "type", "resourceSegment");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "originalHash", &format!("{original_hash:?}"));
            set_u64(&object, "segmentIndex", segment_index);
            set_u64(&object, "totalSegments", total_segments);
            if let Some(metadata) = metadata {
                set_bytes(&object, "metadata", metadata);
            }
            set_bytes(&object, "data", data);
        }
        Journaled::ResourceAssembled {
            link_id,
            original_hash,
            total_size_bytes,
        } => {
            set_str(&object, "type", "resourceAssembled");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "originalHash", &format!("{original_hash:?}"));
            set_u64(&object, "totalSizeBytes", total_size_bytes);
        }
        Journaled::RouteRemoved { destination, cause } => {
            let kind = match cause {
                RouteRemovalCause::Expired => "routeExpired",
                RouteRemovalCause::Evicted => "routeEvicted",
                RouteRemovalCause::InterfaceGone => "routeInterfaceGone",
                RouteRemovalCause::Dropped => "routeDropped",
            };
            set_str(&object, "type", kind);
            set_bytes(&object, "destination", destination.as_bytes());
        }
    }
    object.into()
}

pub(crate) fn outbound_to_js(frame: &OutboundFrame) -> JsValue {
    let object = Object::new();
    set_str(
        &object,
        "type",
        if frame.announce { "announce" } else { "frame" },
    );
    set_value(
        &object,
        "target",
        outbound_target_to_js(frame.target.clone()),
    );
    if let Some(hops) = frame.hops {
        set_u32(&object, "hops", hops as u32);
    }
    set_bytes(&object, "bytes", &frame.bytes);
    object.into()
}

pub(crate) fn usb_auto_message_to_js(message: usb_auto::Message<'_>) -> JsValue {
    let object = Object::new();
    match message {
        usb_auto::Message::Hello(_) => set_str(&object, "type", "hello"),
        usb_auto::Message::HelloAck { tag, .. } => {
            set_str(&object, "type", "helloAck");
            set_bytes(&object, "tag", &tag.0);
        }
        usb_auto::Message::Data(packet) => {
            set_str(&object, "type", "data");
            set_bytes(&object, "bytes", packet);
        }
    }
    object.into()
}

pub(crate) fn bluetooth_control_to_js(control: bluetooth_contract::Control) -> JsValue {
    let object = Object::new();
    match control {
        bluetooth_contract::Control::Hello {
            identity,
            endpoint,
            capabilities,
            peer_rssi,
        } => {
            set_str(&object, "type", "hello");
            set_bytes(&object, "identity", identity.as_bytes());
            set_str(&object, "endpoint", &format!("{endpoint:?}"));
            set_bool(&object, "l2cap", capabilities.l2cap.is_some());
            set_u32(&object, "linkMtu", capabilities.link_mtu as u32);
            if let Some(rssi) = peer_rssi {
                set_i32(&object, "peerRssi", rssi as i32);
            }
        }
        bluetooth_contract::Control::Welcome {
            identity,
            endpoint,
            capabilities,
            peer_rssi,
        } => {
            set_str(&object, "type", "welcome");
            set_bytes(&object, "identity", identity.as_bytes());
            set_str(&object, "endpoint", &format!("{endpoint:?}"));
            set_bool(&object, "l2cap", capabilities.l2cap.is_some());
            set_u32(&object, "linkMtu", capabilities.link_mtu as u32);
            if let Some(rssi) = peer_rssi {
                set_i32(&object, "peerRssi", rssi as i32);
            }
        }
        bluetooth_contract::Control::Close { reason } => {
            set_str(&object, "type", "close");
            set_str(&object, "reason", &format!("{reason:?}"));
        }
    }
    object.into()
}

fn outbound_target_to_js(target: OutboundTarget) -> JsValue {
    let object = Object::new();
    match target {
        OutboundTarget::Interface(interface) => {
            set_str(&object, "type", "interface");
            set_bytes(&object, "interfaceId", interface.as_bytes());
        }
        OutboundTarget::Broadcast { supervisor, fan } => {
            set_str(&object, "type", "broadcast");
            set_str(
                &object,
                "supervisorKind",
                interface_kind_name(Some(supervisor)),
            );
            set_value(&object, "fan", fan_target_to_js(fan));
        }
    }
    object.into()
}

fn fan_target_to_js(fan: FanTarget) -> JsValue {
    let object = Object::new();
    match fan {
        FanTarget::All => set_str(&object, "type", "all"),
        FanTarget::Only(interface) => {
            set_str(&object, "type", "only");
            set_bytes(&object, "interfaceId", interface.as_bytes());
        }
        FanTarget::AllExcept(interface) => {
            set_str(&object, "type", "allExcept");
            set_bytes(&object, "interfaceId", interface.as_bytes());
        }
    }
    object.into()
}

pub(crate) fn interface_kind_name(kind: Option<InterfaceKind>) -> &'static str {
    match kind {
        Some(kind) => kind.name(),
        None => "unknown",
    }
}

pub(crate) fn set_str(object: &Object, key: &str, value: &str) {
    set_value(object, key, JsValue::from_str(value));
}

pub(crate) fn set_u32(object: &Object, key: &str, value: u32) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

pub(crate) fn set_i32(object: &Object, key: &str, value: i32) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

pub(crate) fn set_bool(object: &Object, key: &str, value: bool) {
    set_value(object, key, JsValue::from_bool(value));
}

pub(crate) fn set_u64(object: &Object, key: &str, value: u64) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

pub(crate) fn set_usize(object: &Object, key: &str, value: usize) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

pub(crate) fn set_bytes(object: &Object, key: &str, value: &[u8]) {
    set_value(object, key, Uint8Array::from(value).into());
}

pub(crate) fn set_value(object: &Object, key: &str, value: JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}
