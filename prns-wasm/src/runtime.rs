use core::convert::TryFrom;

use js_sys::{Array, Object};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Directive, EngineCommand,
    EngineReaction, EngineState, FanTarget, InstantMillis, IssuedCommand, RatchetPolicy,
};
use personal_rns::interfaces::bluetooth_auto::core as bluetooth_core;
use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, Capabilities, InboundPacket, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceMode,
};
use personal_rns::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use personal_rns::storage::GrowableHeap;
use wasm_bindgen::prelude::*;

use crate::input::{
    array_to_strings, destination_hash_from_vec, interface_id_from_vec, optional_bytes,
    optional_u32, parse_interface_kind, require_entropy, required_array, required_bytes,
    required_string, required_u64, secret_key_from_vec,
};
use crate::js_translation::{
    interface_kind_name, journaled_to_js, outbound_to_js, set_bytes, set_str, set_u32, set_u64,
    set_usize, set_value,
};
use crate::parameters::bitrate_bps_u32;

#[derive(Clone)]
pub(crate) struct OutboundFrame {
    pub(crate) target: OutboundTarget,
    pub(crate) bytes: Vec<u8>,
    pub(crate) announce: bool,
    pub(crate) hops: Option<u8>,
}

#[derive(Clone)]
pub(crate) enum OutboundTarget {
    Interface(InterfaceId),
    Broadcast {
        supervisor: InterfaceKind,
        fan: FanTarget,
    },
}

#[wasm_bindgen]
pub struct PrnsRuntime {
    engine: EngineState<GrowableHeap>,
    interfaces: Vec<InterfaceDescriptor>,
    events: Vec<JsValue>,
    outbound: Vec<OutboundFrame>,
    next_command_id: u64,
    ble_identity: bluetooth_core::BleIdentity,
}

#[wasm_bindgen]
impl PrnsRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new(identity_secret_key: Vec<u8>) -> Result<PrnsRuntime, JsValue> {
        let secret = secret_key_from_vec(identity_secret_key)?;
        let mut ble_identity_bytes = [0u8; 16];
        getrandom::getrandom(&mut ble_identity_bytes).map_err(|error| {
            JsValue::from_str(&format!("no CSPRNG for the BLE wire identity: {error}"))
        })?;
        Ok(Self {
            engine: EngineState::new(secret),
            interfaces: Vec::new(),
            events: Vec::new(),
            outbound: Vec::new(),
            next_command_id: 0,
            ble_identity: bluetooth_core::BleIdentity::new(ble_identity_bytes),
        })
    }

    #[wasm_bindgen(js_name = registerInterface)]
    pub fn register_interface(&mut self, options: JsValue) -> Result<Vec<u8>, JsValue> {
        let kind = parse_interface_kind(&required_string(&options, "kind")?)?;
        let channel_tag = required_bytes(&options, "channelTag")?;
        let bitrate = optional_u32(&options, "bitrateBps")?
            .map(u64::from)
            .and_then(BitrateBps::new)
            .ok_or_else(|| {
                JsValue::from_str("bitrateBps is required and must be at least 5 bps")
            })?;
        let hardware_mtu = optional_u32(&options, "hardwareMtu")?;
        let id = InterfaceId::from_channel_tag(kind, &channel_tag);
        let capabilities = InterfaceCapabilities::try_from(Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: true,
        })
        .map_err(|_| JsValue::from_str("invalid default interface capabilities"))?;
        let descriptor = InterfaceDescriptor {
            id,
            capabilities,
            mode: InterfaceMode::Full,
            bitrate,
            hardware_mtu: hardware_mtu.map(|mtu| mtu as usize),
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
            airtime_duty_cycle: None,
            common: InterfaceCommonPolicy::RNS_DEFAULT,
        };
        if let Some(slot) = self.interfaces.iter_mut().find(|iface| iface.id == id) {
            *slot = descriptor;
        } else {
            self.interfaces.push(descriptor);
        }
        Ok(id.as_bytes().to_vec())
    }

    #[wasm_bindgen(js_name = bluetoothIdentity)]
    pub fn bluetooth_identity(&self) -> Vec<u8> {
        self.ble_identity.as_bytes().to_vec()
    }

    #[wasm_bindgen(js_name = registerSingleDestination)]
    pub fn register_single_destination(&mut self, options: JsValue) -> Result<Vec<u8>, JsValue> {
        let app_name = required_string(&options, "appName")?;
        let aspects = required_array(&options, "aspects")?;
        let app_data = optional_bytes(&options, "appData")?.unwrap_or_default();
        let aspect_strings = array_to_strings(&aspects)?;
        let aspect_refs = aspect_strings
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let Some(identity) = self.engine.held_identity_hashes().first().copied() else {
            return Err(JsValue::from_str("runtime has no held identity"));
        };
        let destination = self
            .engine
            .register_single_destination(
                &identity,
                &app_name,
                &aspect_refs,
                &app_data,
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::Ratcheted,
            )
            .map_err(|error| {
                JsValue::from_str(&format!("destination registration failed: {error:?}"))
            })?;
        Ok(destination.as_bytes().to_vec())
    }

    #[wasm_bindgen(js_name = announce)]
    pub fn announce(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let destination = required_bytes(&options, "destination")?;
        let now_ms = required_u64(&options, "nowMs")?;
        let entropy = required_bytes(&options, "entropy")?;
        require_entropy(&entropy)?;
        let destination = destination_hash_from_vec(destination)?;
        let id = self.mint_command_id();
        let command = EngineCommand::AnnounceNow(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        });
        self.ingest_command(id, command, now_ms, entropy);
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = ingest)]
    pub fn ingest(&mut self, options: JsValue) -> Result<(), JsValue> {
        let interface_id = required_bytes(&options, "interfaceId")?;
        let bytes = required_bytes(&options, "bytes")?;
        let now_ms = required_u64(&options, "nowMs")?;
        let entropy = required_bytes(&options, "entropy")?;
        require_entropy(&entropy)?;
        let source_interface = interface_id_from_vec(interface_id)?;
        let mut bytes = bytes;
        let mut entropy = EntropyCursor::new(entropy);
        let packet = InboundPacket {
            arrived_at: InstantMillis(now_ms),
            source_interface,
            bytes: &mut bytes,
        };
        let mut should_prove = |_request: &personal_rns::engine::ProofRequest| true;
        let mut should_accept_resource =
            |_offer: &personal_rns::routing::links::resources::ResourceOffer| false;
        let interfaces_snapshot = self.interfaces.clone();
        let mut reactions = Vec::new();
        self.engine.ingest_packet_into(
            packet,
            personal_rns::engine::IngestIo {
                interfaces: personal_rns::interfaces::AttachedInterfaces::new(&interfaces_snapshot),
                now: InstantMillis(now_ms),
                fill_entropy: &mut |out| entropy.fill(out),
                should_prove: &mut should_prove,
                should_accept_resource: &mut should_accept_resource,
                sink: &mut |reaction| reactions.push(capture_reaction(reaction)),
            },
        );
        self.apply_captured(reactions);
        Ok(())
    }

    #[wasm_bindgen(js_name = drainEvents)]
    pub fn drain_events(&mut self) -> Array {
        let drained = Array::new();
        for event in self.events.drain(..) {
            drained.push(&event);
        }
        drained
    }

    #[wasm_bindgen(js_name = drainOutbound)]
    pub fn drain_outbound(&mut self) -> Array {
        let drained = Array::new();
        for frame in self.outbound.drain(..) {
            drained.push(&outbound_to_js(&frame));
        }
        drained
    }

    #[wasm_bindgen(js_name = snapshot)]
    pub fn snapshot(&self) -> JsValue {
        let object = Object::new();
        set_str(&object, "type", "snapshot");
        set_u64(
            &object,
            "ingestedPackets",
            self.engine.ingested_packet_count(),
        );
        set_u64(
            &object,
            "ingestedCommands",
            self.engine.ingested_command_count(),
        );
        set_usize(&object, "routes", self.engine.route_count());
        set_usize(
            &object,
            "scheduledAnnounces",
            self.engine.scheduled_announce_count(),
        );
        let interfaces = Array::new();
        for interface in &self.interfaces {
            let row = Object::new();
            set_bytes(&row, "id", interface.id.as_bytes());
            set_str(&row, "kind", interface_kind_name(interface.id.kind()));
            set_u32(&row, "bitrateBps", bitrate_bps_u32(interface.bitrate));
            if let Some(mtu) = interface.hardware_mtu {
                set_usize(&row, "hardwareMtu", mtu);
            }
            set_usize(&row, "routes", self.engine.route_count_via(interface.id));
            set_usize(&row, "links", self.engine.link_count_via(interface.id));
            interfaces.push(&row);
        }
        set_value(&object, "interfaces", interfaces.into());
        object.into()
    }
}

impl PrnsRuntime {
    fn mint_command_id(&mut self) -> CommandId {
        let id = CommandId(self.next_command_id);
        self.next_command_id = self.next_command_id.saturating_add(1);
        id
    }

    fn ingest_command(
        &mut self,
        id: CommandId,
        command: EngineCommand,
        now_ms: u64,
        entropy: Vec<u8>,
    ) {
        let mut entropy = EntropyCursor::new(entropy);
        let interfaces_snapshot = self.interfaces.clone();
        let mut reactions = Vec::new();
        self.engine.ingest_command_into(
            IssuedCommand { id, command },
            personal_rns::interfaces::AttachedInterfaces::new(&interfaces_snapshot),
            InstantMillis(now_ms),
            &mut |out| entropy.fill(out),
            &mut |reaction| reactions.push(capture_reaction(reaction)),
        );
        self.apply_captured(reactions);
    }

    fn apply_captured(&mut self, reactions: Vec<CapturedReaction>) {
        for reaction in reactions {
            match reaction {
                CapturedReaction::Event(event) => self.events.push(event),
                CapturedReaction::Outbound(frame) => self.outbound.push(frame),
            }
        }
    }
}

enum CapturedReaction {
    Event(JsValue),
    Outbound(OutboundFrame),
}

struct EntropyCursor {
    bytes: Vec<u8>,
    offset: usize,
}

impl EntropyCursor {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes, offset: 0 }
    }

    fn fill(&mut self, out: &mut [u8]) {
        let available = self.bytes.len().saturating_sub(self.offset);
        let copied = available.min(out.len());
        if copied > 0 {
            out[..copied].copy_from_slice(&self.bytes[self.offset..self.offset + copied]);
            self.offset += copied;
        }
        if copied < out.len() {
            out[copied..].fill(0);
        }
    }
}

fn capture_reaction(reaction: EngineReaction<'_>) -> CapturedReaction {
    match reaction {
        EngineReaction::Journaled(journaled) => CapturedReaction::Event(journaled_to_js(journaled)),
        EngineReaction::Directive(directive) => {
            CapturedReaction::Outbound(directive_to_frame(directive))
        }
    }
}

fn directive_to_frame(directive: Directive<'_>) -> OutboundFrame {
    match directive {
        Directive::Send { target, bytes } => OutboundFrame {
            target: OutboundTarget::Interface(target),
            bytes: bytes.to_vec(),
            announce: false,
            hops: None,
        },
        Directive::SendAnnounce {
            target,
            bytes,
            hops,
        } => OutboundFrame {
            target: OutboundTarget::Interface(target),
            bytes: bytes.to_vec(),
            announce: true,
            hops: Some(hops),
        },
        Directive::SendToFleet {
            supervisor,
            fan,
            bytes,
        } => OutboundFrame {
            target: OutboundTarget::Broadcast { supervisor, fan },
            bytes: bytes.to_vec(),
            announce: false,
            hops: None,
        },
        Directive::SendAnnounceToFleet {
            supervisor,
            fan,
            bytes,
            hops,
        } => OutboundFrame {
            target: OutboundTarget::Broadcast { supervisor, fan },
            bytes: bytes.to_vec(),
            announce: true,
            hops: Some(hops),
        },
        Directive::EmitFrame {
            target,
            size_hint,
            fill,
        } => {
            let mut bytes = vec![0u8; size_hint];
            let len = fill(&mut bytes).unwrap_or(0);
            bytes.truncate(len);
            OutboundFrame {
                target: OutboundTarget::Interface(target),
                bytes,
                announce: false,
                hops: None,
            }
        }
    }
}
