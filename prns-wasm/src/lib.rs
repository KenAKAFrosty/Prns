#![forbid(unsafe_code)]

use core::convert::TryFrom;

use js_sys::{Array, Object, Reflect, Uint8Array};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Directive, EngineCommand,
    EngineReaction, EngineState, FanTarget, InstantMillis, IssuedCommand, Journaled, RatchetPolicy,
    RouteRemovalCause,
};
use personal_rns::identity::IDENTITY_SECRET_KEY_LEN;
use personal_rns::interfaces::bluetooth_auto::core as bluetooth_core;
use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;
use personal_rns::interfaces::usb_auto::core as usb_auto_core;
use personal_rns::interfaces::websocket::core as websocket_core;
use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, Capabilities, InboundPacket, InterfaceCapabilities,
    InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceMode, INTERFACE_ID_LEN,
};
use personal_rns::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use zeroize::Zeroizing;

const DEFAULT_ENTROPY_FLOOR: usize = 64;
const WEB_BLUETOOTH_GATT_FRAGMENT_PAYLOAD: usize = 120;
const WEB_BLUETOOTH_REASSEMBLY_CAP: usize = 600;

#[wasm_bindgen(js_name = identitySecretKeyLength)]
pub fn identity_secret_key_length() -> usize {
    IDENTITY_SECRET_KEY_LEN
}

#[wasm_bindgen(js_name = interfaceIdLength)]
pub fn interface_id_length() -> usize {
    INTERFACE_ID_LEN
}

#[wasm_bindgen(js_name = destinationHashLength)]
pub fn destination_hash_length() -> usize {
    TRUNCATED_HASH_BYTE_LEN
}

#[wasm_bindgen(js_name = usbAutoHostBitrateBps)]
pub fn usb_auto_host_bitrate_bps() -> u32 {
    personal_rns::interfaces::usb_auto::core::HOST_USB_BITRATE_BPS.get()
}

#[wasm_bindgen(js_name = usbAutoHostHardwareMtu)]
pub fn usb_auto_host_hardware_mtu() -> usize {
    personal_rns::interfaces::usb_auto::core::HOST_USB_HW_MTU
}

#[wasm_bindgen(js_name = usbAutoWebUsbVendorId)]
pub fn usb_auto_web_usb_vendor_id() -> u16 {
    personal_rns::interfaces::usb_auto::core::WEBUSB_VENDOR_ID
}

#[wasm_bindgen(js_name = usbAutoWebUsbProductId)]
pub fn usb_auto_web_usb_product_id() -> u16 {
    personal_rns::interfaces::usb_auto::core::WEBUSB_PRODUCT_ID
}

#[wasm_bindgen(js_name = usbAutoNodeTagFor)]
pub fn usb_auto_node_tag_for(interface_id: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let interface_id = interface_id_from_vec(interface_id)?;
    Ok(usb_auto_core::node_tag_for(interface_id).0.to_vec())
}

#[wasm_bindgen(js_name = usbAutoHostHelloFrame)]
pub fn usb_auto_host_hello_frame() -> Result<Vec<u8>, JsValue> {
    write_usb_auto_frame(usb_auto_core::Message::Hello(
        usb_auto_core::Capabilities::host(),
    ))
}

#[wasm_bindgen(js_name = usbAutoHostHelloAckFrame)]
pub fn usb_auto_host_hello_ack_frame(node_tag: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let tag = node_tag_from_vec(node_tag)?;
    write_usb_auto_frame(usb_auto_core::Message::HelloAck {
        tag,
        capabilities: usb_auto_core::Capabilities::host(),
    })
}

#[wasm_bindgen(js_name = usbAutoDataFrame)]
pub fn usb_auto_data_frame(packet: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    write_usb_auto_frame(usb_auto_core::Message::Data(&packet))
}

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
    bluetooth_core::BLE_BITRATE_GUESS_BPS.get()
}

#[wasm_bindgen(js_name = bluetoothHardwareMtu)]
pub fn bluetooth_hardware_mtu() -> usize {
    bluetooth_core::BLE_HW_MTU
}

#[wasm_bindgen(js_name = websocketBitrateBps)]
pub fn websocket_bitrate_bps() -> u32 {
    websocket_core::WEBSOCKET_BITRATE_ESTIMATE.get()
}

#[wasm_bindgen(js_name = websocketHardwareMtu)]
pub fn websocket_hardware_mtu() -> usize {
    websocket_core::WEBSOCKET_HW_MTU_CAP
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
pub struct UsbAutoDecoder {
    inner: RnsSerialDecoder<{ usb_auto_core::MAX_MESSAGE_BYTES }>,
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
            if let Ok(message) = usb_auto_core::decode_message(frame) {
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

#[derive(Clone)]
struct OutboundFrame {
    target: OutboundTarget,
    bytes: Vec<u8>,
    announce: bool,
    hops: Option<u8>,
}

#[derive(Clone)]
enum OutboundTarget {
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
            set_u32(&row, "bitrateBps", interface.bitrate.get());
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

fn journaled_to_js(journaled: Journaled<'_>) -> JsValue {
    let object = Object::new();
    match journaled {
        Journaled::AnnounceHeard {
            destination,
            hops,
            source_interface,
        } => {
            set_str(&object, "type", "announce");
            set_bytes(&object, "destination", destination.as_bytes());
            set_u32(&object, "hops", hops as u32);
            set_bytes(&object, "sourceInterface", source_interface.as_bytes());
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
        Journaled::Delivered(delivery) => {
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
            uncompressed_data_len,
        } => {
            set_str(&object, "type", "resourceNeedsDecompression");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "hash", &format!("{hash:?}"));
            set_bytes(&object, "stream", stream);
            set_u64(&object, "uncompressedDataLen", uncompressed_data_len);
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
            total_size,
        } => {
            set_str(&object, "type", "resourceAssembled");
            set_str(&object, "linkId", &format!("{link_id:?}"));
            set_str(&object, "originalHash", &format!("{original_hash:?}"));
            set_u64(&object, "totalSize", total_size);
        }
        Journaled::RouteRemoved { destination, cause } => {
            let kind = match cause {
                RouteRemovalCause::Expired => "routeExpired",
                RouteRemovalCause::Evicted => "routeEvicted",
                RouteRemovalCause::InterfaceGone => "routeInterfaceGone",
            };
            set_str(&object, "type", kind);
            set_bytes(&object, "destination", destination.as_bytes());
        }
    }
    object.into()
}

fn outbound_to_js(frame: &OutboundFrame) -> JsValue {
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

fn usb_auto_message_to_js(message: usb_auto_core::Message<'_>) -> JsValue {
    let object = Object::new();
    match message {
        usb_auto_core::Message::Hello(_) => set_str(&object, "type", "hello"),
        usb_auto_core::Message::HelloAck { tag, .. } => {
            set_str(&object, "type", "helloAck");
            set_bytes(&object, "tag", &tag.0);
        }
        usb_auto_core::Message::Data(packet) => {
            set_str(&object, "type", "data");
            set_bytes(&object, "bytes", packet);
        }
    }
    object.into()
}

fn bluetooth_control_to_js(control: bluetooth_core::Control) -> JsValue {
    let object = Object::new();
    match control {
        bluetooth_core::Control::Hello {
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
        bluetooth_core::Control::Welcome {
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
        bluetooth_core::Control::Close { reason } => {
            set_str(&object, "type", "close");
            set_str(&object, "reason", &format!("{reason:?}"));
        }
    }
    object.into()
}

fn write_usb_auto_frame(message: usb_auto_core::Message<'_>) -> Result<Vec<u8>, JsValue> {
    let mut out = vec![0u8; usb_auto_core::MAX_FRAMED_BYTES];
    let len = message
        .write_framed(&mut out)
        .map_err(|error| JsValue::from_str(&format!("USB-auto frame encode failed: {error:?}")))?;
    out.truncate(len);
    Ok(out)
}

fn node_tag_from_vec(bytes: Vec<u8>) -> Result<usb_auto_core::NodeTag, JsValue> {
    let Ok(tag) = <[u8; usb_auto_core::NODE_TAG_LEN]>::try_from(bytes) else {
        return Err(JsValue::from_str("USB-auto node tag must be 8 bytes"));
    };
    Ok(usb_auto_core::NodeTag(tag))
}

fn write_bluetooth_control(control: bluetooth_core::Control) -> Result<Vec<u8>, JsValue> {
    let mut out = vec![0u8; bluetooth_core::CONTROL_MAX_LEN];
    let len = control
        .encode(&mut out)
        .ok_or_else(|| JsValue::from_str("Bluetooth control encode failed"))?;
    out.truncate(len);
    Ok(out)
}

fn web_bluetooth_local(identity: Vec<u8>) -> Result<bluetooth_core::Local, JsValue> {
    let identity = bluetooth_identity_from_vec(identity)?;
    Ok(bluetooth_core::Local {
        identity,
        // Web Bluetooth exposes the central-side GATT floor, not L2CAP CoC. Android is the
        // already-deployed GATT-only endpoint remote firmware understands today.
        endpoint: bluetooth_core::Endpoint::Android(bluetooth_core::AndroidHost::Android),
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

fn required_value(object: &JsValue, key: &str) -> Result<JsValue, JsValue> {
    let value = Reflect::get(object, &JsValue::from_str(key))
        .map_err(|_| JsValue::from_str(&format!("failed to read {key}")))?;
    if value.is_undefined() || value.is_null() {
        return Err(JsValue::from_str(&format!("missing required option {key}")));
    }
    Ok(value)
}

fn optional_value(object: &JsValue, key: &str) -> Result<Option<JsValue>, JsValue> {
    let value = Reflect::get(object, &JsValue::from_str(key))
        .map_err(|_| JsValue::from_str(&format!("failed to read {key}")))?;
    if value.is_undefined() || value.is_null() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn required_string(object: &JsValue, key: &str) -> Result<String, JsValue> {
    required_value(object, key)?
        .as_string()
        .ok_or_else(|| JsValue::from_str(&format!("{key} must be a string")))
}

fn required_array(object: &JsValue, key: &str) -> Result<Array, JsValue> {
    let value = required_value(object, key)?;
    if !Array::is_array(&value) {
        return Err(JsValue::from_str(&format!("{key} must be an array")));
    }
    Ok(Array::from(&value))
}

fn required_bytes(object: &JsValue, key: &str) -> Result<Vec<u8>, JsValue> {
    bytes_from_value(required_value(object, key)?, key)
}

fn optional_bytes(object: &JsValue, key: &str) -> Result<Option<Vec<u8>>, JsValue> {
    optional_value(object, key)?
        .map(|value| bytes_from_value(value, key))
        .transpose()
}

fn bytes_from_value(value: JsValue, key: &str) -> Result<Vec<u8>, JsValue> {
    let Some(array) = value.dyn_ref::<Uint8Array>() else {
        return Err(JsValue::from_str(&format!("{key} must be a Uint8Array")));
    };
    Ok(array.to_vec())
}

fn required_u64(object: &JsValue, key: &str) -> Result<u64, JsValue> {
    let value = required_value(object, key)?;
    let number = value
        .as_f64()
        .ok_or_else(|| JsValue::from_str(&format!("{key} must be a number")))?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return Err(JsValue::from_str(&format!(
            "{key} must be a non-negative integer"
        )));
    }
    if number > u64::MAX as f64 {
        return Err(JsValue::from_str(&format!("{key} is too large")));
    }
    Ok(number as u64)
}

fn optional_u32(object: &JsValue, key: &str) -> Result<Option<u32>, JsValue> {
    let Some(value) = optional_value(object, key)? else {
        return Ok(None);
    };
    let number = value
        .as_f64()
        .ok_or_else(|| JsValue::from_str(&format!("{key} must be a number")))?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        return Err(JsValue::from_str(&format!(
            "{key} must be a non-negative integer"
        )));
    }
    if number > u32::MAX as f64 {
        return Err(JsValue::from_str(&format!("{key} is too large")));
    }
    Ok(Some(number as u32))
}

fn array_to_strings(values: &Array) -> Result<Vec<String>, JsValue> {
    let mut out = Vec::new();
    for value in values.iter() {
        let Some(value) = value.as_string() else {
            return Err(JsValue::from_str("aspects must be strings"));
        };
        out.push(value);
    }
    Ok(out)
}

fn secret_key_from_vec(
    bytes: Vec<u8>,
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, JsValue> {
    if bytes.len() != IDENTITY_SECRET_KEY_LEN {
        return Err(JsValue::from_str("identity secret key must be 64 bytes"));
    }
    let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret.copy_from_slice(&bytes);
    Ok(secret)
}

fn destination_hash_from_vec(bytes: Vec<u8>) -> Result<DestinationHash, JsValue> {
    if bytes.len() != TRUNCATED_HASH_BYTE_LEN {
        return Err(JsValue::from_str("destination hash must be 16 bytes"));
    }
    let mut hash = [0u8; TRUNCATED_HASH_BYTE_LEN];
    hash.copy_from_slice(&bytes);
    Ok(DestinationHash::new(hash))
}

fn interface_id_from_vec(bytes: Vec<u8>) -> Result<InterfaceId, JsValue> {
    if bytes.len() != INTERFACE_ID_LEN {
        return Err(JsValue::from_str("interface id must be 8 bytes"));
    }
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(&bytes);
    Ok(InterfaceId::new(id))
}

fn require_entropy(bytes: &[u8]) -> Result<(), JsValue> {
    if bytes.len() < DEFAULT_ENTROPY_FLOOR {
        return Err(JsValue::from_str(
            "operation requires at least 64 entropy bytes",
        ));
    }
    Ok(())
}

fn parse_interface_kind(kind: &str) -> Result<InterfaceKind, JsValue> {
    match kind {
        "auto-usb-host" | "usb-auto-host" | "AutoUSB" => Ok(InterfaceKind::UsbAutoHost),
        "auto-usb-device" | "usb-auto-device" => Ok(InterfaceKind::UsbAutoDevice),
        "rnode" | "RNode" => Ok(InterfaceKind::Rnode),
        "bluetooth-auto" | "ble-auto" => Ok(InterfaceKind::BluetoothAuto),
        "bluetooth-peer" | "ble-peer" => Ok(InterfaceKind::BluetoothPeer),
        "websocket-client" | "websocket" => Ok(InterfaceKind::WebSocketClient),
        "websocket-server" => Ok(InterfaceKind::WebSocketServer),
        "websocket-server-peer" => Ok(InterfaceKind::WebSocketServerPeer),
        "serial" => Ok(InterfaceKind::Serial),
        "kiss" => Ok(InterfaceKind::Kiss),
        "pipe" => Ok(InterfaceKind::Pipe),
        _ => Err(JsValue::from_str("unsupported interface kind")),
    }
}

fn interface_kind_name(kind: Option<InterfaceKind>) -> &'static str {
    match kind {
        Some(InterfaceKind::Loopback) => "loopback",
        Some(InterfaceKind::TcpClient) => "tcp-client",
        Some(InterfaceKind::TcpServer) => "tcp-server",
        Some(InterfaceKind::Udp) => "udp",
        Some(InterfaceKind::Serial) => "serial",
        Some(InterfaceKind::UsbAutoHost) => "usb-auto-host",
        Some(InterfaceKind::UsbAutoDevice) => "usb-auto-device",
        Some(InterfaceKind::AutoWifi) => "auto-wifi",
        Some(InterfaceKind::WifiPeer) => "wifi-peer",
        Some(InterfaceKind::LocalServer) => "local-server",
        Some(InterfaceKind::LocalClient) => "local-client",
        Some(InterfaceKind::TcpServerPeer) => "tcp-server-peer",
        Some(InterfaceKind::BluetoothAuto) => "bluetooth-auto",
        Some(InterfaceKind::BluetoothPeer) => "bluetooth-peer",
        Some(InterfaceKind::LoRa) => "lora",
        Some(InterfaceKind::Kiss) => "kiss",
        Some(InterfaceKind::Ax25Kiss) => "ax25-kiss",
        Some(InterfaceKind::Pipe) => "pipe",
        Some(InterfaceKind::Rnode) => "rnode",
        Some(InterfaceKind::BackboneServer) => "backbone-server",
        Some(InterfaceKind::BackboneServerPeer) => "backbone-server-peer",
        Some(InterfaceKind::BackboneClient) => "backbone-client",
        Some(InterfaceKind::EspNow) => "esp-now",
        Some(InterfaceKind::WifiDirect) => "wifi-direct",
        Some(InterfaceKind::WifiDirectPeer) => "wifi-direct-peer",
        Some(InterfaceKind::WifiAware) => "wifi-aware",
        Some(InterfaceKind::WifiAwarePeer) => "wifi-aware-peer",
        Some(InterfaceKind::WebSocketClient) => "websocket-client",
        Some(InterfaceKind::WebSocketServer) => "websocket-server",
        Some(InterfaceKind::WebSocketServerPeer) => "websocket-server-peer",
        None => "unknown",
    }
}

fn set_str(object: &Object, key: &str, value: &str) {
    set_value(object, key, JsValue::from_str(value));
}

fn set_u32(object: &Object, key: &str, value: u32) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

fn set_i32(object: &Object, key: &str, value: i32) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

fn set_bool(object: &Object, key: &str, value: bool) {
    set_value(object, key, JsValue::from_bool(value));
}

fn set_u64(object: &Object, key: &str, value: u64) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

fn set_usize(object: &Object, key: &str, value: usize) {
    set_value(object, key, JsValue::from_f64(value as f64));
}

fn set_bytes(object: &Object, key: &str, value: &[u8]) {
    set_value(object, key, Uint8Array::from(value).into());
}

fn set_value(object: &Object, key: &str, value: JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}
