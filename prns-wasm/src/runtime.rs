use core::convert::TryFrom;

use js_sys::{Array, Object};
use personal_rns::engine::{
    AllowRequester, AnnounceAppData, AnnounceNow, AnnounceTarget, CloseLink, CommandId, Directive,
    EngineCommand, EngineReaction, EngineState, EstablishLink, FanTarget, Identify, InstantMillis,
    IssuedCommand, Journaled, PathRequestId, RatchetPolicy, RequestPath, RequestResponseTimeout,
    Respond, RespondData, RespondPayload, SendRequest, SendRequestData, SendSinglePacket,
    SendSinglePacketPayload, SendToChannel, SendToChannelBody, SendToLink, SendToLinkPayload,
    SetResourceStrategy,
};
use personal_rns::interfaces::bluetooth_auto as bluetooth_contract;
use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, Capabilities, InboundPacket, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceMode,
};
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::links::request::RequestId;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::{RequestPathHash, RequestPolicy};
use personal_rns::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use personal_rns::routing::warmth::Departure;
use personal_rns::storage::GrowableHeap;
use personal_rns::units::DurationMillis;
use prns_host::PrnsLimits;
use prns_host_cooperative::{CooperativeHost, Entropy, MonotonicMillis};
use wasm_bindgen::prelude::*;

use crate::input::{
    array_to_strings, destination_hash_from_vec, identity_hash_from_vec, interface_id_from_vec,
    link_id_from_vec, optional_array, optional_bytes, optional_u32, optional_u64,
    parse_interface_kind, request_id_from_vec, request_path_hash_from_vec, required_array,
    required_bool, required_bytes, required_string, required_u64, secret_key_from_vec,
};
use crate::js_translation::{
    interface_kind_name, journaled_to_js, outbound_to_js, set_bytes, set_str, set_u32, set_u64,
    set_usize, set_value,
};
use crate::parameters::bitrate_bps_u32;

#[derive(Clone, Copy)]
enum NodeResponse {
    Index,
    #[cfg(feature = "source-archive")]
    SourceArchive,
    #[cfg(feature = "source-archive")]
    SourceChecksum,
}

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
    ble_identity: Option<bluetooth_contract::BleIdentity>,
    node_page: bool,
    host: CooperativeHost<()>,
}

#[wasm_bindgen]
impl PrnsRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new(
        identity_secret_key: Vec<u8>,
        ble_identity: Option<Vec<u8>>,
    ) -> Result<PrnsRuntime, JsValue> {
        let secret = secret_key_from_vec(identity_secret_key)?;
        let ble_identity = ble_identity
            .map(|bytes| {
                let identity: [u8; 16] = bytes.try_into().map_err(|_| {
                    JsValue::from_str("Bluetooth LE identity must be exactly 16 bytes")
                })?;
                Ok::<_, JsValue>(bluetooth_contract::BleIdentity::new(identity))
            })
            .transpose()?;
        Ok(Self {
            engine: EngineState::new(secret),
            interfaces: Vec::new(),
            events: Vec::new(),
            outbound: Vec::new(),
            next_command_id: 0,
            ble_identity,
            node_page: false,
            host: CooperativeHost::new(PrnsLimits::balanced()),
        })
    }

    #[wasm_bindgen(js_name = registerInterface)]
    pub fn register_interface(&mut self, options: JsValue) -> Result<Vec<u8>, JsValue> {
        let kind = parse_interface_kind(&required_string(&options, "kind")?)?;
        let channel_tag = required_bytes(&options, "channelTag")?;
        let now_ms = required_u64(&options, "nowMs")?;
        self.host
            .observe_time(MonotonicMillis::new(now_ms))
            .map_err(|error| JsValue::from_str(&format!("host time moved backwards: {error:?}")))?;
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
        self.engine.interface_attached(id, InstantMillis(now_ms));
        Ok(id.as_bytes().to_vec())
    }

    #[wasm_bindgen(js_name = removeInterface)]
    pub fn remove_interface(&mut self, options: JsValue) -> Result<bool, JsValue> {
        let interface_id = required_bytes(&options, "interfaceId")?;
        let now_ms = required_u64(&options, "nowMs")?;
        self.host
            .observe_time(MonotonicMillis::new(now_ms))
            .map_err(|error| JsValue::from_str(&format!("host time moved backwards: {error:?}")))?;
        let id = interface_id_from_vec(interface_id)?;
        let before = self.interfaces.len();
        self.interfaces.retain(|interface| interface.id != id);
        let removed = self.interfaces.len() != before;
        if removed {
            self.engine
                .interface_departed(id, Departure::MayReturn, InstantMillis(now_ms));
        }
        Ok(removed)
    }

    #[wasm_bindgen(js_name = bluetoothIdentity)]
    pub fn bluetooth_identity(&self) -> Result<Vec<u8>, JsValue> {
        self.ble_identity
            .as_ref()
            .map(|identity| identity.as_bytes().to_vec())
            .ok_or_else(|| JsValue::from_str("persisted Bluetooth LE identity is unavailable"))
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
        if let Some(handlers) = optional_array(&options, "requestHandlers")? {
            for handler in handlers.iter() {
                let path = required_string(&handler, "path")?;
                let policy = match required_string(&handler, "policy")?.as_str() {
                    "AllowNone" => RequestPolicy::AllowNone,
                    "AllowAll" => RequestPolicy::AllowAll,
                    "AllowList" => RequestPolicy::AllowList,
                    _ => {
                        return Err(JsValue::from_str(
                            "request handler policy must be AllowNone, AllowAll, or AllowList",
                        ));
                    }
                };
                self.engine
                    .register_request_handler(&destination, &path, policy)
                    .map_err(|error| {
                        JsValue::from_str(&format!(
                            "request handler registration failed: {error:?}"
                        ))
                    })?;
            }
        }
        Ok(destination.as_bytes().to_vec())
    }

    #[wasm_bindgen(js_name = registerNodePage)]
    pub fn register_node_page(&mut self, options: JsValue) -> Result<Vec<u8>, JsValue> {
        let mut app_data = optional_bytes(&options, "appData")?.unwrap_or_default();
        let Some(identity) = self.engine.held_identity_hashes().first().copied() else {
            return Err(JsValue::from_str("runtime has no held identity"));
        };
        let derived = personal_rns::routing::announce::derive_single_destination_hash(
            &identity,
            personal_hopspot_core::node_pages::NODE_APP_NAME,
            personal_hopspot_core::node_pages::NODE_ASPECTS,
        )
        .map_err(|error| JsValue::from_str(&format!("node page name is invalid: {error:?}")))?;
        if !app_data.is_empty() {
            app_data.push(b' ');
        }
        let tag = derived.as_bytes();
        app_data.extend_from_slice(format!("{:02x}{:02x}", tag[0], tag[1]).as_bytes());
        let destination = self
            .engine
            .register_single_destination(
                &identity,
                personal_hopspot_core::node_pages::NODE_APP_NAME,
                personal_hopspot_core::node_pages::NODE_ASPECTS,
                &app_data,
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .map_err(|error| {
                JsValue::from_str(&format!("node page registration failed: {error:?}"))
            })?;
        for (path, policy) in <personal_hopspot_core::node_pages::NodePageRoutes as personal_rns::runtime::request_router::RouteSet<()>>::REGISTRATIONS {
            self.engine
                .register_request_handler(&destination, path, policy.engine_policy())
                .map_err(|error| {
                    JsValue::from_str(&format!("node page handler failed: {error:?}"))
                })?;
        }
        self.node_page = true;
        Ok(destination.as_bytes().to_vec())
    }

    #[wasm_bindgen(js_name = announce)]
    pub fn announce(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let destination = required_bytes(&options, "destination")?;
        let now_ms = required_u64(&options, "nowMs")?;
        let entropy = required_bytes(&options, "entropy")?;
        let entropy = Entropy::try_new(entropy)
            .map_err(|error| JsValue::from_str(&format!("host entropy rejected: {error:?}")))?;
        let step = self
            .host
            .begin_step(MonotonicMillis::new(now_ms), entropy)
            .map_err(|error| JsValue::from_str(&format!("host time moved backwards: {error:?}")))?;
        let destination = destination_hash_from_vec(destination)?;
        let target = optional_bytes(&options, "interfaceId")?
            .map(interface_id_from_vec)
            .transpose()?
            .map_or(AnnounceTarget::AllInterfaces, AnnounceTarget::Interface);
        let id = self.mint_command_id();
        let command = EngineCommand::AnnounceNow(AnnounceNow {
            destination,
            target,
            app_data: AnnounceAppData::Registered,
        });
        self.ingest_command(id, command, now_ms, step.entropy.as_bytes().to_vec());
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = sendSinglePacket)]
    pub fn send_single_packet(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let destination = destination_hash_from_vec(required_bytes(&options, "destination")?)?;
        let payload = required_bytes(&options, "payload")?;
        let payload = SendSinglePacketPayload::from_slice(&payload)
            .map_err(|_| JsValue::from_str("payload exceeds the single packet limit"))?;
        let now_ms = required_u64(&options, "nowMs")?;
        let entropy = required_bytes(&options, "entropy")?;
        let entropy = Entropy::try_new(entropy)
            .map_err(|error| JsValue::from_str(&format!("host entropy rejected: {error:?}")))?;
        let step = self
            .host
            .begin_step(MonotonicMillis::new(now_ms), entropy)
            .map_err(|error| JsValue::from_str(&format!("host time moved backwards: {error:?}")))?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::SendSinglePacket(SendSinglePacket {
                destination,
                payload,
            }),
            now_ms,
            step.entropy.as_bytes().to_vec(),
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = closeLink)]
    pub fn close_link(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let link_id = link_id_from_vec(required_bytes(&options, "linkId")?)?;
        let now_ms = required_u64(&options, "nowMs")?;
        let entropy = required_bytes(&options, "entropy")?;
        let entropy = Entropy::try_new(entropy)
            .map_err(|error| JsValue::from_str(&format!("host entropy rejected: {error:?}")))?;
        let step = self
            .host
            .begin_step(MonotonicMillis::new(now_ms), entropy)
            .map_err(|error| JsValue::from_str(&format!("host time moved backwards: {error:?}")))?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::CloseLink(CloseLink { link_id }),
            now_ms,
            step.entropy.as_bytes().to_vec(),
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = establishLink)]
    pub fn establish_link(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let destination = destination_hash_from_vec(required_bytes(&options, "destination")?)?;
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::EstablishLink(EstablishLink { destination }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = requestPath)]
    pub fn request_path(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let destination = destination_hash_from_vec(required_bytes(&options, "destination")?)?;
        let (now_ms, entropy) = self.command_context(&options)?;
        let request_id = entropy
            .get(..personal_rns::engine::PATH_REQUEST_ID_LEN)
            .and_then(|bytes| bytes.try_into().ok())
            .map(PathRequestId::new)
            .ok_or_else(|| JsValue::from_str("host entropy is too short for a path request"))?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::RequestPath(RequestPath {
                destination,
                id: request_id,
            }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = identify)]
    pub fn identify(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let link_id = link_id_from_vec(required_bytes(&options, "linkId")?)?;
        let identity = identity_hash_from_vec(required_bytes(&options, "identity")?)?;
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::Identify(Identify { link_id, identity }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = sendLinkPacket)]
    pub fn send_link_packet(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let link_id = link_id_from_vec(required_bytes(&options, "linkId")?)?;
        let payload = SendToLinkPayload::from_slice(&required_bytes(&options, "payload")?)
            .map_err(|_| JsValue::from_str("payload exceeds the link packet limit"))?;
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::SendToLink(SendToLink { link_id, payload }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = request)]
    pub fn request(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let link_id = link_id_from_vec(required_bytes(&options, "linkId")?)?;
        let path_hash = request_path_hash_from_vec(required_bytes(&options, "pathHash")?)?;
        let data = SendRequestData::from_slice(&required_bytes(&options, "payload")?)
            .map_err(|_| JsValue::from_str("payload exceeds the request packet limit"))?;
        let response_timeout = optional_u64(&options, "timeoutMillis")?
            .map(|millis| RequestResponseTimeout::Exact(DurationMillis(millis)))
            .unwrap_or(RequestResponseTimeout::LinkDefault);
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::SendRequest(SendRequest {
                link_id,
                path_hash,
                data,
                response_timeout,
            }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = respond)]
    pub fn respond(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let link_id = link_id_from_vec(required_bytes(&options, "linkId")?)?;
        let request_id = request_id_from_vec(required_bytes(&options, "requestId")?)?;
        let payload = RespondData::from_slice(&required_bytes(&options, "payload")?)
            .map_err(|_| JsValue::from_str("payload exceeds the response packet limit"))?;
        let _ = required_u64(&options, "requestRttMillis")?;
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::Respond(Respond {
                link_id,
                request_id,
                payload: RespondPayload::Packed(payload),
            }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = setLinkResourceStrategy)]
    pub fn set_link_resource_strategy(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let link_id = link_id_from_vec(required_bytes(&options, "linkId")?)?;
        let strategy = resource_strategy(&options)?;
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::SetResourceStrategy(SetResourceStrategy { link_id, strategy }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = setDestinationResourceStrategy)]
    pub fn set_destination_resource_strategy(&mut self, options: JsValue) -> Result<bool, JsValue> {
        let destination = destination_hash_from_vec(required_bytes(&options, "destination")?)?;
        let strategy = resource_strategy(&options)?;
        Ok(self
            .engine
            .set_default_resource_strategy(&destination, strategy))
    }

    #[wasm_bindgen(js_name = sendChannelMessage)]
    pub fn send_channel_message(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let link_id = link_id_from_vec(required_bytes(&options, "linkId")?)?;
        let message_type = u16::try_from(required_u64(&options, "messageType")?)
            .ok()
            .map(MessageType)
            .filter(|kind| !kind.is_system_reserved())
            .ok_or_else(|| JsValue::from_str("messageType must be an application message type"))?;
        let body = SendToChannelBody::from_slice(&required_bytes(&options, "payload")?)
            .map_err(|_| JsValue::from_str("payload exceeds the channel message limit"))?;
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::SendToChannel(SendToChannel {
                link_id,
                message_type,
                body,
            }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = allowRequester)]
    pub fn allow_requester(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let destination = destination_hash_from_vec(required_bytes(&options, "destination")?)?;
        let path_hash = request_path_hash_from_vec(required_bytes(&options, "pathHash")?)?;
        let identity = identity_hash_from_vec(required_bytes(&options, "identity")?)?;
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            EngineCommand::AllowRequester(AllowRequester {
                destination,
                path_hash,
                identity,
            }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = ingest)]
    pub fn ingest(&mut self, options: JsValue) -> Result<(), JsValue> {
        let interface_id = required_bytes(&options, "interfaceId")?;
        let bytes = required_bytes(&options, "bytes")?;
        let now_ms = required_u64(&options, "nowMs")?;
        let entropy = required_bytes(&options, "entropy")?;
        let entropy = Entropy::try_new(entropy)
            .map_err(|error| JsValue::from_str(&format!("host entropy rejected: {error:?}")))?;
        let step = self
            .host
            .begin_step(MonotonicMillis::new(now_ms), entropy)
            .map_err(|error| JsValue::from_str(&format!("host time moved backwards: {error:?}")))?;
        let source_interface = interface_id_from_vec(interface_id)?;
        let mut bytes = bytes;
        let mut entropy = EntropyCursor::new(step.entropy.as_bytes().to_vec());
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
        let node_page = self.node_page;
        let index_path = RequestPathHash::of(personal_hopspot_core::node_pages::INDEX_PATH);
        #[cfg(feature = "source-archive")]
        let source_path =
            RequestPathHash::of(personal_hopspot_core::node_pages::SOURCE_ARCHIVE_PATH);
        #[cfg(feature = "source-archive")]
        let checksum_path =
            RequestPathHash::of(personal_hopspot_core::node_pages::SOURCE_CHECKSUM_PATH);
        let mut page_requests: Vec<(LinkId, RequestId, NodeResponse)> = Vec::new();
        self.engine.ingest_packet_into(
            packet,
            personal_rns::engine::IngestIo {
                interfaces: personal_rns::interfaces::AttachedInterfaces::new(&interfaces_snapshot),
                now: InstantMillis(now_ms),
                fill_entropy: &mut |out| entropy.fill(out),
                should_prove: &mut should_prove,
                should_accept_resource: &mut should_accept_resource,
                sink: &mut |reaction| {
                    if let EngineReaction::Journaled(Journaled::RequestReceived {
                        link_id,
                        request_id,
                        path_hash,
                        ..
                    }) = &reaction
                    {
                        if node_page && *path_hash == index_path {
                            page_requests.push((*link_id, *request_id, NodeResponse::Index));
                        }
                        #[cfg(feature = "source-archive")]
                        if node_page && *path_hash == source_path {
                            page_requests.push((
                                *link_id,
                                *request_id,
                                NodeResponse::SourceArchive,
                            ));
                        }
                        #[cfg(feature = "source-archive")]
                        if node_page && *path_hash == checksum_path {
                            page_requests.push((
                                *link_id,
                                *request_id,
                                NodeResponse::SourceChecksum,
                            ));
                        }
                    }
                    reactions.push(capture_reaction(reaction));
                },
            },
        );
        self.apply_captured(reactions);
        for (link_id, request_id, response) in page_requests {
            let id = self.mint_command_id();
            let mut respond_reactions = Vec::new();
            self.engine.ingest_command_into(
                IssuedCommand {
                    id,
                    command: EngineCommand::Respond(Respond {
                        link_id,
                        request_id,
                        payload: match response {
                            NodeResponse::Index => RespondPayload::StaticBytes(
                                personal_hopspot_core::node_pages::BROWSER_INDEX_PAGE,
                            ),
                            #[cfg(feature = "source-archive")]
                            NodeResponse::SourceArchive => RespondPayload::StaticFile {
                                name: "source.zip",
                                bytes: personal_hopspot_core::node_pages::SOURCE_ARCHIVE,
                            },
                            #[cfg(feature = "source-archive")]
                            NodeResponse::SourceChecksum => RespondPayload::StaticFile {
                                name: "source.zip.sha256",
                                bytes: personal_hopspot_core::node_pages::SOURCE_CHECKSUM,
                            },
                        },
                    }),
                },
                personal_rns::interfaces::AttachedInterfaces::new(&interfaces_snapshot),
                InstantMillis(now_ms),
                &mut |out| entropy.fill(out),
                &mut |reaction| respond_reactions.push(capture_reaction(reaction)),
            );
            self.apply_captured(respond_reactions);
        }
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
    fn command_context(&mut self, options: &JsValue) -> Result<(u64, Vec<u8>), JsValue> {
        let now_ms = required_u64(options, "nowMs")?;
        let entropy = Entropy::try_new(required_bytes(options, "entropy")?)
            .map_err(|error| JsValue::from_str(&format!("host entropy rejected: {error:?}")))?;
        let step = self
            .host
            .begin_step(MonotonicMillis::new(now_ms), entropy)
            .map_err(|error| JsValue::from_str(&format!("host time moved backwards: {error:?}")))?;
        Ok((now_ms, step.entropy.as_bytes().to_vec()))
    }

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

fn resource_strategy(options: &JsValue) -> Result<ResourceStrategy, JsValue> {
    match required_string(options, "strategy")?.as_str() {
        "refuse" => Ok(ResourceStrategy::AcceptNone),
        "accept" => Ok(ResourceStrategy::Accept {
            max_uncompressed_bytes: required_u64(options, "maximumUncompressedBytes")?,
            accept_compressed: required_bool(options, "acceptCompressed")?,
        }),
        _ => Err(JsValue::from_str("strategy must be refuse or accept")),
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
