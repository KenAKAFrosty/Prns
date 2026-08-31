use js_sys::{BigInt, Object, Reflect, Uint8Array};
use personal_rns::engine::FanTarget;
use personal_rns::interfaces::bluetooth_auto as bluetooth_contract;
use personal_rns::interfaces::usb_auto;
use personal_rns::interfaces::InterfaceKind;
use prns_host::{CommandOutcome, DeliveryEvidence};
use wasm_bindgen::prelude::*;

use crate::command_settlement::{CapturedCommandResult, CapturedCommandSettlement};
use crate::runtime::{OutboundFrame, OutboundTarget};

pub(crate) fn command_settled_to_js(settlement: CapturedCommandSettlement) -> JsValue {
    let object = Object::new();
    set_str(&object, "type", "commandSettled");
    set_u64(&object, "id", settlement.id().0);
    match settlement.into_result() {
        CapturedCommandResult::Untracked => set_str(&object, "result", "untracked"),
        CapturedCommandResult::Tracked(Ok(outcome)) => {
            set_str(&object, "result", "succeeded");
            outcome_to_js(&object, outcome);
        }
        CapturedCommandResult::Tracked(Err(failure)) => {
            set_str(&object, "result", "failed");
            set_str(&object, "kind", failure.kind().contract_name());
            if let Some(detail) = failure.detail() {
                set_str(&object, "detail", detail);
            }
        }
    }
    object.into()
}

fn outcome_to_js(object: &Object, outcome: CommandOutcome) {
    set_str(object, "kind", outcome.kind().contract_name());
    match outcome {
        CommandOutcome::Announced
        | CommandOutcome::LinkCloseQueued
        | CommandOutcome::Identified
        | CommandOutcome::ResourceSent
        | CommandOutcome::ResourceStrategySet
        | CommandOutcome::RequesterAllowed => {}
        CommandOutcome::PacketDelivered {
            rtt_millis,
            evidence,
        } => {
            set_u64(object, "rttMillis", rtt_millis);
            set_str(object, "evidence", evidence.kind().contract_name());
            match evidence {
                DeliveryEvidence::ExplicitProof(hash) | DeliveryEvidence::ImplicitProof(hash) => {
                    set_bytes(object, "packetHash", hash.as_bytes());
                }
                DeliveryEvidence::Response => {}
            }
        }
        CommandOutcome::InterfaceAttached { interface }
        | CommandOutcome::InterfaceDetached { interface } => {
            set_bytes(object, "interface", interface.as_bytes());
        }
        CommandOutcome::LinkEstablished {
            link_id,
            rtt_millis,
        } => {
            set_bytes(object, "linkId", link_id.as_bytes());
            set_u64(object, "rttMillis", rtt_millis);
        }
        CommandOutcome::PathDiscovered { hops } => {
            set_u32(object, "hops", u32::from(hops));
        }
        CommandOutcome::ResponseReceived { data, rtt_millis } => {
            set_bytes(object, "data", &data);
            set_u64(object, "rttMillis", rtt_millis);
        }
        CommandOutcome::ResponseSent { rtt_millis } => {
            set_u64(object, "rttMillis", rtt_millis);
        }
    }
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
            group_tag,
        } => {
            set_str(&object, "type", "hello");
            set_bytes(&object, "identity", identity.as_bytes());
            set_str(&object, "endpoint", &format!("{endpoint:?}"));
            set_bool(&object, "l2cap", capabilities.l2cap.is_some());
            set_u32(&object, "linkMtu", capabilities.link_mtu as u32);
            if let Some(rssi) = peer_rssi {
                set_i32(&object, "peerRssi", rssi as i32);
            }
            if let Some(tag) = group_tag {
                set_bytes(&object, "groupTag", &tag);
            }
        }
        bluetooth_contract::Control::Welcome {
            identity,
            endpoint,
            capabilities,
            peer_rssi,
            group_tag,
        } => {
            set_str(&object, "type", "welcome");
            set_bytes(&object, "identity", identity.as_bytes());
            set_str(&object, "endpoint", &format!("{endpoint:?}"));
            set_bool(&object, "l2cap", capabilities.l2cap.is_some());
            set_u32(&object, "linkMtu", capabilities.link_mtu as u32);
            if let Some(rssi) = peer_rssi {
                set_i32(&object, "peerRssi", rssi as i32);
            }
            if let Some(tag) = group_tag {
                set_bytes(&object, "groupTag", &tag);
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

pub(crate) fn set_bigint(object: &Object, key: &str, value: u64) {
    set_value(object, key, BigInt::from(value).into());
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
