use core::convert::TryFrom;

use js_sys::{Array, Object};
use personal_rns::crypto::ratchets::SeedSelfRatchetsOutcome;
use personal_rns::engine::{
    AllowRequester, AnnounceAppData, AnnounceNow, AnnounceTarget, CloseLink, CommandId,
    DestinationIdentitySeedOutcome, Directive, EngineReaction, EngineState, EstablishLink,
    FanTarget, Identify, InstantMillis, IssuedCommand, Journaled, PathRequestId, PrnsCommand,
    RatchetPolicy, RequestPath, RequestResponseTimeout, Respond, RespondData, RespondPayload,
    RouteSeedOutcome, SendRequest, SendRequestData, SendSinglePacket, SendSinglePacketPayload,
    SendToChannel, SendToChannelBody, SendToLink, SendToLinkPayload, SetResourceStrategy,
};
use personal_rns::interfaces::bluetooth_auto as bluetooth_contract;
use personal_rns::interfaces::{
    AnnounceBandwidthCap, BitrateBps, Capabilities, InboundPacket, InterfaceCapabilities,
    InterfaceCommonPolicy, InterfaceDescriptor, InterfaceGravity, InterfaceId, InterfaceKind,
    InterfaceMode, RecursivePathRequestPolicy,
};
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::links::request::RequestId;
use personal_rns::routing::links::resources::{
    ResourceBody, ResourceCorrelation, ResourceMetadata, ResourceSend, ResourceSendPlan,
    ResourceSendPlanError, ResourceStrategy, MAX_EFFICIENT_SIZE,
};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::{RequestPathHash, RequestPolicy};
use personal_rns::routing::routes::NextHop;
use personal_rns::routing::tunnel::SeedTunnelOutcome;
use personal_rns::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
use personal_rns::routing::warmth::Departure;
use personal_rns::storage::GrowableHeap;
use personal_rns::units::{ByteLimit, DurationMillis};
use prns_host::{EventBatchProjection, EventProjection, PrnsLimits};
use prns_host_cooperative::{CooperativeHost, Entropy, MonotonicMillis};
use prns_runtime::runtime::persistence_snapshots::{
    snapshot_persisted_state, snapshot_self_ratchets,
};
use wasm_bindgen::prelude::*;

use crate::event_projection::{capture_journaled, CapturedJournal};
use crate::input::{
    array_to_strings, destination_hash_from_vec, identity_hash_from_vec, interface_id_from_vec,
    link_id_from_vec, optional_array, optional_bool, optional_bytes, optional_i64, optional_string,
    optional_u32, optional_u64, parse_interface_kind, parse_interface_mode, request_id_from_vec,
    request_path_hash_from_vec, required_array, required_bool, required_bytes, required_string,
    required_u64, secret_key_from_vec,
};
use crate::js_translation::{
    interface_kind_name, outbound_to_js, set_bigint, set_bytes, set_str, set_u32, set_u64,
    set_usize, set_value,
};
use crate::parameters::{bitrate_bps_u32, BROWSER_PERSISTENCE_VERSION};

#[derive(Clone, Copy)]
enum NodeResponse {
    Index,
    Quickstart,
    ComingFromRns,
    SourcePage,
    #[cfg(feature = "source-archive")]
    SourceArchive,
    #[cfg(feature = "source-archive")]
    SourceChecksum,
}

const MAX_PERSISTED_STATE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PERSISTED_RATCHETS: usize = 4_096;
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
    events: Vec<EventProjection>,
    control_events: Vec<JsValue>,
    outbound: Vec<OutboundFrame>,
    next_command_id: u64,
    revision: u64,
    ble_identity: Option<bluetooth_contract::BleIdentity>,
    node_page: bool,
    host: CooperativeHost<()>,
    pending_ratchets: Vec<(personal_rns::wire::DestinationHash, Vec<u8>)>,
    persistence_restored: bool,
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
            control_events: Vec::new(),
            outbound: Vec::new(),
            next_command_id: 0,
            revision: 0,
            ble_identity,
            node_page: false,
            host: CooperativeHost::new(PrnsLimits::balanced()),
            pending_ratchets: Vec::new(),
            persistence_restored: false,
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
        let mode = optional_string(&options, "mode")?
            .map(|mode| parse_interface_mode(&mode))
            .transpose()?
            .unwrap_or(InterfaceMode::Full);
        let gravity = optional_i64(&options, "gravity")?
            .map(InterfaceGravity::new)
            .unwrap_or(InterfaceGravity::ZERO);
        let mut common = InterfaceCommonPolicy::RNS_DEFAULT;
        if let Some(value) = optional_bool(&options, "recursivePathRequests")? {
            common.forwarding.recursive_path_requests =
                RecursivePathRequestPolicy::from_configured(value);
        }
        if let Some(value) = optional_bool(&options, "announcesFromInternal")? {
            common.forwarding.announces_from_internal = value;
        }
        if let Some(value) = optional_bool(&options, "announcesToInternal")? {
            common.forwarding.announces_to_internal = value;
        }
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
            mode,
            gravity,
            bitrate,
            hardware_mtu: hardware_mtu.map(|mtu| mtu as usize),
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
            airtime_duty_cycle: None,
            common,
        };
        if let Some(slot) = self.interfaces.iter_mut().find(|iface| iface.id == id) {
            *slot = descriptor;
        } else {
            self.interfaces.push(descriptor);
        }
        self.engine.interface_attached(id, InstantMillis(now_ms));
        self.bump_revision();
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
            self.bump_revision();
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
        self.engine.set_maximum_request_bytes(
            &destination,
            ByteLimit::from(optional_u64(&options, "maximumRequestBytes")?),
        );
        self.bump_revision();
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
        self.restore_pending_ratchet(destination)?;
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
        self.bump_revision();
        for (path, policy) in <personal_hopspot_core::node_pages::BrowserNodePageRoutes as personal_rns::runtime::request_endpoints::RequestEndpointSet<()>>::REGISTRATIONS {
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
        let command = PrnsCommand::AnnounceNow(AnnounceNow {
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
            PrnsCommand::SendSinglePacket(SendSinglePacket {
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
            PrnsCommand::CloseLink(CloseLink { link_id }),
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
            PrnsCommand::EstablishLink(EstablishLink { destination }),
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
            PrnsCommand::RequestPath(RequestPath {
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
            PrnsCommand::Identify(Identify { link_id, identity }),
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
            PrnsCommand::SendToLink(SendToLink { link_id, payload }),
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
        let maximum_response_bytes =
            ByteLimit::from(optional_u64(&options, "maximumResponseBytes")?);
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        self.ingest_command(
            id,
            PrnsCommand::SendRequest(SendRequest {
                link_id,
                path_hash,
                data,
                response_timeout,
                maximum_response_bytes,
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
            PrnsCommand::Respond(Respond {
                link_id,
                request_id,
                payload: RespondPayload::Packed(payload),
            }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = resourceSegmentPlan)]
    pub fn resource_segment_plan(&self, options: JsValue) -> Result<JsValue, JsValue> {
        let total_data_bytes = required_u64(&options, "totalDataBytes")?;
        let packed_metadata_bytes = optional_u64(&options, "packedMetadataBytes")?;
        let segment_index = required_u64(&options, "segmentIndex")?;
        let plan = match ResourceSendPlan::new(
            total_data_bytes,
            packed_metadata_bytes,
            MAX_EFFICIENT_SIZE as u64,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let rejected = Object::new();
                set_str(&rejected, "type", "rejected");
                set_str(
                    &rejected,
                    "cause",
                    match error {
                        ResourceSendPlanError::PackedMetadataLengthOverflow
                        | ResourceSendPlanError::MetadataDoesNotFit => "metadataTooLarge",
                        ResourceSendPlanError::TotalLengthOverflow => "payloadTooLarge",
                        ResourceSendPlanError::ZeroSegmentBytes
                        | ResourceSendPlanError::SegmentTooLarge => "invalidSegmentSize",
                    },
                );
                return Ok(rejected.into());
            }
        };
        let Some(segment) = plan.segment(segment_index) else {
            let rejected = Object::new();
            set_str(&rejected, "type", "rejected");
            set_str(&rejected, "cause", "invalidSegmentIndex");
            return Ok(rejected.into());
        };
        let ready = Object::new();
        set_str(&ready, "type", "ready");
        set_u64(&ready, "totalStreamBytes", plan.total_stream_bytes());
        set_u64(&ready, "segmentIndex", segment.segment.index);
        set_u64(&ready, "totalSegments", segment.segment.total_segments);
        set_u64(&ready, "totalDataBytes", segment.segment.total_data_bytes);
        set_u64(&ready, "dataStart", segment.data_start);
        set_u64(&ready, "dataEnd", segment.data_end);
        set_u64(&ready, "streamBytes", segment.stream_bytes);
        Ok(ready.into())
    }

    #[wasm_bindgen(js_name = sendResourceSegment)]
    pub fn send_resource_segment(&mut self, options: JsValue) -> Result<u64, JsValue> {
        let link_id = link_id_from_vec(required_bytes(&options, "linkId")?)?;
        let data = required_bytes(&options, "payload")?;
        let compressed_candidate = optional_bytes(&options, "compressedCandidate")?;
        let metadata_kind = required_string(&options, "metadata")?;
        let packed_metadata = optional_bytes(&options, "packedMetadata")?;
        let packed_metadata_bytes = optional_u32(&options, "packedMetadataBytes")?;
        let (metadata, metadata_len) = match (
            metadata_kind.as_str(),
            &packed_metadata,
            packed_metadata_bytes,
        ) {
            ("none", None, None) => (ResourceMetadata::None, None),
            ("packed", Some(packed), None) => {
                (ResourceMetadata::Packed(packed), Some(packed.len() as u64))
            }
            ("sentInFirstSegment", None, Some(packed_len)) => (
                ResourceMetadata::SentInFirstSegment { packed_len },
                Some(u64::from(packed_len)),
            ),
            _ => {
                return Err(JsValue::from_str(
                    "resource segment metadata fields are inconsistent",
                ));
            }
        };
        let total_data_bytes = required_u64(&options, "totalDataBytes")?;
        let segment_index = required_u64(&options, "segmentIndex")?;
        let plan = ResourceSendPlan::new(total_data_bytes, metadata_len, MAX_EFFICIENT_SIZE as u64)
            .map_err(|error| {
                JsValue::from_str(&format!("resource send plan rejected: {error:?}"))
            })?;
        let segment = plan
            .segment(segment_index)
            .ok_or_else(|| JsValue::from_str("resource segment index is outside the send plan"))?;
        let expected_data_bytes = segment.data_end.saturating_sub(segment.data_start);
        if data.len() as u64 != expected_data_bytes {
            return Err(JsValue::from_str(
                "resource segment payload does not match the send plan",
            ));
        }
        let (now_ms, entropy) = self.command_context(&options)?;
        let id = self.mint_command_id();
        let mut entropy = EntropyCursor::new(entropy);
        let mut reactions = Vec::new();
        self.engine.ingest_send_resource_segment_into(
            &ResourceSend {
                id,
                link_id,
                body: ResourceBody {
                    data: &data,
                    compressed_candidate: compressed_candidate.as_deref(),
                    metadata,
                },
                correlation: ResourceCorrelation::Unsolicited,
            },
            segment.segment,
            InstantMillis(now_ms),
            &mut |out| entropy.fill(out),
            &mut |reaction| reactions.push(capture_reaction(reaction)),
        );
        self.apply_captured(reactions);
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
            PrnsCommand::SetResourceStrategy(SetResourceStrategy { link_id, strategy }),
            now_ms,
            entropy,
        );
        Ok(id.0)
    }

    #[wasm_bindgen(js_name = setDestinationResourceStrategy)]
    pub fn set_destination_resource_strategy(&mut self, options: JsValue) -> Result<bool, JsValue> {
        let destination = destination_hash_from_vec(required_bytes(&options, "destination")?)?;
        let strategy = resource_strategy(&options)?;
        let changed = self
            .engine
            .set_default_resource_strategy(&destination, strategy);
        if changed {
            self.bump_revision();
        }
        Ok(changed)
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
            PrnsCommand::SendToChannel(SendToChannel {
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
            PrnsCommand::AllowRequester(AllowRequester {
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
        let quickstart_path =
            RequestPathHash::of(personal_hopspot_core::node_pages::QUICKSTART_PATH);
        let coming_from_rns_path =
            RequestPathHash::of(personal_hopspot_core::node_pages::COMING_FROM_RNS_PATH);
        let source_page_path =
            RequestPathHash::of(personal_hopspot_core::node_pages::SOURCE_PAGE_PATH);
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
                        if node_page && *path_hash == quickstart_path {
                            page_requests.push((*link_id, *request_id, NodeResponse::Quickstart));
                        }
                        if node_page && *path_hash == coming_from_rns_path {
                            page_requests.push((
                                *link_id,
                                *request_id,
                                NodeResponse::ComingFromRns,
                            ));
                        }
                        if node_page && *path_hash == source_page_path {
                            page_requests.push((*link_id, *request_id, NodeResponse::SourcePage));
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
        self.bump_revision();
        self.apply_captured(reactions);
        for (link_id, request_id, response) in page_requests {
            let id = self.mint_command_id();
            let mut respond_reactions = Vec::new();
            self.engine.ingest_command_into(
                IssuedCommand {
                    id,
                    command: PrnsCommand::Respond(Respond {
                        link_id,
                        request_id,
                        payload: match response {
                            NodeResponse::Index => RespondPayload::StaticBytes(
                                personal_hopspot_core::node_pages::BROWSER_INDEX_PAGE,
                            ),
                            NodeResponse::Quickstart => RespondPayload::StaticBytes(
                                personal_hopspot_core::node_pages::QUICKSTART_PAGE,
                            ),
                            NodeResponse::ComingFromRns => RespondPayload::StaticBytes(
                                personal_hopspot_core::node_pages::COMING_FROM_RNS_PAGE,
                            ),
                            NodeResponse::SourcePage => RespondPayload::StaticBytes(
                                personal_hopspot_core::node_pages::SOURCE_PAGE,
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
        for event in self.control_events.drain(..) {
            drained.push(&event);
        }
        drained
    }

    #[wasm_bindgen(js_name = drainEventBatch)]
    pub fn drain_event_batch(&mut self) -> Result<Vec<u8>, JsValue> {
        let batch = EventBatchProjection::from_records(core::mem::take(&mut self.events));
        batch.encode().map_err(|error| {
            JsValue::from_str(&format!("event batch projection failed: {error:?}"))
        })
    }

    #[wasm_bindgen(js_name = drainOutbound)]
    pub fn drain_outbound(&mut self) -> Array {
        let drained = Array::new();
        for frame in self.outbound.drain(..) {
            drained.push(&outbound_to_js(&frame));
        }
        drained
    }

    #[wasm_bindgen(js_name = persistedState)]
    pub fn persisted_state(&self, options: JsValue) -> Result<JsValue, JsValue> {
        let now_ms = required_u64(&options, "nowMs")?;
        let snapshot = snapshot_persisted_state(&self.engine, InstantMillis(now_ms))
            .ok_or_else(|| JsValue::from_str("runtime persistence snapshot exceeded its bounds"))?;
        let ratchet_snapshot = snapshot_self_ratchets(&self.engine);
        let ratchet_count = ratchet_snapshot
            .blobs
            .len()
            .checked_add(self.pending_ratchets.len())
            .ok_or_else(|| JsValue::from_str("persisted ratchet count is unsupported"))?;
        if ratchet_count > MAX_PERSISTED_RATCHETS {
            return Err(JsValue::from_str(
                "persisted ratchet count exceeds the runtime limit",
            ));
        }
        let mut persisted_bytes = 0;
        account_persisted_bytes(&mut persisted_bytes, snapshot.routing_table.len())?;
        account_persisted_bytes(&mut persisted_bytes, snapshot.tunnels.len())?;
        account_persisted_bytes(&mut persisted_bytes, snapshot.destination_identities.len())?;
        for (destination, sealed) in &ratchet_snapshot.blobs {
            account_persisted_bytes(&mut persisted_bytes, destination.as_bytes().len())?;
            account_persisted_bytes(&mut persisted_bytes, sealed.len())?;
        }
        for (destination, sealed) in &self.pending_ratchets {
            account_persisted_bytes(&mut persisted_bytes, destination.as_bytes().len())?;
            account_persisted_bytes(&mut persisted_bytes, sealed.len())?;
        }
        let object = Object::new();
        set_str(&object, "type", "persistedState");
        set_u32(&object, "persistenceVersion", BROWSER_PERSISTENCE_VERSION);
        set_u64(&object, "takenAtMillis", snapshot.taken_at.0);
        set_bytes(&object, "routingTable", &snapshot.routing_table);
        set_bytes(&object, "tunnels", &snapshot.tunnels);
        set_bytes(
            &object,
            "destinationIdentities",
            &snapshot.destination_identities,
        );
        let ratchets = Array::new();
        for (destination, sealed) in ratchet_snapshot.blobs {
            let row = Object::new();
            set_bytes(&row, "destination", destination.as_bytes());
            set_bytes(&row, "sealed", &sealed);
            ratchets.push(&row);
        }
        for (destination, sealed) in &self.pending_ratchets {
            let row = Object::new();
            set_bytes(&row, "destination", destination.as_bytes());
            set_bytes(&row, "sealed", sealed);
            ratchets.push(&row);
        }
        set_value(&object, "ratchets", ratchets.into());
        Ok(object.into())
    }

    #[wasm_bindgen(js_name = restorePersistedState)]
    pub fn restore_persisted_state(&mut self, options: JsValue) -> Result<JsValue, JsValue> {
        if self.persistence_restored {
            return Err(JsValue::from_str(
                "runtime persistence was already restored",
            ));
        }
        let persistence_version = required_u64(&options, "persistenceVersion")?;
        if persistence_version != u64::from(BROWSER_PERSISTENCE_VERSION) {
            return Err(JsValue::from_str("persisted state version is unsupported"));
        }
        let now_ms = required_u64(&options, "nowMs")?;
        let routing_table = bounded_persisted_region(&options, "routingTable")?;
        let tunnels = bounded_persisted_region(&options, "tunnels")?;
        let destination_identities = bounded_persisted_region(&options, "destinationIdentities")?;
        let mut persisted_bytes = 0;
        account_persisted_bytes(&mut persisted_bytes, routing_table.len())?;
        account_persisted_bytes(&mut persisted_bytes, tunnels.len())?;
        account_persisted_bytes(&mut persisted_bytes, destination_identities.len())?;
        validate_route_snapshot(&routing_table)?;
        validate_destination_identity_snapshot(&destination_identities)?;
        personal_rns::persistence::read_tunnels_snapshot(&tunnels)
            .map_err(|error| persisted_state_error("tunnels", error))?;
        let ratchet_values = required_array(&options, "ratchets")?;
        let ratchet_count = usize::try_from(ratchet_values.length())
            .map_err(|_| JsValue::from_str("persisted ratchet count is unsupported"))?;
        if ratchet_count > MAX_PERSISTED_RATCHETS {
            return Err(JsValue::from_str(
                "persisted ratchet count exceeds the runtime limit",
            ));
        }
        let mut pending_ratchets = Vec::with_capacity(ratchet_count);
        for value in ratchet_values.iter() {
            let destination = destination_hash_from_vec(required_bytes(&value, "destination")?)?;
            let sealed = bounded_persisted_region(&value, "sealed")?;
            account_persisted_bytes(&mut persisted_bytes, destination.as_bytes().len())?;
            account_persisted_bytes(&mut persisted_bytes, sealed.len())?;
            personal_rns::persistence::read_self_ratchets_snapshot(&sealed)
                .map_err(|error| persisted_state_error("ratchets", error))?;
            if pending_ratchets
                .iter()
                .any(|(existing, _)| *existing == destination)
            {
                return Err(JsValue::from_str(
                    "persisted state contains duplicate destination ratchets",
                ));
            }
            pending_ratchets.push((destination, sealed));
        }

        let now = InstantMillis(now_ms);
        let mut report = PersistenceRestoreCounts::default();
        let routes = personal_rns::persistence::read_routing_table_snapshot(&routing_table)
            .map_err(|error| persisted_state_error("routing table", error))?;
        for row in routes {
            let row = row.map_err(|error| persisted_state_error("routing table", error))?;
            report.record_route(self.engine.seed_route(&row, now));
        }
        let identities = personal_rns::persistence::read_destination_identities_snapshot(
            &destination_identities,
        )
        .map_err(|error| persisted_state_error("destination identities", error))?;
        for row in identities {
            let row =
                row.map_err(|error| persisted_state_error("destination identities", error))?;
            report.record_destination_identity(self.engine.seed_destination_identity(row, now));
        }
        let tunnel_rows = personal_rns::persistence::read_tunnels_snapshot(&tunnels)
            .map_err(|error| persisted_state_error("tunnels", error))?;
        for row in tunnel_rows {
            report.record_tunnel(self.engine.seed_tunnel(row));
        }
        report.ratchets = u32::try_from(pending_ratchets.len()).unwrap_or(u32::MAX);
        self.pending_ratchets = pending_ratchets;
        self.persistence_restored = true;
        if report.total_restored() > 0 {
            self.bump_revision();
        }
        Ok(report.into_js())
    }

    #[wasm_bindgen(js_name = snapshot)]
    pub fn snapshot(&self) -> JsValue {
        let object = Object::new();
        set_str(&object, "type", "snapshot");
        set_bigint(&object, "revision", self.revision);
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
            set_usize(
                &row,
                "transportedLinks",
                self.engine.transported_link_count_via(interface.id),
            );
            interfaces.push(&row);
        }
        set_value(&object, "interfaces", interfaces.into());
        set_u32(&object, "activeLinkCount", self.engine.link_count());
        let route_snapshots = Array::new();
        self.engine.visit_route_snapshots(
            personal_rns::interfaces::AttachedInterfaces::new(&self.interfaces),
            |route| {
                let row = Object::new();
                set_bytes(&row, "destination", route.destination.as_bytes());
                set_u32(&row, "hops", u32::from(route.hops));
                if let NextHop::Via(identity) = route.via {
                    set_bytes(&row, "viaIdentity", identity.as_bytes());
                }
                set_bytes(&row, "interfaceId", route.interface.as_bytes());
                set_u64(&row, "learnedAtMillis", route.learned_at.0);
                set_u64(
                    &row,
                    "lastRouteActivityAtMillis",
                    route.last_route_activity_at.0,
                );
                set_u64(&row, "expiresAtMillis", route.expires_at.0);
                route_snapshots.push(&row);
            },
        );
        set_value(&object, "routeSnapshots", route_snapshots.into());
        let destination_identities = Array::new();
        for identity in self.engine.destination_identities() {
            let row = Object::new();
            set_bytes(&row, "destination", identity.destination.as_bytes());
            set_bytes(&row, "identity", identity.identity.as_bytes());
            destination_identities.push(&row);
        }
        set_value(
            &object,
            "destinationIdentities",
            destination_identities.into(),
        );
        object.into()
    }
}

impl PrnsRuntime {
    fn restore_pending_ratchet(
        &mut self,
        destination: personal_rns::wire::DestinationHash,
    ) -> Result<(), JsValue> {
        let Some(index) = self
            .pending_ratchets
            .iter()
            .position(|(stored, _)| *stored == destination)
        else {
            return Ok(());
        };
        let (_, sealed) = self.pending_ratchets.remove(index);
        let record = personal_rns::persistence::read_self_ratchets_snapshot(&sealed)
            .map_err(|error| persisted_state_error("ratchets", error))?;
        let last_rotated = record.last_rotated;
        let secrets = record.secrets_newest_first().collect::<Vec<_>>();
        match self
            .engine
            .seed_self_ratchets(&destination, last_rotated, secrets.into_iter())
        {
            SeedSelfRatchetsOutcome::Seeded | SeedSelfRatchetsOutcome::AlreadyMinted => Ok(()),
            SeedSelfRatchetsOutcome::Untracked => Err(JsValue::from_str(
                "persisted ratchet destination is not tracked",
            )),
        }
    }

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
        command: PrnsCommand,
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
        self.bump_revision();
        self.apply_captured(reactions);
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    fn apply_captured(&mut self, reactions: Vec<CapturedReaction>) {
        for reaction in reactions {
            match reaction {
                CapturedReaction::Event(event) => self.events.push(event),
                CapturedReaction::Control(event) => self.control_events.push(event),
                CapturedReaction::Outbound(frame) => self.outbound.push(frame),
            }
        }
    }
}

#[derive(Default)]
struct PersistenceRestoreCounts {
    routes: u32,
    destination_identities: u32,
    tunnels: u32,
    ratchets: u32,
    refused: u32,
    dropped: u32,
}

impl PersistenceRestoreCounts {
    fn record_route(&mut self, outcome: RouteSeedOutcome) {
        match outcome {
            RouteSeedOutcome::Seeded => self.routes = self.routes.saturating_add(1),
            RouteSeedOutcome::RefusedDestinationMismatch
            | RouteSeedOutcome::RefusedBlackholedIdentity
            | RouteSeedOutcome::RefusedInvalidSignature => {
                self.refused = self.refused.saturating_add(1);
            }
            RouteSeedOutcome::AlreadyPresent
            | RouteSeedOutcome::TableFull
            | RouteSeedOutcome::AppDataArenaFull => {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
    }

    fn record_destination_identity(&mut self, outcome: DestinationIdentitySeedOutcome) {
        match outcome {
            DestinationIdentitySeedOutcome::Seeded => {
                self.destination_identities = self.destination_identities.saturating_add(1);
            }
            DestinationIdentitySeedOutcome::RefusedPublicKeyChanged => {
                self.refused = self.refused.saturating_add(1);
            }
            DestinationIdentitySeedOutcome::Replaced
            | DestinationIdentitySeedOutcome::Expired
            | DestinationIdentitySeedOutcome::CapacityExhausted => {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
    }

    fn record_tunnel(&mut self, outcome: SeedTunnelOutcome) {
        match outcome {
            SeedTunnelOutcome::Seeded => self.tunnels = self.tunnels.saturating_add(1),
            SeedTunnelOutcome::AlreadyPresent | SeedTunnelOutcome::TableFull => {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
    }

    fn total_restored(&self) -> u32 {
        self.routes
            .saturating_add(self.destination_identities)
            .saturating_add(self.tunnels)
            .saturating_add(self.ratchets)
    }

    fn into_js(self) -> JsValue {
        let object = Object::new();
        set_u32(&object, "routes", self.routes);
        set_u32(
            &object,
            "destinationIdentities",
            self.destination_identities,
        );
        set_u32(&object, "tunnels", self.tunnels);
        set_u32(&object, "ratchets", self.ratchets);
        set_u32(&object, "refused", self.refused);
        set_u32(&object, "dropped", self.dropped);
        object.into()
    }
}

fn bounded_persisted_region(options: &JsValue, field: &str) -> Result<Vec<u8>, JsValue> {
    let bytes = required_bytes(options, field)?;
    if bytes.len() > MAX_PERSISTED_STATE_BYTES {
        return Err(JsValue::from_str(
            "persisted state region exceeds the runtime limit",
        ));
    }
    Ok(bytes)
}

fn account_persisted_bytes(total: &mut usize, additional: usize) -> Result<(), JsValue> {
    *total = total
        .checked_add(additional)
        .ok_or_else(|| JsValue::from_str("persisted state size is unsupported"))?;
    if *total > MAX_PERSISTED_STATE_BYTES {
        return Err(JsValue::from_str(
            "persisted state exceeds the runtime limit",
        ));
    }
    Ok(())
}

fn validate_route_snapshot(bytes: &[u8]) -> Result<(), JsValue> {
    let rows = personal_rns::persistence::read_routing_table_snapshot(bytes)
        .map_err(|error| persisted_state_error("routing table", error))?;
    for row in rows {
        row.map_err(|error| persisted_state_error("routing table", error))?;
    }
    Ok(())
}

fn validate_destination_identity_snapshot(bytes: &[u8]) -> Result<(), JsValue> {
    let rows = personal_rns::persistence::read_destination_identities_snapshot(bytes)
        .map_err(|error| persisted_state_error("destination identities", error))?;
    for row in rows {
        row.map_err(|error| persisted_state_error("destination identities", error))?;
    }
    Ok(())
}

fn persisted_state_error(region: &str, error: impl core::fmt::Debug) -> JsValue {
    JsValue::from_str(&format!("persisted {region} is invalid: {error:?}"))
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
    Event(EventProjection),
    Control(JsValue),
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
        EngineReaction::Journaled(journaled) => match capture_journaled(journaled) {
            CapturedJournal::Event(event) => CapturedReaction::Event(event),
            CapturedJournal::Control(event) => CapturedReaction::Control(event),
        },
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
        Directive::SendIfOnline {
            target,
            bytes,
            on_send,
        } => {
            on_send();
            OutboundFrame {
                target: OutboundTarget::Interface(target),
                bytes: bytes.to_vec(),
                announce: false,
                hops: None,
            }
        }
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
