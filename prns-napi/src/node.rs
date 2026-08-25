use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use napi::bindgen_prelude::{BigInt, Buffer, Function};
use napi::threadsafe_function::ThreadsafeCallContext;
use napi::Result;
use napi_derive::napi;
use personal_rns::engine::{
    AllowRequester, AllowRequesterFailure, AllowRequesterRejection, AnnounceAppData, AnnounceNow,
    AnnounceTarget, DeliveryEvidence, DeliveryProof, EstablishLinkFailure, EstablishLinkRejection,
    IdentifyFailure, IdentifyRejection, RequestPathFailure, RequestResponseTimeout, RespondFailure,
    RespondRejection, SendRequestFailure, SendRequestRejection, SendResourceFailure,
    SendResourceRejection, SendToChannelFailure, SendToChannelRejection, SendToLinkFailure,
    SendToLinkRejection, SetResourceStrategyFailure, SetResourceStrategyRejection,
};
use personal_rns::engine::{DropRouteOutcome, RouteSnapshot};
use personal_rns::identity::{
    IdentityHash, MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
};
use personal_rns::interfaces::shared_instance as shared_instance_contract;
use personal_rns::interfaces::{
    BitrateBps, ConnectionState, InterfaceId, InterfaceSnapshot, Membership,
};
use personal_rns::node_introspection::{DestinationIdentityQuery, NodeIntrospection};
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::{
    BlackholeExpiry, BlackholeIdentityOutcome, BlackholedIdentity, NextHop,
    UnblackholeIdentityOutcome,
};
use personal_rns::runtime::request_endpoints::RespondToken;
use personal_rns::runtime::{
    DestinationIdentityRetentionControl, IdentityBlackholeControl, IdentityBlackholeSource,
    RoutingControl, RoutingControlError,
};
use personal_rns::runtime::{
    RequestOptions as EngineRequestOptions, RequestPathError, ResourceSendError, ResponseSendError,
    SegmentCompression,
};
use personal_rns::shared_instance::{SharedInstanceClient, SharedInstanceServer};
use personal_rns::units::{ByteLimit, DurationMillis, RttMillis};
use personal_rns::wifi_auto::AutoWifi;
use personal_rns::ResourceStrategy;
use personal_rns::{attach_plan_with_context, PlanOutcome, PlanRuntimeContext};
use personal_rns::{
    load_or_create_ble_identity, AnnounceNowError, AttachedInterface, AttachedSupervisor,
    AutoBluetoothLe, AutoUsb, PacketReceiptDelivered, PlanAttachments, PrnsNodeHandle,
};
use prns_host::{
    Bitrate as HostBitrate, CommandFailure as HostCommandFailure,
    CommandOutcome as HostCommandOutcome, DestinationConfig as HostDestinationConfig,
    DestinationIdentityConfig, DestinationLinkRequestPolicy, DestinationName,
    DestinationProofStrategy, DestinationRatchetPolicy, DiscoveryScope,
    HostCommand as StableHostCommand, HostConfig, HostRole, IdentityConfig, IdentitySecret,
    InterfaceConfig as StableInterfaceConfig, InterfaceMode as StableInterfaceMode,
    InterfaceRoutingPolicy as StableInterfaceRoutingPolicy, MultiRNodeMemberConfig,
    MulticastAddressType, PersistenceConfig, PrnsLimits, RNodeRadioConfig, RequestHandlerConfig,
    RequestPolicy as HostRequestPolicy, ResourceStrategy as HostResourceStrategy, SerialDataBits,
    SerialLineConfig, SerialParity, SerialStopBits, SingleDestinationConfig,
    WebSocketFramingSelection,
};
use prns_host_native::{
    CommandWait, NativeHost, NativeSnapshotError, NativeStartError, NativeSubmitError,
};

use crate::errors::{code_err, send_error, CodeResult, ErrorCode, Fallible};
use crate::events::bridge::{EventQueue, EventSink};
use crate::events::owned::OwnedEvent;
use crate::events::translate;
use crate::marshal;

#[napi(object)]
pub struct IdentitySpec {
    pub secret: Option<Buffer>,
    pub path: Option<String>,
}

#[napi(object)]
pub struct RequestPathSpec {
    pub path: String,
    #[napi(ts_type = "RequestPolicyName")]
    pub policy: Option<String>,
}

#[napi(object)]
pub struct ResourceStrategySpec {
    #[napi(ts_type = "ResourceAcceptName")]
    pub accept: String,
    /// Maximum accepted uncompressed payload size in bytes.
    pub max_uncompressed_bytes: Option<f64>,
    pub accept_compressed: Option<bool>,
}

#[napi(object)]
pub struct DestinationSpec {
    pub app_name: String,
    pub aspects: Vec<String>,
    #[napi(ts_type = "'single' | 'plain'")]
    pub kind: Option<String>,
    pub identity: Option<IdentitySpec>,
    pub use_host_identity: Option<bool>,
    pub announce_app_data: Option<Buffer>,
    pub maximum_request_bytes: Option<f64>,
    #[napi(ts_type = "ProofStrategyName")]
    pub proof: Option<String>,
    #[napi(ts_type = "LinkRequestPolicyName")]
    pub link_requests: Option<String>,
    #[napi(ts_type = "RatchetPolicyName")]
    pub ratchet: Option<String>,
    pub resource_strategy: Option<ResourceStrategySpec>,
    pub request_paths: Option<Vec<RequestPathSpec>>,
}

#[napi(object)]
pub struct NodeOptions {
    pub identity: Option<IdentitySpec>,
    #[napi(ts_type = "'endpoint' | 'transport'")]
    pub role: Option<String>,
    pub destinations: Option<Vec<DestinationSpec>>,
    pub event_queue_limit: Option<u32>,
    pub application_event_queue_limit: Option<u32>,
    pub retained_event_bytes_limit: Option<u32>,
    pub diagnostic_event_queue_limit: Option<u32>,
    pub persistence_path: Option<String>,
}

#[napi(object)]
pub struct AutoBluetoothLeOptions {
    pub identity_path: Option<String>,
    pub identity_secret: Option<Buffer>,
}

#[napi(object)]
pub struct AutoUsbOptions {
    pub baud: Option<u32>,
}

#[napi(object)]
pub struct AnnounceOptions {
    pub interface_id: Option<Buffer>,
}

#[napi(object)]
pub struct PacketReceipt {
    pub rtt_millis: f64,
    #[napi(ts_type = "DeliveryEvidenceName")]
    pub evidence: String,
    pub packet_hash: Option<Buffer>,
}

#[napi(object)]
pub struct LinkInfo {
    pub link_id: Buffer,
    pub rtt_millis: f64,
}

#[napi(object)]
pub struct PathInfo {
    pub hops: u32,
}

#[napi(object)]
pub struct RespondTokenSpec {
    pub link_id: Buffer,
    pub request_id: Buffer,
    pub rtt_millis: f64,
}

#[napi(object)]
pub struct RequestOptions {
    /// Request timeout in milliseconds.
    pub timeout_millis: Option<f64>,
    pub maximum_response_bytes: Option<f64>,
}

#[napi(object)]
pub struct RequestResult {
    pub data: Buffer,
    pub packed: Buffer,
    pub rtt_millis: f64,
}

#[napi(object)]
pub struct TcpServerOptions {
    pub bind: String,
    pub bitrate_bps: Option<f64>,
}

#[napi(object)]
pub struct TcpClientOptions {
    pub target: String,
    pub bitrate_bps: Option<f64>,
}

#[napi(object)]
pub struct UdpOptions {
    pub local: String,
    pub peer: String,
    pub bitrate_bps: Option<f64>,
}

#[napi(object)]
pub struct SerialLineSpec {
    pub baud: u32,
    pub data_bits: String,
    pub parity: String,
    pub stop_bits: String,
}

#[napi(object)]
pub struct RNodeRadioSpec {
    pub frequency_hz: f64,
    pub bandwidth_hz: u32,
    pub tx_power_dbm: i32,
    pub spreading_factor: u32,
    pub coding_rate: u32,
}

#[napi(object)]
pub struct MultiRNodeMemberSpec {
    pub name: String,
    pub virtual_port: u32,
    pub radio: RNodeRadioSpec,
    pub flow_control: bool,
    pub outgoing: bool,
}

#[napi(object)]
pub struct InterfaceConfigSpec {
    pub kind: String,
    pub group_id: Option<String>,
    pub discovery_scope: Option<String>,
    pub discovery_port: Option<u32>,
    pub data_port: Option<u32>,
    pub devices: Option<Vec<String>>,
    pub ignored_devices: Option<Vec<String>>,
    pub multicast_address_type: Option<String>,
    pub target: Option<String>,
    pub bind: Option<String>,
    pub local: Option<String>,
    pub peer: Option<String>,
    pub bitrate_bps: Option<f64>,
    pub port: Option<String>,
    pub line: Option<SerialLineSpec>,
    pub flow_control: Option<bool>,
    pub preamble_millis: Option<u32>,
    pub transmit_tail_millis: Option<u32>,
    pub persistence: Option<u32>,
    pub slot_time_millis: Option<u32>,
    pub station_callsign: Option<String>,
    pub station_interval_seconds: Option<f64>,
    pub callsign: Option<String>,
    pub ssid: Option<u32>,
    pub radio: Option<RNodeRadioSpec>,
    pub airtime_limit_short_centi_percent: Option<u32>,
    pub airtime_limit_long_centi_percent: Option<u32>,
    pub members: Option<Vec<MultiRNodeMemberSpec>>,
    pub command: Option<Vec<String>>,
    pub respawn_delay_millis: Option<f64>,
    pub peers: Option<Vec<String>>,
    pub connectable: Option<bool>,
    pub url: Option<String>,
    #[napi(ts_type = "'RawPacket' | 'Hdlc' | 'Kiss' | 'Auto'")]
    pub framing: Option<String>,
}

#[napi(object)]
pub struct InterfaceRoutingPolicySpec {
    pub mode: Option<String>,
    pub gravity: Option<f64>,
    pub recursive_path_requests: Option<bool>,
    pub announces_from_internal: Option<bool>,
    pub announces_to_internal: Option<bool>,
}

#[napi(object)]
pub struct SharedInstanceOptions {
    pub port: Option<u16>,
}

#[napi(object)]
pub struct ConfigAttachment {
    pub name: String,
    pub id: Buffer,
}

#[napi(object)]
pub struct ConfigFailure {
    pub name: String,
    pub error: String,
}

#[napi(object)]
pub struct ConfigAttachResult {
    pub attached: Vec<ConfigAttachment>,
    pub failures: Vec<ConfigFailure>,
    pub warnings: Vec<String>,
}

#[napi(object)]
pub struct SendResourceOptions {
    pub metadata: Option<Buffer>,
    #[napi(ts_type = "CompressionName")]
    pub compression: Option<String>,
    pub progress: Option<bool>,
}

#[napi(object)]
pub struct ResourceData {
    pub data: Buffer,
    pub metadata: Option<Buffer>,
    pub original_hash: Buffer,
    pub total_size_bytes: BigInt,
}

#[napi(object)]
pub struct ResourceFileReceipt {
    pub metadata: Option<Buffer>,
    pub original_hash: Buffer,
    pub total_size_bytes: BigInt,
}

#[napi(object)]
pub struct InterfaceInfo {
    pub id: Buffer,
    #[napi(ts_type = "InterfaceKindName")]
    pub kind: Option<String>,
    #[napi(ts_type = "ConnectionStateName")]
    pub connection: String,
    pub failure_reason: Option<String>,
    pub rx_bytes: BigInt,
    pub tx_bytes: BigInt,
    pub rx_bps: Option<f64>,
    pub tx_bps: Option<f64>,
    pub destinations: u32,
    pub links: u32,
    pub transported_links: u32,
    pub supervisor_id: Option<Buffer>,
}

#[napi(object)]
pub struct InterfaceInventoryInfo {
    pub name: Option<String>,
    pub origin: String,
    pub interface: InterfaceInfo,
}

#[napi(object)]
pub struct HostInterfaceSnapshotInfo {
    pub interface_id: Buffer,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub health: String,
    pub failure_detail: Option<String>,
    pub rx_bytes: BigInt,
    pub tx_bytes: BigInt,
    pub rx_bps: Option<f64>,
    pub tx_bps: Option<f64>,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
}

#[napi(object)]
pub struct HostRouteSnapshotInfo {
    pub destination: Buffer,
    pub hops: u32,
    pub via_identity: Option<Buffer>,
    pub interface_id: Buffer,
    pub learned_at_millis: f64,
    pub last_route_activity_at_millis: f64,
    pub expires_at_millis: f64,
}

#[napi(object)]
pub struct HostDestinationIdentitySnapshotInfo {
    pub destination: Buffer,
    pub identity: Buffer,
}

#[napi(object)]
pub struct HostRuntimeHealthSnapshotInfo {
    pub running: bool,
    pub uptime_millis: f64,
    pub interface_count: u32,
    pub online_interface_count: u32,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
    pub rx_bytes: BigInt,
    pub tx_bytes: BigInt,
    pub rx_bps: f64,
    pub tx_bps: f64,
}

#[napi(object)]
pub struct HostPersistenceSnapshotInfo {
    pub persistent: bool,
    pub restored: bool,
    pub last_flush_cause: Option<String>,
    pub last_failure_detail: Option<String>,
}

#[napi(object)]
pub struct HostSnapshotInfo {
    pub revision: BigInt,
    pub backend: crate::BackendInfo,
    pub interfaces: Vec<HostInterfaceSnapshotInfo>,
    pub routes: Vec<HostRouteSnapshotInfo>,
    pub active_link_count: u32,
    pub destination_identities: Vec<HostDestinationIdentitySnapshotInfo>,
    pub runtime: HostRuntimeHealthSnapshotInfo,
    pub persistence: HostPersistenceSnapshotInfo,
}

#[napi(object)]
pub struct RouteInfo {
    pub destination: Buffer,
    pub hops: u32,
    pub via: Option<Buffer>,
    pub interface_id: Buffer,
    pub learned_at_millis: f64,
    pub last_route_activity_at_millis: f64,
    pub expires_at_millis: f64,
}

#[napi(object)]
pub struct AnnounceRateInfo {
    pub destination: Buffer,
    pub last_allowed_announce_at_millis: f64,
    pub blocked_until_millis: f64,
    pub observed_at_millis: Vec<f64>,
    pub rate_violations: u32,
}

#[napi(object)]
pub struct DestinationIdentityInfo {
    pub destination: Buffer,
    pub identity: Buffer,
    pub public_key: Buffer,
}

#[napi(object)]
pub struct DestinationIdentityQuerySpec {
    pub destination: Option<Buffer>,
    pub identity: Option<Buffer>,
}

#[napi(object)]
pub struct BlackholedIdentityInfo {
    pub identity: Buffer,
    pub source: Buffer,
    pub reason: Option<String>,
    pub indefinite: bool,
}

#[napi(object)]
pub struct RetainIdentityResult {
    pub newly_retained_destination_count: u32,
    pub already_retained_destination_count: u32,
}

fn identity_config(spec: &IdentitySpec) -> CodeResult<IdentityConfig> {
    match (&spec.secret, &spec.path) {
        (Some(secret), None) => marshal::identity_secret(secret)
            .map(|secret| IdentityConfig::Existing(IdentitySecret::new(*secret))),
        (None, Some(path)) => Ok(IdentityConfig::LoadOrCreate { path: path.clone() }),
        _ => Err(code_err(
            ErrorCode::InvalidArgument,
            "identity requires exactly one of secret or path",
        )),
    }
}

fn parse_proof(value: Option<&str>) -> CodeResult<DestinationProofStrategy> {
    match value {
        None | Some("proveAll") => Ok(DestinationProofStrategy::ProveAll),
        Some("proveNone") => Ok(DestinationProofStrategy::ProveNone),
        Some("proveIf") => Ok(DestinationProofStrategy::ProveIf),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("unknown proof strategy {other:?}; expected proveAll, proveNone, or proveIf"),
        )),
    }
}

fn parse_link_requests(value: Option<&str>) -> CodeResult<DestinationLinkRequestPolicy> {
    match value {
        None | Some("acceptAll") => Ok(DestinationLinkRequestPolicy::AcceptAll),
        Some("acceptNone") => Ok(DestinationLinkRequestPolicy::AcceptNone),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("unknown link request policy {other:?}; expected acceptAll or acceptNone"),
        )),
    }
}

fn parse_ratchet(value: Option<&str>) -> CodeResult<DestinationRatchetPolicy> {
    match value {
        None | Some("noRatchets") => Ok(DestinationRatchetPolicy::NoRatchets),
        Some("ratcheted") => Ok(DestinationRatchetPolicy::Ratcheted),
        Some("ratchetsRequired") => Ok(DestinationRatchetPolicy::RatchetsRequired),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!(
                "unknown ratchet policy {other:?}; expected noRatchets, ratcheted, or ratchetsRequired"
            ),
        )),
    }
}

fn parse_request_policy(value: Option<&str>) -> CodeResult<HostRequestPolicy> {
    match value {
        None | Some("allowAll") => Ok(HostRequestPolicy::AllowAll),
        Some("allowNone") => Ok(HostRequestPolicy::AllowNone),
        Some("allowList") => Ok(HostRequestPolicy::AllowList),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("unknown request policy {other:?}; expected allowAll, allowNone, or allowList"),
        )),
    }
}

fn link_error(error: personal_rns::SendError<EstablishLinkFailure>) -> crate::errors::CodeError {
    match error {
        personal_rns::SendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        personal_rns::SendError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        personal_rns::SendError::PayloadTooLarge => {
            code_err(ErrorCode::PayloadTooLarge, "payload too large")
        }
        personal_rns::SendError::Failed(EstablishLinkFailure::Rejected(rejection)) => {
            match rejection {
                EstablishLinkRejection::NoRouteToDestination => {
                    code_err(ErrorCode::NoRouteToDestination, "no route to destination")
                }
                EstablishLinkRejection::NotDirectlyReachable => code_err(
                    ErrorCode::NotDirectlyReachable,
                    "destination is not directly reachable",
                ),
            }
        }
        personal_rns::SendError::Failed(EstablishLinkFailure::WriteFailed(error)) => {
            code_err(ErrorCode::WriteFailed, format!("{error:?}"))
        }
        personal_rns::SendError::Failed(EstablishLinkFailure::Timeout) => {
            code_err(ErrorCode::DeliveryTimedOut, "link establishment timed out")
        }
    }
}

fn path_error(error: RequestPathError) -> crate::errors::CodeError {
    match error {
        RequestPathError::EntropyUnavailable => {
            code_err(ErrorCode::EntropyUnavailable, "entropy unavailable")
        }
        RequestPathError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        RequestPathError::Failed(RequestPathFailure::WriteFailed(error)) => {
            code_err(ErrorCode::WriteFailed, format!("{error:?}"))
        }
        RequestPathError::Failed(RequestPathFailure::Timeout) => {
            code_err(ErrorCode::DeliveryTimedOut, "path discovery timed out")
        }
        RequestPathError::Failed(RequestPathFailure::Culled) => {
            code_err(ErrorCode::PacketCulled, "path request was culled")
        }
    }
}

fn identify_error(error: personal_rns::SendError<IdentifyFailure>) -> crate::errors::CodeError {
    match error {
        personal_rns::SendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        personal_rns::SendError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        personal_rns::SendError::PayloadTooLarge => {
            code_err(ErrorCode::PayloadTooLarge, "payload too large")
        }
        personal_rns::SendError::Failed(IdentifyFailure::Rejected(rejection)) => match rejection {
            IdentifyRejection::NoSuchLink => code_err(ErrorCode::UnknownLink, "unknown link"),
            IdentifyRejection::LinkNotActive => {
                code_err(ErrorCode::LinkNotActive, "link is not active")
            }
            IdentifyRejection::NotInitiator => code_err(
                ErrorCode::NotLinkInitiator,
                "host did not initiate the link",
            ),
            IdentifyRejection::IdentityNotHeld => {
                code_err(ErrorCode::IdentityNotHeld, "identity is not held")
            }
        },
        personal_rns::SendError::Failed(IdentifyFailure::WriteFailed) => {
            code_err(ErrorCode::WriteFailed, "identity write failed")
        }
    }
}

fn send_link_error(error: personal_rns::SendError<SendToLinkFailure>) -> crate::errors::CodeError {
    match error {
        personal_rns::SendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        personal_rns::SendError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        personal_rns::SendError::PayloadTooLarge => {
            code_err(ErrorCode::PayloadTooLarge, "payload too large")
        }
        personal_rns::SendError::Failed(SendToLinkFailure::Rejected(rejection)) => {
            match rejection {
                SendToLinkRejection::NoSuchLink => code_err(ErrorCode::UnknownLink, "unknown link"),
                SendToLinkRejection::LinkNotActive => {
                    code_err(ErrorCode::LinkNotActive, "link is not active")
                }
            }
        }
        personal_rns::SendError::Failed(SendToLinkFailure::WriteFailed(error)) => {
            code_err(ErrorCode::WriteFailed, format!("{error:?}"))
        }
        personal_rns::SendError::Failed(SendToLinkFailure::Culled) => {
            code_err(ErrorCode::PacketCulled, "packet was culled")
        }
        personal_rns::SendError::Failed(SendToLinkFailure::Timeout) => {
            code_err(ErrorCode::DeliveryTimedOut, "delivery timed out")
        }
    }
}

fn request_error(error: personal_rns::SendError<SendRequestFailure>) -> crate::errors::CodeError {
    match error {
        personal_rns::SendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        personal_rns::SendError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        personal_rns::SendError::PayloadTooLarge => {
            code_err(ErrorCode::PayloadTooLarge, "payload too large")
        }
        personal_rns::SendError::Failed(SendRequestFailure::Rejected(rejection)) => match rejection
        {
            SendRequestRejection::NoSuchLink => code_err(ErrorCode::UnknownLink, "unknown link"),
            SendRequestRejection::LinkNotActive => {
                code_err(ErrorCode::LinkNotActive, "link is not active")
            }
        },
        personal_rns::SendError::Failed(SendRequestFailure::WriteFailed) => {
            code_err(ErrorCode::WriteFailed, "request write failed")
        }
        personal_rns::SendError::Failed(SendRequestFailure::Culled) => {
            code_err(ErrorCode::PacketCulled, "request was culled")
        }
        personal_rns::SendError::Failed(SendRequestFailure::Timeout) => {
            code_err(ErrorCode::DeliveryTimedOut, "request timed out")
        }
        personal_rns::SendError::Failed(SendRequestFailure::ResponseTooLarge) => {
            code_err(ErrorCode::ResponseTooLarge, "response is too large")
        }
        personal_rns::SendError::Failed(SendRequestFailure::ResourceCapacity) => {
            code_err(ErrorCode::ResourceTableFull, "resource capacity exhausted")
        }
    }
}

fn allow_requester_error(
    error: personal_rns::SendError<AllowRequesterFailure>,
) -> crate::errors::CodeError {
    match error {
        personal_rns::SendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        personal_rns::SendError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        personal_rns::SendError::PayloadTooLarge => {
            code_err(ErrorCode::PayloadTooLarge, "payload too large")
        }
        personal_rns::SendError::Failed(AllowRequesterFailure::Rejected(rejection)) => {
            match rejection {
                AllowRequesterRejection::NoSuchHandler => {
                    code_err(ErrorCode::UnknownRequestHandler, "unknown request handler")
                }
                AllowRequesterRejection::NoAllowList => code_err(
                    ErrorCode::RequestPolicyNotAllowList,
                    "request handler does not use an allow list",
                ),
                AllowRequesterRejection::AllowListFull => code_err(
                    ErrorCode::RequestAllowListFull,
                    "request allow list is full",
                ),
            }
        }
    }
}

fn channel_error(error: personal_rns::SendError<SendToChannelFailure>) -> crate::errors::CodeError {
    match error {
        personal_rns::SendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        personal_rns::SendError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        personal_rns::SendError::PayloadTooLarge => {
            code_err(ErrorCode::PayloadTooLarge, "payload too large")
        }
        personal_rns::SendError::Failed(SendToChannelFailure::Rejected(rejection)) => {
            match rejection {
                SendToChannelRejection::NoSuchLink => {
                    code_err(ErrorCode::UnknownLink, "unknown link")
                }
                SendToChannelRejection::LinkNotActive => {
                    code_err(ErrorCode::LinkNotActive, "link is not active")
                }
            }
        }
        personal_rns::SendError::Failed(SendToChannelFailure::WriteFailed(error)) => {
            code_err(ErrorCode::WriteFailed, format!("{error:?}"))
        }
        personal_rns::SendError::Failed(SendToChannelFailure::WindowFull) => {
            code_err(ErrorCode::ChannelWindowFull, "channel window is full")
        }
        personal_rns::SendError::Failed(SendToChannelFailure::Untrackable) => code_err(
            ErrorCode::ChannelUntrackable,
            "channel message could not be tracked",
        ),
        personal_rns::SendError::Failed(SendToChannelFailure::Timeout) => {
            code_err(ErrorCode::DeliveryTimedOut, "channel delivery timed out")
        }
    }
}

fn send_resource_failure(failure: SendResourceFailure) -> crate::errors::CodeError {
    match failure {
        SendResourceFailure::Rejected(rejection) => match rejection {
            SendResourceRejection::NoSuchLink => {
                code_err(ErrorCode::UnknownLink, "unknown link")
            }
            SendResourceRejection::LinkNotActive => {
                code_err(ErrorCode::LinkNotActive, "link is not active")
            }
            SendResourceRejection::LinkBusy => {
                code_err(ErrorCode::LinkBusy, "link is busy")
            }
            SendResourceRejection::TableFull => {
                code_err(ErrorCode::ResourceTableFull, "resource table is full")
            }
            SendResourceRejection::Build(
                personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError::DataTooLarge,
            ) => code_err(ErrorCode::PayloadTooLarge, "resource is too large"),
            SendResourceRejection::Build(
                personal_rns::routing::links::resources::build_outgoing::BuildOutgoingResourceError::MetadataTooLarge,
            )
            | SendResourceRejection::MetadataMisplaced => code_err(
                ErrorCode::ResourceMetadataTooLarge,
                "resource metadata is too large",
            ),
            SendResourceRejection::Build(error) => {
                code_err(ErrorCode::WriteFailed, format!("{error:?}"))
            }
        },
        SendResourceFailure::WriteFailed => {
            code_err(ErrorCode::WriteFailed, "resource write failed")
        }
        SendResourceFailure::RejectedByPeer => {
            code_err(ErrorCode::ResourceRejectedByPeer, "resource rejected by peer")
        }
        SendResourceFailure::Sequencing => code_err(
            ErrorCode::ResourceSequencingFailed,
            "resource sequencing failed",
        ),
        SendResourceFailure::Timeout => {
            code_err(ErrorCode::DeliveryTimedOut, "resource delivery timed out")
        }
        SendResourceFailure::PredecessorFailed => code_err(
            ErrorCode::ResourcePredecessorFailed,
            "resource predecessor failed",
        ),
    }
}

fn resource_send_error(error: ResourceSendError) -> crate::errors::CodeError {
    match error {
        ResourceSendError::Source(error) => code_err(ErrorCode::WriteFailed, error.to_string()),
        ResourceSendError::UnrepresentableLength => code_err(
            ErrorCode::PayloadTooLarge,
            "resource length is not representable",
        ),
        ResourceSendError::Rejected(failure) => send_resource_failure(failure),
        ResourceSendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
    }
}

fn respond_failure(failure: RespondFailure) -> crate::errors::CodeError {
    match failure {
        RespondFailure::Rejected(RespondRejection::NoSuchLink) => {
            code_err(ErrorCode::UnknownLink, "unknown link")
        }
        RespondFailure::Rejected(RespondRejection::LinkNotActive) => {
            code_err(ErrorCode::LinkNotActive, "link is not active")
        }
        RespondFailure::WriteFailed => code_err(ErrorCode::WriteFailed, "response write failed"),
        RespondFailure::Resource(failure) => send_resource_failure(failure),
    }
}

fn response_send_error(error: ResponseSendError) -> crate::errors::CodeError {
    match error {
        ResponseSendError::Source(error) => code_err(ErrorCode::WriteFailed, error.to_string()),
        ResponseSendError::UnrepresentableLength => code_err(
            ErrorCode::PayloadTooLarge,
            "response length is not representable",
        ),
        ResponseSendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        ResponseSendError::CompressionTask => {
            code_err(ErrorCode::WriteFailed, "response compression failed")
        }
        ResponseSendError::Rejected(failure) => respond_failure(failure),
        ResponseSendError::UnexpectedSettlement => code_err(
            ErrorCode::WriteFailed,
            "response returned an unrelated settlement",
        ),
    }
}

fn announce_error(error: AnnounceNowError) -> crate::errors::CodeError {
    match error {
        AnnounceNowError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        AnnounceNowError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        failure @ (AnnounceNowError::Rejected(_) | AnnounceNowError::WriteFailed(_)) => {
            code_err(ErrorCode::AnnounceFailed, format!("{failure:?}"))
        }
    }
}

fn resource_strategy_error(
    error: personal_rns::SendError<SetResourceStrategyFailure>,
) -> crate::errors::CodeError {
    match error {
        personal_rns::SendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        personal_rns::SendError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        personal_rns::SendError::PayloadTooLarge => {
            code_err(ErrorCode::PayloadTooLarge, "payload too large")
        }
        personal_rns::SendError::Failed(SetResourceStrategyFailure::Rejected(
            SetResourceStrategyRejection::NoSuchLink,
        )) => code_err(ErrorCode::UnknownLink, "unknown link"),
        personal_rns::SendError::Failed(SetResourceStrategyFailure::Rejected(
            SetResourceStrategyRejection::LinkNotActive,
        )) => code_err(ErrorCode::LinkNotActive, "link is not active"),
    }
}

const DEFAULT_ACCEPT_MAX_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;

fn parse_resource_strategy(spec: &ResourceStrategySpec) -> CodeResult<ResourceStrategy> {
    match spec.accept.as_str() {
        "none" => Ok(ResourceStrategy::AcceptNone),
        "all" => {
            let max_uncompressed_bytes = match spec.max_uncompressed_bytes {
                None => DEFAULT_ACCEPT_MAX_UNCOMPRESSED_BYTES,
                Some(len) => safe_u64_argument(len, "maxUncompressedBytes")?,
            };
            Ok(ResourceStrategy::Accept {
                max_uncompressed_bytes,
                accept_compressed: spec.accept_compressed.unwrap_or(true),
            })
        }
        "if" => Ok(ResourceStrategy::AcceptIf),
        other => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("unknown resource strategy {other:?}; expected none, all, or if"),
        )),
    }
}

fn safe_u64_argument(value: f64, name: &str) -> CodeResult<u64> {
    if value.is_finite()
        && value >= 0.0
        && value.fract() == 0.0
        && value <= prns_host::SAFE_UINT_MAX as f64
    {
        Ok(value as u64)
    } else {
        Err(code_err(
            ErrorCode::InvalidArgument,
            format!("{name} must be a non-negative safe integer"),
        ))
    }
}

fn parse_compression(value: Option<&str>) -> CodeResult<SegmentCompression> {
    match value {
        None | Some("auto") => Ok(SegmentCompression::AUTO),
        Some("never") => Ok(SegmentCompression::Never),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("unknown compression {other:?}; expected auto or never"),
        )),
    }
}

fn parse_host_bitrate(value: Option<f64>) -> CodeResult<HostBitrate> {
    match value {
        None => Ok(HostBitrate::Auto),
        Some(bps) => {
            let bps = safe_u64_argument(bps, "bitrateBps")?;
            if bps < BitrateBps::MINIMUM {
                return Err(code_err(
                    ErrorCode::InvalidArgument,
                    "bitrateBps is below the protocol minimum",
                ));
            }
            Ok(HostBitrate::BitsPerSecond(bps))
        }
    }
}

fn interface_config_error(detail: impl Into<String>) -> crate::errors::CodeError {
    code_err(ErrorCode::ConfigInvalid, detail.into())
}

fn required_interface_field<T>(value: Option<T>, name: &str) -> CodeResult<T> {
    value.ok_or_else(|| interface_config_error(format!("{name} is required")))
}

fn interface_u64(value: f64, name: &str) -> CodeResult<u64> {
    if value.is_finite()
        && value >= 0.0
        && value.fract() == 0.0
        && value <= prns_host::SAFE_UINT_MAX as f64
    {
        Ok(value as u64)
    } else {
        Err(interface_config_error(format!(
            "{name} must be a non-negative safe integer"
        )))
    }
}

fn interface_i64(value: f64, name: &str) -> CodeResult<i64> {
    if value.is_finite()
        && value.fract() == 0.0
        && value >= prns_host::SAFE_INT_MIN as f64
        && value <= prns_host::SAFE_INT_MAX as f64
    {
        Ok(value as i64)
    } else {
        Err(interface_config_error(format!(
            "{name} must be a safe integer"
        )))
    }
}

fn interface_u16(value: u32, name: &str) -> CodeResult<u16> {
    u16::try_from(value)
        .map_err(|_| interface_config_error(format!("{name} exceeds the 16-bit range")))
}

fn interface_u8(value: u32, name: &str) -> CodeResult<u8> {
    u8::try_from(value)
        .map_err(|_| interface_config_error(format!("{name} exceeds the 8-bit range")))
}

fn interface_i16(value: i32, name: &str) -> CodeResult<i16> {
    i16::try_from(value)
        .map_err(|_| interface_config_error(format!("{name} exceeds the signed 16-bit range")))
}

fn serial_line(spec: SerialLineSpec) -> CodeResult<SerialLineConfig> {
    let data_bits = match spec.data_bits.as_str() {
        "Five" => SerialDataBits::Five,
        "Six" => SerialDataBits::Six,
        "Seven" => SerialDataBits::Seven,
        "Eight" => SerialDataBits::Eight,
        other => {
            return Err(interface_config_error(format!(
                "unknown serial data bits {other:?}"
            )))
        }
    };
    let parity = match spec.parity.as_str() {
        "None" => SerialParity::None,
        "Even" => SerialParity::Even,
        "Odd" => SerialParity::Odd,
        other => {
            return Err(interface_config_error(format!(
                "unknown serial parity {other:?}"
            )))
        }
    };
    let stop_bits = match spec.stop_bits.as_str() {
        "One" => SerialStopBits::One,
        "Two" => SerialStopBits::Two,
        other => {
            return Err(interface_config_error(format!(
                "unknown serial stop bits {other:?}"
            )))
        }
    };
    Ok(SerialLineConfig {
        baud: spec.baud,
        data_bits,
        parity,
        stop_bits,
    })
}

fn rnode_radio(spec: RNodeRadioSpec) -> CodeResult<RNodeRadioConfig> {
    Ok(RNodeRadioConfig {
        frequency_hz: interface_u64(spec.frequency_hz, "frequencyHz")?,
        bandwidth_hz: spec.bandwidth_hz,
        tx_power_dbm: interface_i16(spec.tx_power_dbm, "txPowerDbm")?,
        spreading_factor: interface_u8(spec.spreading_factor, "spreadingFactor")?,
        coding_rate: interface_u8(spec.coding_rate, "codingRate")?,
    })
}

fn discovery_scope(value: Option<String>) -> CodeResult<Option<DiscoveryScope>> {
    value
        .map(|value| match value.as_str() {
            "Link" => Ok(DiscoveryScope::Link),
            "Admin" => Ok(DiscoveryScope::Admin),
            "Site" => Ok(DiscoveryScope::Site),
            "Organization" => Ok(DiscoveryScope::Organization),
            "Global" => Ok(DiscoveryScope::Global),
            other => Err(interface_config_error(format!(
                "unknown discovery scope {other:?}"
            ))),
        })
        .transpose()
}

fn multicast_address_type(value: Option<String>) -> CodeResult<Option<MulticastAddressType>> {
    value
        .map(|value| match value.as_str() {
            "Temporary" => Ok(MulticastAddressType::Temporary),
            "Permanent" => Ok(MulticastAddressType::Permanent),
            other => Err(interface_config_error(format!(
                "unknown multicast address type {other:?}"
            ))),
        })
        .transpose()
}

fn websocket_framing_selection(value: Option<String>) -> CodeResult<WebSocketFramingSelection> {
    match required_interface_field(value, "framing")?.as_str() {
        "RawPacket" => Ok(WebSocketFramingSelection::RawPacket),
        "Hdlc" => Ok(WebSocketFramingSelection::Hdlc),
        "Kiss" => Ok(WebSocketFramingSelection::Kiss),
        "Auto" => Ok(WebSocketFramingSelection::Auto),
        other => Err(interface_config_error(format!(
            "unknown WebSocket framing selection {other:?}"
        ))),
    }
}

fn stable_interface_config(spec: InterfaceConfigSpec) -> CodeResult<StableInterfaceConfig> {
    let kind = spec.kind.clone();
    let config = match kind.as_str() {
        "AutoLan" => StableInterfaceConfig::AutoLan {
            group_id: spec.group_id,
            discovery_scope: discovery_scope(spec.discovery_scope)?,
            discovery_port: spec
                .discovery_port
                .map(|value| interface_u16(value, "discoveryPort"))
                .transpose()?,
            data_port: spec
                .data_port
                .map(|value| interface_u16(value, "dataPort"))
                .transpose()?,
            devices: spec.devices.unwrap_or_default(),
            ignored_devices: spec.ignored_devices.unwrap_or_default(),
            multicast_address_type: multicast_address_type(spec.multicast_address_type)?,
        },
        "TcpClient" => StableInterfaceConfig::TcpClient {
            target: required_interface_field(spec.target, "target")?,
            bitrate: parse_host_bitrate(spec.bitrate_bps)?,
        },
        "TcpServer" => StableInterfaceConfig::TcpServer {
            bind: required_interface_field(spec.bind, "bind")?,
            bitrate: parse_host_bitrate(spec.bitrate_bps)?,
        },
        "Udp" => StableInterfaceConfig::Udp {
            local: required_interface_field(spec.local, "local")?,
            peer: required_interface_field(spec.peer, "peer")?,
            bitrate: parse_host_bitrate(spec.bitrate_bps)?,
        },
        "Serial" => StableInterfaceConfig::Serial {
            port: required_interface_field(spec.port, "port")?,
            line: serial_line(required_interface_field(spec.line, "line")?)?,
        },
        "Kiss" => StableInterfaceConfig::Kiss {
            port: required_interface_field(spec.port, "port")?,
            line: serial_line(required_interface_field(spec.line, "line")?)?,
            flow_control: required_interface_field(spec.flow_control, "flowControl")?,
            preamble_millis: required_interface_field(spec.preamble_millis, "preambleMillis")?,
            transmit_tail_millis: required_interface_field(
                spec.transmit_tail_millis,
                "transmitTailMillis",
            )?,
            persistence: interface_u8(
                required_interface_field(spec.persistence, "persistence")?,
                "persistence",
            )?,
            slot_time_millis: required_interface_field(spec.slot_time_millis, "slotTimeMillis")?,
            station_callsign: spec.station_callsign,
            station_interval_seconds: spec
                .station_interval_seconds
                .map(|value| interface_u64(value, "stationIntervalSeconds"))
                .transpose()?,
        },
        "Ax25Kiss" => StableInterfaceConfig::Ax25Kiss {
            port: required_interface_field(spec.port, "port")?,
            line: serial_line(required_interface_field(spec.line, "line")?)?,
            flow_control: required_interface_field(spec.flow_control, "flowControl")?,
            preamble_millis: required_interface_field(spec.preamble_millis, "preambleMillis")?,
            transmit_tail_millis: required_interface_field(
                spec.transmit_tail_millis,
                "transmitTailMillis",
            )?,
            persistence: interface_u8(
                required_interface_field(spec.persistence, "persistence")?,
                "persistence",
            )?,
            slot_time_millis: required_interface_field(spec.slot_time_millis, "slotTimeMillis")?,
            callsign: required_interface_field(spec.callsign, "callsign")?,
            ssid: interface_u8(required_interface_field(spec.ssid, "ssid")?, "ssid")?,
        },
        "RNode" => StableInterfaceConfig::RNode {
            port: required_interface_field(spec.port, "port")?,
            radio: rnode_radio(required_interface_field(spec.radio, "radio")?)?,
            flow_control: required_interface_field(spec.flow_control, "flowControl")?,
            station_callsign: spec.station_callsign,
            station_interval_seconds: spec
                .station_interval_seconds
                .map(|value| interface_u64(value, "stationIntervalSeconds"))
                .transpose()?,
            airtime_limit_short_centi_percent: spec
                .airtime_limit_short_centi_percent
                .map(|value| interface_u16(value, "airtimeLimitShortCentiPercent"))
                .transpose()?,
            airtime_limit_long_centi_percent: spec
                .airtime_limit_long_centi_percent
                .map(|value| interface_u16(value, "airtimeLimitLongCentiPercent"))
                .transpose()?,
        },
        "MultiRNode" => StableInterfaceConfig::MultiRNode {
            port: required_interface_field(spec.port, "port")?,
            station_callsign: spec.station_callsign,
            station_interval_seconds: spec
                .station_interval_seconds
                .map(|value| interface_u64(value, "stationIntervalSeconds"))
                .transpose()?,
            members: required_interface_field(spec.members, "members")?
                .into_iter()
                .map(|member| {
                    Ok(MultiRNodeMemberConfig {
                        name: member.name,
                        virtual_port: interface_u8(member.virtual_port, "virtualPort")?,
                        radio: rnode_radio(member.radio)?,
                        flow_control: member.flow_control,
                        outgoing: member.outgoing,
                    })
                })
                .collect::<CodeResult<Vec<_>>>()?,
        },
        "Pipe" => StableInterfaceConfig::Pipe {
            command: required_interface_field(spec.command, "command")?,
            respawn_delay_millis: interface_u64(
                required_interface_field(spec.respawn_delay_millis, "respawnDelayMillis")?,
                "respawnDelayMillis",
            )?,
        },
        "BackboneClient" => StableInterfaceConfig::BackboneClient {
            target: required_interface_field(spec.target, "target")?,
            bitrate: parse_host_bitrate(spec.bitrate_bps)?,
        },
        "BackboneServer" => StableInterfaceConfig::BackboneServer {
            bind: required_interface_field(spec.bind, "bind")?,
            bitrate: parse_host_bitrate(spec.bitrate_bps)?,
        },
        "I2p" => StableInterfaceConfig::I2p {
            peers: required_interface_field(spec.peers, "peers")?,
            connectable: required_interface_field(spec.connectable, "connectable")?,
        },
        "Weave" => StableInterfaceConfig::Weave {
            port: required_interface_field(spec.port, "port")?,
        },
        "AutomaticUsb" => StableInterfaceConfig::AutomaticUsb,
        "AutomaticBluetoothLe" => StableInterfaceConfig::AutomaticBluetoothLe,
        "WebSocketClient" => StableInterfaceConfig::WebSocketClient {
            target: required_interface_field(spec.target, "target")?,
            framing: websocket_framing_selection(spec.framing)?,
        },
        "WebSocketServer" => StableInterfaceConfig::WebSocketServer {
            bind: required_interface_field(spec.bind, "bind")?,
            framing: websocket_framing_selection(spec.framing)?,
        },
        "BrowserRendezvous" => StableInterfaceConfig::BrowserRendezvous {
            url: required_interface_field(spec.url, "url")?,
        },
        other => {
            return Err(interface_config_error(format!(
                "unknown interface kind {other:?}"
            )))
        }
    };
    Ok(config)
}

fn stable_interface_routing_policy(
    spec: InterfaceRoutingPolicySpec,
) -> CodeResult<StableInterfaceRoutingPolicy> {
    let mode = spec
        .mode
        .map(|mode| match mode.as_str() {
            "Full" => Ok(StableInterfaceMode::Full),
            "PointToPoint" => Ok(StableInterfaceMode::PointToPoint),
            "AccessPoint" => Ok(StableInterfaceMode::AccessPoint),
            "Roaming" => Ok(StableInterfaceMode::Roaming),
            "Boundary" => Ok(StableInterfaceMode::Boundary),
            "Gateway" => Ok(StableInterfaceMode::Gateway),
            "Internal" => Ok(StableInterfaceMode::Internal),
            other => Err(interface_config_error(format!(
                "unknown interface mode {other:?}"
            ))),
        })
        .transpose()?;
    Ok(StableInterfaceRoutingPolicy {
        mode,
        gravity: spec
            .gravity
            .map(|gravity| interface_i64(gravity, "gravity"))
            .transpose()?,
        recursive_path_requests: spec.recursive_path_requests,
        announces_from_internal: spec.announces_from_internal,
        announces_to_internal: spec.announces_to_internal,
    })
}

fn host_resource_strategy(spec: &ResourceStrategySpec) -> CodeResult<HostResourceStrategy> {
    match parse_resource_strategy(spec)? {
        ResourceStrategy::AcceptNone => Ok(HostResourceStrategy::Refuse),
        ResourceStrategy::Accept {
            max_uncompressed_bytes,
            accept_compressed,
        } => Ok(HostResourceStrategy::Accept {
            maximum_uncompressed_bytes: max_uncompressed_bytes,
            accept_compressed,
        }),
        ResourceStrategy::AcceptIf => Err(code_err(
            ErrorCode::InvalidArgument,
            "conditional resource strategies are not configurable at startup",
        )),
    }
}

fn parse_options(options: NodeOptions, limits: PrnsLimits) -> CodeResult<HostConfig> {
    let role = match options.role.as_deref().unwrap_or("endpoint") {
        "endpoint" => HostRole::Endpoint,
        "transport" => HostRole::Transport,
        other => {
            return Err(code_err(
                ErrorCode::InvalidArgument,
                format!("unknown node role {other:?}; expected endpoint or transport"),
            ))
        }
    };
    let identity = match options.identity.as_ref() {
        Some(identity) => identity_config(identity)?,
        None => IdentityConfig::GenerateEphemeral,
    };
    let mut destinations = Vec::new();
    for spec in options.destinations.unwrap_or_default() {
        let name = DestinationName::try_new(spec.app_name, spec.aspects).map_err(|error| {
            code_err(
                ErrorCode::InvalidArgument,
                format!("invalid destination name: {error:?}"),
            )
        })?;
        let kind = spec.kind.as_deref().unwrap_or("single");
        let destination = match kind {
            "plain" => {
                if spec
                    .request_paths
                    .as_ref()
                    .is_some_and(|paths| !paths.is_empty())
                {
                    return Err(code_err(
                        ErrorCode::InvalidArgument,
                        "requestPaths require a single destination",
                    ));
                }
                if spec.use_host_identity.unwrap_or(false) || spec.identity.is_some() {
                    return Err(code_err(
                        ErrorCode::InvalidArgument,
                        "plain destinations do not have identities",
                    ));
                }
                HostDestinationConfig::Plain(name)
            }
            "single" => {
                let identity = match (spec.use_host_identity.unwrap_or(false), &spec.identity) {
                    (true, None) => DestinationIdentityConfig::HostIdentity,
                    (false, Some(identity)) => {
                        DestinationIdentityConfig::Dedicated(identity_config(identity)?)
                    }
                    (false, None) => {
                        DestinationIdentityConfig::Dedicated(IdentityConfig::GenerateEphemeral)
                    }
                    (true, Some(_)) => {
                        return Err(code_err(
                            ErrorCode::InvalidArgument,
                            "destination identity cannot be both host and dedicated",
                        ))
                    }
                };
                let mut request_handlers = Vec::new();
                for path_spec in spec.request_paths.iter().flatten() {
                    request_handlers.push(RequestHandlerConfig {
                        path: path_spec.path.clone(),
                        policy: parse_request_policy(path_spec.policy.as_deref())?,
                    });
                }
                HostDestinationConfig::Single(SingleDestinationConfig {
                    name,
                    identity,
                    announce_app_data: spec
                        .announce_app_data
                        .as_ref()
                        .map(|data| data.to_vec())
                        .unwrap_or_default(),
                    maximum_request_bytes: spec
                        .maximum_request_bytes
                        .map(|value| safe_u64_argument(value, "maximumRequestBytes"))
                        .transpose()?,
                    proof: parse_proof(spec.proof.as_deref())?,
                    link_requests: parse_link_requests(spec.link_requests.as_deref())?,
                    ratchet: parse_ratchet(spec.ratchet.as_deref())?,
                    resource_strategy: spec
                        .resource_strategy
                        .as_ref()
                        .map(host_resource_strategy)
                        .transpose()?
                        .unwrap_or(HostResourceStrategy::Refuse),
                    request_handlers,
                })
            }
            other => {
                return Err(code_err(
                    ErrorCode::InvalidArgument,
                    format!("unknown destination kind {other:?}; expected single or plain"),
                ))
            }
        };
        destinations.push(destination);
    }
    Ok(HostConfig {
        identity,
        persistence: match options.persistence_path {
            Some(path) => PersistenceConfig::Directory { path },
            None => PersistenceConfig::Ephemeral,
        },
        role,
        destinations,
        required_capabilities: Vec::new(),
        limits,
    })
}

enum Attachment {
    Interface(AttachedInterface),
    Supervisor(AttachedSupervisor),
    Host {
        host: Arc<NativeHost>,
        id: prns_host::InterfaceId,
    },
    Ble {
        handle: personal_rns::PrnsNodeHandle,
        id: InterfaceId,
    },
}

#[napi]
pub struct InterfaceHandle {
    id_bytes: [u8; 8],
    kind_name: Option<String>,
    attachment: Mutex<Option<Attachment>>,
}

impl InterfaceHandle {
    fn from_host(host: Arc<NativeHost>, id: prns_host::InterfaceId, kind: &str) -> Self {
        Self {
            id_bytes: id.into_bytes(),
            kind_name: Some(kind.to_string()),
            attachment: Mutex::new(Some(Attachment::Host { host, id })),
        }
    }

    fn from_ble(handle: personal_rns::PrnsNodeHandle, id: InterfaceId) -> Self {
        Self {
            id_bytes: *id.as_bytes(),
            kind_name: id.kind().map(|kind| kind.name().to_string()),
            attachment: Mutex::new(Some(Attachment::Ble { handle, id })),
        }
    }

    fn from_interface(attached: AttachedInterface) -> Self {
        let id = attached.id();
        Self {
            id_bytes: *id.as_bytes(),
            kind_name: id.kind().map(|kind| kind.name().to_string()),
            attachment: Mutex::new(Some(Attachment::Interface(attached))),
        }
    }

    fn from_supervisor(attached: AttachedSupervisor) -> Self {
        let id = attached.id();
        Self {
            id_bytes: *id.as_bytes(),
            kind_name: id.kind().map(|kind| kind.name().to_string()),
            attachment: Mutex::new(Some(Attachment::Supervisor(attached))),
        }
    }
}

#[napi]
impl InterfaceHandle {
    #[napi(getter)]
    pub fn id(&self) -> Buffer {
        marshal::to_buffer(&self.id_bytes)
    }

    #[napi(getter)]
    pub fn kind(&self) -> Option<String> {
        self.kind_name.clone()
    }

    #[napi]
    pub fn teardown(&self) -> bool {
        let taken = self
            .attachment
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match taken {
            Some(Attachment::Host { host, id }) => host
                .submit(StableHostCommand::DetachInterface { interface: id })
                .ok()
                .is_some_and(|command| {
                    matches!(
                        command.wait(Some(std::time::Duration::from_secs(5))),
                        CommandWait::Completed(Ok(HostCommandOutcome::InterfaceDetached { .. }))
                    )
                }),
            Some(Attachment::Interface(attached)) => {
                attached.teardown();
                true
            }
            Some(Attachment::Supervisor(attached)) => {
                attached.teardown();
                true
            }
            Some(Attachment::Ble { handle, id }) => {
                handle.remove_interface(id);
                true
            }
            None => false,
        }
    }
}

#[napi]
pub struct PrnsNode {
    host: Arc<NativeHost>,
    sink: Mutex<Option<EventSink>>,
    plan_attachments: Mutex<Vec<PlanAttachments>>,
    hashes: Vec<[u8; 16]>,
}

#[napi]
pub fn start_node(
    options: NodeOptions,
    #[napi(ts_arg_type = "(event: PrnsNodeEvent) => void")] on_event: Function<(), ()>,
) -> Result<PrnsNode, ErrorCode> {
    let balanced = PrnsLimits::balanced();
    let application_events = event_limit(
        options
            .application_event_queue_limit
            .or(options.event_queue_limit),
        balanced.application_events(),
        "applicationEventQueueLimit",
    )?;
    let retained_event_bytes = event_limit(
        options.retained_event_bytes_limit,
        balanced.retained_event_bytes(),
        "retainedEventBytesLimit",
    )?;
    let diagnostics = event_limit(
        options
            .diagnostic_event_queue_limit
            .or(options.event_queue_limit),
        balanced.diagnostics(),
        "diagnosticEventQueueLimit",
    )?;
    let limits = PrnsLimits::try_new(
        balanced.pending_commands(),
        application_events,
        retained_event_bytes,
        diagnostics,
    )
    .map_err(|error| code_err(ErrorCode::InvalidArgument, format!("{error:?}")))?;
    let config = parse_options(options, limits)?;
    let queue = EventQueue::new(limits);
    let dequeue = queue.clone();
    let tsfn = on_event
        .build_threadsafe_function::<OwnedEvent>()
        .build_callback(move |ctx: ThreadsafeCallContext<OwnedEvent>| {
            dequeue.complete(&ctx.value);
            translate::event_to_object(&ctx.env, ctx.value)
        })
        .map_err(|error| code_err(ErrorCode::Internal, format!("{error}")))?;
    let sink = EventSink::new(tsfn, queue);
    let host = NativeHost::start(config, Arc::new(sink.clone())).map_err(native_start_error)?;
    let hashes = host
        .destination_hashes()
        .iter()
        .map(|hash| hash.into_bytes())
        .collect();
    Ok(PrnsNode {
        host: Arc::new(host),
        sink: Mutex::new(Some(sink)),
        plan_attachments: Mutex::new(Vec::new()),
        hashes,
    })
}

fn event_limit(configured: Option<u32>, default: usize, name: &str) -> CodeResult<usize> {
    match configured {
        Some(0) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("{name} must be at least 1"),
        )),
        Some(value) => Ok(value as usize),
        None => Ok(default),
    }
}

fn native_start_error(error: NativeStartError) -> crate::errors::CodeError {
    let detail = format!("{error:?}");
    let code = match error {
        NativeStartError::Identity(prns_host_native::IdentityStartError::EntropyUnavailable) => {
            ErrorCode::EntropyUnavailable
        }
        NativeStartError::Identity(prns_host_native::IdentityStartError::PermissionDenied {
            ..
        })
        | NativeStartError::Persistence(
            prns_host_native::PersistenceStartError::PermissionDenied { .. },
        ) => ErrorCode::PermissionDenied,
        NativeStartError::Identity(
            prns_host_native::IdentityStartError::Malformed { .. }
            | prns_host_native::IdentityStartError::InvalidMaterial,
        ) => ErrorCode::InvalidIdentityFile,
        NativeStartError::Identity(prns_host_native::IdentityStartError::Unavailable {
            ..
        })
        | NativeStartError::Persistence(
            prns_host_native::PersistenceStartError::NotDirectory { .. }
            | prns_host_native::PersistenceStartError::Unavailable { .. },
        ) => ErrorCode::Unavailable,
        NativeStartError::TimedOut => ErrorCode::StartTimeout,
        NativeStartError::MissingCapabilities(_)
        | NativeStartError::Destination(_)
        | NativeStartError::Runtime(_)
        | NativeStartError::Thread(_) => ErrorCode::StartFailed,
    };
    code_err(code, detail)
}

fn native_submit_error(error: NativeSubmitError) -> crate::errors::CodeError {
    match error {
        NativeSubmitError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        NativeSubmitError::Stopped => code_err(ErrorCode::NodeStopped, "node stopped"),
    }
}

fn host_command_error(error: HostCommandFailure) -> crate::errors::CodeError {
    let detail = format!("{error:?}");
    let code = match error {
        HostCommandFailure::NodeStopped => ErrorCode::NodeStopped,
        HostCommandFailure::Busy => ErrorCode::Busy,
        HostCommandFailure::PayloadTooLarge => ErrorCode::PayloadTooLarge,
        HostCommandFailure::ResponseTooLarge => ErrorCode::ResponseTooLarge,
        HostCommandFailure::InvalidBitrate | HostCommandFailure::InvalidConfiguration { .. } => {
            ErrorCode::InvalidArgument
        }
        HostCommandFailure::BindFailed { .. } => ErrorCode::BindFailed,
        HostCommandFailure::WriteFailed { .. } => ErrorCode::WriteFailed,
        HostCommandFailure::PermissionDenied { .. } => ErrorCode::PermissionDenied,
        HostCommandFailure::DeviceUnavailable { .. } => ErrorCode::DeviceUnavailable,
        HostCommandFailure::ConnectFailed { .. } => ErrorCode::ConnectFailed,
        HostCommandFailure::BackendFailed { .. } => ErrorCode::BackendFailed,
        HostCommandFailure::UnsupportedByBackend => ErrorCode::Unsupported,
        HostCommandFailure::UnknownInterface => ErrorCode::UnknownInterface,
        _ => ErrorCode::Internal,
    };
    code_err(code, detail)
}

#[napi]
impl PrnsNode {
    #[napi(getter)]
    pub fn identity_hash(&self) -> Buffer {
        marshal::to_buffer(self.host.identity_hash().as_bytes())
    }

    #[napi(getter)]
    pub fn destination_hashes(&self) -> Vec<Buffer> {
        self.hashes
            .iter()
            .map(|hash| marshal::to_buffer(hash))
            .collect()
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn ready(&self) -> Result<Fallible<()>> {
        Ok(Fallible(Ok(())))
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn stop(&self) -> Result<Fallible<()>> {
        self.plan_attachments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.host.stop();
        self.sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        Ok(Fallible(Ok(())))
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn announce(
        &self,
        destination: Buffer,
        options: Option<AnnounceOptions>,
    ) -> Result<Fallible<()>> {
        Ok(Fallible(self.announce_inner(destination, options).await))
    }

    #[napi(ts_return_type = "Promise<PacketReceipt>")]
    pub async fn send_single_packet(
        &self,
        destination: Buffer,
        data: Buffer,
    ) -> Result<Fallible<PacketReceipt>> {
        Ok(Fallible(
            self.send_single_packet_inner(destination, data).await,
        ))
    }

    #[napi(ts_return_type = "Promise<PacketReceipt>")]
    pub async fn send_link_packet(
        &self,
        link_id: Buffer,
        data: Buffer,
    ) -> Result<Fallible<PacketReceipt>> {
        Ok(Fallible(self.send_link_packet_inner(link_id, data).await))
    }

    #[napi(ts_return_type = "Promise<PacketReceipt>")]
    pub async fn send_channel_message(
        &self,
        link_id: Buffer,
        message_type: u32,
        data: Buffer,
    ) -> Result<Fallible<PacketReceipt>> {
        Ok(Fallible(
            self.send_channel_message_inner(link_id, message_type, data)
                .await,
        ))
    }

    #[napi(ts_return_type = "Promise<Buffer>")]
    pub async fn establish_link(&self, destination: Buffer) -> Result<Fallible<Buffer>> {
        Ok(Fallible(
            self.establish_link_inner(destination)
                .await
                .map(|info| info.link_id),
        ))
    }

    #[napi(ts_return_type = "Promise<LinkInfo>")]
    pub async fn establish_link_with_rtt(&self, destination: Buffer) -> Result<Fallible<LinkInfo>> {
        Ok(Fallible(self.establish_link_inner(destination).await))
    }

    #[napi]
    pub fn close_link(&self, link_id: Buffer) -> Result<bool, ErrorCode> {
        let link_id = marshal::link_id(&link_id)?;
        let handle = self.handle()?;
        Ok(handle.close_link(link_id))
    }

    #[napi(ts_return_type = "Promise<PathInfo>")]
    pub async fn request_path(&self, destination: Buffer) -> Result<Fallible<PathInfo>> {
        Ok(Fallible(self.request_path_inner(destination).await))
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn identify(&self, link_id: Buffer, identity: Buffer) -> Result<Fallible<()>> {
        Ok(Fallible(self.identify_inner(link_id, identity).await))
    }

    #[napi(ts_return_type = "Promise<RequestResult>")]
    pub async fn request(
        &self,
        link_id: Buffer,
        path_hash: Buffer,
        data: Buffer,
        options: Option<RequestOptions>,
    ) -> Result<Fallible<RequestResult>> {
        Ok(Fallible(
            self.request_inner(link_id, path_hash, data, options).await,
        ))
    }

    #[napi(ts_return_type = "Promise<number>")]
    pub async fn respond(&self, token: RespondTokenSpec, data: Buffer) -> Result<Fallible<f64>> {
        Ok(Fallible(self.respond_inner(token, data).await))
    }

    #[napi(ts_return_type = "Promise<number>")]
    pub async fn respond_file(
        &self,
        token: RespondTokenSpec,
        path: String,
    ) -> Result<Fallible<f64>> {
        Ok(Fallible(self.respond_file_inner(token, path).await))
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn allow_requester(
        &self,
        destination: Buffer,
        path_hash: Buffer,
        identity: Buffer,
    ) -> Result<Fallible<()>> {
        Ok(Fallible(
            self.allow_requester_inner(destination, path_hash, identity)
                .await,
        ))
    }

    #[napi(ts_return_type = "Promise<InterfaceHandle>")]
    pub async fn attach_tcp_server(
        &self,
        options: TcpServerOptions,
    ) -> Result<Fallible<InterfaceHandle>> {
        Ok(Fallible(self.attach_tcp_server_inner(options).await))
    }

    #[napi(ts_return_type = "Promise<InterfaceHandle>")]
    pub async fn attach_tcp_client(
        &self,
        options: TcpClientOptions,
    ) -> Result<Fallible<InterfaceHandle>> {
        Ok(Fallible(self.attach_tcp_client_inner(options).await))
    }

    #[napi(ts_return_type = "Promise<InterfaceHandle>")]
    pub async fn attach_udp(&self, options: UdpOptions) -> Result<Fallible<InterfaceHandle>> {
        Ok(Fallible(self.attach_udp_inner(options).await))
    }

    #[napi(ts_return_type = "Promise<InterfaceHandle>")]
    pub async fn attach_interface(
        &self,
        config: InterfaceConfigSpec,
        routing: Option<InterfaceRoutingPolicySpec>,
    ) -> Result<Fallible<InterfaceHandle>> {
        Ok(Fallible(self.attach_interface_inner(config, routing).await))
    }

    #[napi]
    pub fn preview_validate_interface_config(
        &self,
        spec: InterfaceConfigSpec,
    ) -> Result<String, ErrorCode> {
        let config = stable_interface_config(spec)?;
        config
            .validate()
            .map_err(|error| interface_config_error(format!("{error:?}")))?;
        Ok(format!("{:?}", config.kind()))
    }

    #[napi(ts_return_type = "Promise<InterfaceHandle>")]
    pub async fn attach_shared_instance_server(
        &self,
        options: Option<SharedInstanceOptions>,
    ) -> Result<Fallible<InterfaceHandle>> {
        Ok(Fallible(
            self.attach_shared_instance_server_inner(options).await,
        ))
    }

    #[napi(ts_return_type = "Promise<InterfaceHandle>")]
    pub async fn attach_shared_instance_client(
        &self,
        options: Option<SharedInstanceOptions>,
    ) -> Result<Fallible<InterfaceHandle>> {
        Ok(Fallible(
            self.attach_shared_instance_client_inner(options).await,
        ))
    }

    #[napi(ts_return_type = "Promise<ConfigAttachResult>")]
    pub async fn attach_config(&self, config_text: String) -> Result<Fallible<ConfigAttachResult>> {
        Ok(Fallible(self.attach_config_inner(config_text).await))
    }

    #[napi]
    pub fn attach_auto_wifi(&self) -> Result<InterfaceHandle, ErrorCode> {
        let handle = self.handle()?;
        let attached = handle.supervise(AutoWifi::new());
        Ok(InterfaceHandle::from_supervisor(attached))
    }

    #[napi]
    pub fn attach_auto_usb(
        &self,
        options: Option<AutoUsbOptions>,
    ) -> Result<InterfaceHandle, ErrorCode> {
        let handle = self.handle()?;
        let mut auto = AutoUsb::default();
        if let Some(baud) = options.and_then(|opts| opts.baud) {
            auto = auto.with_baud(baud);
        }
        let attached = handle.attach(auto);
        Ok(InterfaceHandle::from_interface(attached))
    }

    #[napi]
    pub fn attach_auto_bluetooth_le(
        &self,
        options: AutoBluetoothLeOptions,
    ) -> Result<InterfaceHandle, ErrorCode> {
        self.attach_auto_bluetooth_le_inner(
            options.identity_path.as_deref(),
            options.identity_secret.as_deref(),
            "autoBluetoothLe",
        )
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn send_resource(
        &self,
        link_id: Buffer,
        data: Buffer,
        options: Option<SendResourceOptions>,
    ) -> Result<Fallible<()>> {
        Ok(Fallible(
            self.send_resource_inner(link_id, data, options).await,
        ))
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn send_resource_file(
        &self,
        link_id: Buffer,
        path: String,
        options: Option<SendResourceOptions>,
    ) -> Result<Fallible<()>> {
        Ok(Fallible(
            self.send_resource_file_inner(link_id, path, options).await,
        ))
    }

    #[napi(ts_return_type = "Promise<ResourceData>")]
    pub async fn receive_resource(&self, link_id: Buffer) -> Result<Fallible<ResourceData>> {
        Ok(Fallible(self.receive_resource_inner(link_id).await))
    }

    #[napi(ts_return_type = "Promise<ResourceFileReceipt>")]
    pub async fn receive_resource_file(
        &self,
        link_id: Buffer,
        path: String,
    ) -> Result<Fallible<ResourceFileReceipt>> {
        Ok(Fallible(
            self.receive_resource_file_inner(link_id, path).await,
        ))
    }

    #[napi(ts_return_type = "Promise<boolean>")]
    pub async fn set_resource_strategy(
        &self,
        destination: Buffer,
        strategy: ResourceStrategySpec,
    ) -> Result<Fallible<bool>> {
        Ok(Fallible(
            self.set_resource_strategy_inner(destination, strategy)
                .await,
        ))
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn set_link_resource_strategy(
        &self,
        link_id: Buffer,
        strategy: ResourceStrategySpec,
    ) -> Result<Fallible<()>> {
        Ok(Fallible(
            self.set_link_resource_strategy_inner(link_id, strategy)
                .await,
        ))
    }

    #[napi]
    pub fn interfaces(&self) -> Result<Vec<InterfaceInfo>, ErrorCode> {
        let handle = self.handle()?;
        Ok(handle.interfaces().iter().map(interface_info).collect())
    }

    #[napi]
    pub fn interface_inventory(&self) -> Result<Vec<InterfaceInventoryInfo>, ErrorCode> {
        let handle = self.handle()?;
        Ok(handle
            .interface_inventory()
            .into_iter()
            .map(|entry| InterfaceInventoryInfo {
                name: entry.name,
                origin: entry.origin.as_str().to_string(),
                interface: interface_info(&entry.snapshot),
            })
            .collect())
    }

    #[napi(ts_return_type = "Promise<HostSnapshotInfo>")]
    pub async fn host_snapshot(&self) -> Result<Fallible<HostSnapshotInfo>> {
        let host = Arc::clone(&self.host);
        let snapshot = match tokio::task::spawn_blocking(move || {
            host.snapshot(Some(std::time::Duration::from_secs(5)))
        })
        .await
        {
            Ok(snapshot) => snapshot
                .map(host_snapshot_info)
                .map_err(native_snapshot_error),
            Err(error) => Err(code_err(
                ErrorCode::Internal,
                format!("snapshot task failed: {error}"),
            )),
        };
        Ok(Fallible(snapshot))
    }

    #[napi(ts_return_type = "Promise<number>")]
    pub async fn link_count(&self) -> Result<Fallible<u32>> {
        Ok(Fallible(match self.handle() {
            Ok(handle) => Ok(NodeIntrospection::link_count(&handle).await),
            Err(error) => Err(error),
        }))
    }

    #[napi(ts_return_type = "Promise<RouteInfo[]>")]
    pub async fn routes(&self) -> Result<Fallible<Vec<RouteInfo>>> {
        Ok(Fallible(match self.handle() {
            Ok(handle) => Ok(NodeIntrospection::routes(&handle)
                .await
                .iter()
                .map(route_info)
                .collect()),
            Err(error) => Err(error),
        }))
    }

    #[napi(ts_return_type = "Promise<RouteInfo | null>")]
    pub async fn route(&self, destination: Buffer) -> Result<Fallible<Option<RouteInfo>>> {
        Ok(Fallible(self.route_inner(destination).await))
    }

    #[napi(ts_return_type = "Promise<AnnounceRateInfo[]>")]
    pub async fn announce_rates(&self) -> Result<Fallible<Vec<AnnounceRateInfo>>> {
        Ok(Fallible(match self.handle() {
            Ok(handle) => Ok(NodeIntrospection::announce_rates(&handle)
                .await
                .into_iter()
                .map(|rate| {
                    let last_allowed_announce_at_millis = rate.last_allowed_announce_at.0 as f64;
                    let blocked_until_millis = rate.blocked_until.0 as f64;
                    let observed_at_millis: Vec<f64> =
                        rate.observed_at.iter().map(|at| at.0 as f64).collect();
                    AnnounceRateInfo {
                        destination: marshal::to_buffer(rate.destination.as_bytes()),
                        last_allowed_announce_at_millis,
                        blocked_until_millis,
                        observed_at_millis,
                        rate_violations: u32::from(rate.rate_violations),
                    }
                })
                .collect()),
            Err(error) => Err(error),
        }))
    }

    #[napi(ts_return_type = "Promise<Buffer | null>")]
    pub async fn destination_identity_hash(
        &self,
        destination: Buffer,
    ) -> Result<Fallible<Option<Buffer>>> {
        Ok(Fallible(
            self.destination_identity_hash_inner(destination).await,
        ))
    }

    #[napi(ts_return_type = "Promise<DestinationIdentityInfo | null>")]
    pub async fn destination_identity(
        &self,
        query: DestinationIdentityQuerySpec,
    ) -> Result<Fallible<Option<DestinationIdentityInfo>>> {
        Ok(Fallible(self.destination_identity_inner(query).await))
    }

    #[napi(ts_return_type = "Promise<boolean>")]
    pub async fn drop_route(&self, destination: Buffer) -> Result<Fallible<bool>> {
        Ok(Fallible(self.drop_route_inner(destination).await))
    }

    #[napi(ts_return_type = "Promise<number>")]
    pub async fn drop_routes_via(&self, transport_id: Buffer) -> Result<Fallible<f64>> {
        Ok(Fallible(self.drop_routes_via_inner(transport_id).await))
    }

    #[napi(ts_return_type = "Promise<number>")]
    pub async fn clear_announce_queues(&self) -> Result<Fallible<f64>> {
        Ok(Fallible(self.clear_announce_queues_inner().await))
    }

    #[napi(ts_return_type = "Promise<BlackholeOutcomeName>")]
    pub async fn blackhole_identity(
        &self,
        identity: Buffer,
        reason: Option<String>,
    ) -> Result<Fallible<String>> {
        Ok(Fallible(
            self.blackhole_identity_inner(identity, reason).await,
        ))
    }

    #[napi(ts_return_type = "Promise<UnblackholeOutcomeName>")]
    pub async fn unblackhole_identity(&self, identity: Buffer) -> Result<Fallible<String>> {
        Ok(Fallible(self.unblackhole_identity_inner(identity).await))
    }

    #[napi(ts_return_type = "Promise<BlackholedIdentityInfo[]>")]
    pub async fn blackholed_identities(&self) -> Result<Fallible<Vec<BlackholedIdentityInfo>>> {
        Ok(Fallible(self.blackholed_identities_inner().await))
    }

    #[napi(ts_return_type = "Promise<boolean>")]
    pub async fn is_blackholed(&self, identity: Buffer) -> Result<Fallible<bool>> {
        Ok(Fallible(self.is_blackholed_inner(identity).await))
    }

    #[napi(ts_return_type = "Promise<MarkDestinationUsedOutcomeName>")]
    pub async fn mark_destination_used(&self, destination: Buffer) -> Result<Fallible<String>> {
        Ok(Fallible(
            self.mark_destination_used_inner(destination).await,
        ))
    }

    #[napi(ts_return_type = "Promise<RetainDestinationOutcomeName>")]
    pub async fn retain_destination(&self, destination: Buffer) -> Result<Fallible<String>> {
        Ok(Fallible(self.retain_destination_inner(destination).await))
    }

    #[napi(ts_return_type = "Promise<ReleaseDestinationOutcomeName>")]
    pub async fn release_destination(&self, destination: Buffer) -> Result<Fallible<String>> {
        Ok(Fallible(self.release_destination_inner(destination).await))
    }

    #[napi(ts_return_type = "Promise<RetainIdentityResult>")]
    pub async fn retain_identity(
        &self,
        identity: Buffer,
    ) -> Result<Fallible<RetainIdentityResult>> {
        Ok(Fallible(self.retain_identity_inner(identity).await))
    }
}

impl PrnsNode {
    fn handle(&self) -> CodeResult<PrnsNodeHandle> {
        self.host.preview_handle().map_err(native_submit_error)
    }

    async fn on_node_runtime<T, F, Fut>(&self, run: F) -> CodeResult<T>
    where
        T: Send + 'static,
        F: FnOnce(PrnsNodeHandle) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        self.host
            .on_preview_runtime(run)
            .await
            .map_err(native_submit_error)
    }

    async fn submit_host(&self, command: StableHostCommand) -> CodeResult<HostCommandOutcome> {
        let command = self.host.submit(command).map_err(native_submit_error)?;
        match tokio::task::spawn_blocking(move || command.wait(None)).await {
            Ok(CommandWait::Completed(result)) => result.map_err(host_command_error),
            Ok(CommandWait::TimedOut | CommandWait::Interrupted) => Err(code_err(
                ErrorCode::Internal,
                "host command wait interrupted",
            )),
            Err(error) => Err(code_err(
                ErrorCode::Internal,
                format!("host command task failed: {error}"),
            )),
        }
    }

    fn attach_auto_bluetooth_le_inner(
        &self,
        identity_path: Option<&str>,
        identity_secret: Option<&[u8]>,
        method_name: &str,
    ) -> Result<InterfaceHandle, ErrorCode> {
        let identity = match (identity_path, identity_secret) {
            (Some(path), None) => {
                load_or_create_ble_identity(Path::new(path)).map_err(|error| {
                    code_err(
                        ErrorCode::InvalidIdentityFile,
                        format!("Bluetooth LE identity file at {path}: {error:?}"),
                    )
                })?
            }
            (None, Some(secret)) => marshal::ble_identity(secret)?,
            _ => {
                return Err(code_err(
                    ErrorCode::InvalidArgument,
                    format!("{method_name} requires exactly one of identityPath or identitySecret"),
                ))
            }
        };
        let handle = self.handle()?;
        let attached = handle.attach(AutoBluetoothLe::new(identity));
        let id = attached.id();
        Ok(InterfaceHandle::from_ble(handle, id))
    }

    async fn establish_link_inner(&self, destination: Buffer) -> CodeResult<LinkInfo> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        let established = handle
            .establish_link_with_rtt(destination)
            .await
            .map_err(link_error)?;
        Ok(LinkInfo {
            link_id: marshal::to_buffer(established.link_id.as_bytes()),
            rtt_millis: established.rtt_millis as f64,
        })
    }

    async fn request_path_inner(&self, destination: Buffer) -> CodeResult<PathInfo> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        let found = handle.request_path(destination).await.map_err(path_error)?;
        Ok(PathInfo {
            hops: u32::from(found.hops.0),
        })
    }

    async fn identify_inner(&self, link_id: Buffer, identity: Buffer) -> CodeResult<()> {
        let link_id = marshal::link_id(&link_id)?;
        let identity = marshal::identity_hash(&identity)?;
        let handle = self.handle()?;
        handle
            .identify(link_id, identity)
            .await
            .map_err(identify_error)
    }

    async fn request_inner(
        &self,
        link_id: Buffer,
        path_hash: Buffer,
        data: Buffer,
        options: Option<RequestOptions>,
    ) -> CodeResult<RequestResult> {
        let link_id = marshal::link_id(&link_id)?;
        let path_hash = marshal::request_path_hash(&path_hash)?;
        let configured_timeout = options.as_ref().and_then(|value| value.timeout_millis);
        let timeout = match configured_timeout {
            Some(ms) => RequestResponseTimeout::Exact(DurationMillis(safe_u64_argument(
                ms,
                "timeoutMillis",
            )?)),
            None => RequestResponseTimeout::LinkDefault,
        };
        let maximum_response_bytes = options
            .and_then(|value| value.maximum_response_bytes)
            .map(|value| safe_u64_argument(value, "maximumResponseBytes"))
            .transpose()?
            .map(ByteLimit::Maximum)
            .unwrap_or_default();
        let handle = self.handle()?;
        let (packed, rtt) = handle
            .request_with_options(
                link_id,
                path_hash,
                &data,
                EngineRequestOptions {
                    response_timeout: timeout,
                    maximum_response_bytes,
                },
            )
            .await
            .map_err(request_error)?;
        let data = match marshal::unwrap_packed_binary(&packed) {
            Some(inner) => Buffer::from(inner.to_vec()),
            None => Buffer::from(packed.clone()),
        };
        Ok(RequestResult {
            data,
            packed: Buffer::from(packed),
            rtt_millis: rtt.millis() as f64,
        })
    }

    fn respond_token(token: &RespondTokenSpec) -> CodeResult<RespondToken> {
        Ok(RespondToken {
            link_id: marshal::link_id(&token.link_id)?,
            request_id: marshal::request_id(&token.request_id)?,
            rtt: RttMillis::new(safe_u64_argument(token.rtt_millis, "rttMillis")?),
        })
    }

    async fn respond_inner(&self, token: RespondTokenSpec, data: Buffer) -> CodeResult<f64> {
        let token = Self::respond_token(&token)?;
        let handle = self.handle()?;
        let byte_len = u64::try_from(data.len())
            .map_err(|_| code_err(ErrorCode::PayloadTooLarge, "response is too large"))?;
        handle
            .respond_bytes_streaming(token, byte_len, std::io::Cursor::new(data.to_vec()))
            .await
            .map(|rtt| rtt.millis() as f64)
            .map_err(response_send_error)
    }

    async fn respond_file_inner(&self, token: RespondTokenSpec, path: String) -> CodeResult<f64> {
        let token = Self::respond_token(&token)?;
        let handle = self.handle()?;
        let file = tokio::fs::File::open(&path).await.map_err(|error| {
            code_err(
                ErrorCode::InvalidArgument,
                format!("could not open {path}: {error}"),
            )
        })?;
        let byte_len = file
            .metadata()
            .await
            .map_err(|error| {
                code_err(
                    ErrorCode::InvalidArgument,
                    format!("could not stat {path}: {error}"),
                )
            })?
            .len();
        handle
            .respond_bytes_streaming(token, byte_len, file)
            .await
            .map(|rtt| rtt.millis() as f64)
            .map_err(|error| code_err(ErrorCode::RespondFailed, format!("{error}")))
    }

    async fn allow_requester_inner(
        &self,
        destination: Buffer,
        path_hash: Buffer,
        identity: Buffer,
    ) -> CodeResult<()> {
        let allow = AllowRequester {
            destination: marshal::destination_hash(&destination)?,
            path_hash: marshal::request_path_hash(&path_hash)?,
            identity: marshal::identity_hash(&identity)?,
        };
        let handle = self.handle()?;
        handle
            .allow_requester(allow)
            .await
            .map_err(allow_requester_error)
    }

    async fn announce_inner(
        &self,
        destination: Buffer,
        options: Option<AnnounceOptions>,
    ) -> CodeResult<()> {
        let destination = marshal::destination_hash(&destination)?;
        let target = match options.as_ref().and_then(|opts| opts.interface_id.as_ref()) {
            Some(id) => AnnounceTarget::Interface(marshal::interface_id(id)?),
            None => AnnounceTarget::AllInterfaces,
        };
        let handle = self.handle()?;
        handle
            .announce_now(AnnounceNow {
                destination,
                target,
                app_data: AnnounceAppData::Registered,
            })
            .await
            .map_err(announce_error)
    }

    async fn send_single_packet_inner(
        &self,
        destination: Buffer,
        data: Buffer,
    ) -> CodeResult<PacketReceipt> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        let receipt = handle
            .send_single_packet(destination, &data)
            .await
            .map_err(|error| send_error(ErrorCode::SendFailed, error))?;
        Ok(packet_receipt(receipt))
    }

    async fn send_link_packet_inner(
        &self,
        link_id: Buffer,
        data: Buffer,
    ) -> CodeResult<PacketReceipt> {
        let link_id = marshal::link_id(&link_id)?;
        let handle = self.handle()?;
        let receipt = handle
            .send_link_packet(link_id, &data)
            .await
            .map_err(send_link_error)?;
        Ok(packet_receipt(receipt))
    }

    async fn send_channel_message_inner(
        &self,
        link_id: Buffer,
        message_type: u32,
        data: Buffer,
    ) -> CodeResult<PacketReceipt> {
        let message_type = u16::try_from(message_type)
            .ok()
            .map(MessageType)
            .filter(|kind| !kind.is_system_reserved())
            .ok_or_else(|| {
                code_err(
                    ErrorCode::InvalidChannelMessageType,
                    "messageType must be an application message type",
                )
            })?;
        let link_id = marshal::link_id(&link_id)?;
        let handle = self.handle()?;
        let receipt = handle
            .send_channel_message(link_id, message_type, &data)
            .await
            .map_err(channel_error)?;
        Ok(packet_receipt(receipt))
    }

    async fn attach_tcp_server_inner(
        &self,
        options: TcpServerOptions,
    ) -> CodeResult<InterfaceHandle> {
        let outcome = self
            .submit_host(StableHostCommand::AttachTcpServer {
                bind: options.bind,
                bitrate: parse_host_bitrate(options.bitrate_bps)?,
            })
            .await?;
        match outcome {
            HostCommandOutcome::InterfaceAttached { interface } => Ok(InterfaceHandle::from_host(
                Arc::clone(&self.host),
                interface,
                "tcp-server",
            )),
            other => Err(code_err(
                ErrorCode::Internal,
                format!("unexpected attach outcome: {other:?}"),
            )),
        }
    }

    async fn attach_tcp_client_inner(
        &self,
        options: TcpClientOptions,
    ) -> CodeResult<InterfaceHandle> {
        let outcome = self
            .submit_host(StableHostCommand::AttachTcpClient {
                target: options.target,
                bitrate: parse_host_bitrate(options.bitrate_bps)?,
            })
            .await?;
        match outcome {
            HostCommandOutcome::InterfaceAttached { interface } => Ok(InterfaceHandle::from_host(
                Arc::clone(&self.host),
                interface,
                "tcp-client",
            )),
            other => Err(code_err(
                ErrorCode::Internal,
                format!("unexpected attach outcome: {other:?}"),
            )),
        }
    }

    async fn attach_udp_inner(&self, options: UdpOptions) -> CodeResult<InterfaceHandle> {
        let outcome = self
            .submit_host(StableHostCommand::AttachUdp {
                local: options.local,
                peer: options.peer,
                bitrate: parse_host_bitrate(options.bitrate_bps)?,
            })
            .await?;
        match outcome {
            HostCommandOutcome::InterfaceAttached { interface } => Ok(InterfaceHandle::from_host(
                Arc::clone(&self.host),
                interface,
                "udp",
            )),
            other => Err(code_err(
                ErrorCode::Internal,
                format!("unexpected attach outcome: {other:?}"),
            )),
        }
    }

    async fn attach_interface_inner(
        &self,
        spec: InterfaceConfigSpec,
        routing: Option<InterfaceRoutingPolicySpec>,
    ) -> CodeResult<InterfaceHandle> {
        let config = stable_interface_config(spec)?;
        let routing = routing.map(stable_interface_routing_policy).transpose()?;
        let kind = format!("{:?}", config.kind());
        let outcome = self
            .submit_host(StableHostCommand::AttachInterface { config, routing })
            .await?;
        match outcome {
            HostCommandOutcome::InterfaceAttached { interface } => Ok(InterfaceHandle::from_host(
                Arc::clone(&self.host),
                interface,
                &kind,
            )),
            other => Err(code_err(
                ErrorCode::Internal,
                format!("unexpected attach outcome: {other:?}"),
            )),
        }
    }

    async fn attach_shared_instance_server_inner(
        &self,
        options: Option<SharedInstanceOptions>,
    ) -> CodeResult<InterfaceHandle> {
        let port = options.and_then(|opts| opts.port);
        let attached = self
            .on_node_runtime(move |handle| async move {
                let server = match port {
                    Some(port) => SharedInstanceServer::with_port(port),
                    None => SharedInstanceServer::new(),
                };
                server.bind().await.map(|bound| handle.supervise(bound))
            })
            .await?
            .map_err(|error| {
                code_err(
                    ErrorCode::AttachFailed,
                    format!("shared instance bind failed: {error:?}"),
                )
            })?;
        Ok(InterfaceHandle::from_supervisor(attached))
    }

    async fn attach_shared_instance_client_inner(
        &self,
        options: Option<SharedInstanceOptions>,
    ) -> CodeResult<InterfaceHandle> {
        let port = options
            .and_then(|opts| opts.port)
            .unwrap_or(shared_instance_contract::DEFAULT_LOCAL_PORT);
        let target = format!("127.0.0.1:{port}");
        let attached = self
            .on_node_runtime(move |handle| async move {
                tokio::net::TcpStream::connect(target.as_str())
                    .await
                    .map(|stream| {
                        let client = SharedInstanceClient::new(target.clone().into_bytes(), stream);
                        handle.add_interface(client)
                    })
            })
            .await?
            .map_err(|error| {
                code_err(
                    ErrorCode::AttachFailed,
                    format!("shared instance connect failed: {error}"),
                )
            })?;
        Ok(InterfaceHandle::from_interface(attached))
    }

    fn spawn_progress_forwarder(
        &self,
        link_id: [u8; 16],
    ) -> CodeResult<tokio::sync::mpsc::UnboundedSender<personal_rns::runtime::ResourceProgress>>
    {
        let sink = self
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| code_err(ErrorCode::NodeStopped, "node stopped"))?;
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(progress) = progress_rx.recv().await {
                let personal_rns::runtime::ResourceProgress {
                    transferred_bytes,
                    total_bytes,
                    physical_transferred_bytes,
                    segment_index,
                    total_segments,
                } = progress;
                sink.emit(OwnedEvent::ResourceSendProgress {
                    link_id,
                    transferred_bytes,
                    total_bytes,
                    physical_transferred_bytes,
                    segment_index,
                    total_segments,
                });
            }
        });
        Ok(progress_tx)
    }

    async fn send_resource_inner(
        &self,
        link_id: Buffer,
        data: Buffer,
        options: Option<SendResourceOptions>,
    ) -> CodeResult<()> {
        let link = marshal::link_id(&link_id)?;
        let handle = self.handle()?;
        let options = options.unwrap_or(SendResourceOptions {
            metadata: None,
            compression: None,
            progress: None,
        });
        let compression = parse_compression(options.compression.as_deref())?;
        let metadata = options.metadata.as_ref().map(|m| m.to_vec());
        let total_len = data.len() as u64;
        let source = std::io::Cursor::new(data.to_vec());
        let result = if options.progress.unwrap_or(false) {
            let progress = self.spawn_progress_forwarder(*link.as_bytes())?;
            handle
                .send_resource_with_options(
                    link,
                    total_len,
                    source,
                    metadata.as_deref().unwrap_or_default(),
                    compression,
                    progress,
                )
                .await
        } else {
            match metadata {
                Some(metadata) => {
                    handle
                        .send_resource_with_metadata(link, total_len, source, &metadata)
                        .await
                }
                None => {
                    handle
                        .send_resource_with_compression(link, total_len, source, compression)
                        .await
                }
            }
        };
        result.map_err(resource_send_error)
    }

    async fn send_resource_file_inner(
        &self,
        link_id: Buffer,
        path: String,
        options: Option<SendResourceOptions>,
    ) -> CodeResult<()> {
        let link = marshal::link_id(&link_id)?;
        let handle = self.handle()?;
        let options = options.unwrap_or(SendResourceOptions {
            metadata: None,
            compression: None,
            progress: None,
        });
        let compression = parse_compression(options.compression.as_deref())?;
        let metadata = options.metadata.as_ref().map(|m| m.to_vec());
        let file = tokio::fs::File::open(&path).await.map_err(|error| {
            code_err(
                ErrorCode::InvalidArgument,
                format!("could not open {path}: {error}"),
            )
        })?;
        let total_len = file
            .metadata()
            .await
            .map_err(|error| {
                code_err(
                    ErrorCode::InvalidArgument,
                    format!("could not stat {path}: {error}"),
                )
            })?
            .len();
        let result = if options.progress.unwrap_or(false) {
            let progress = self.spawn_progress_forwarder(*link.as_bytes())?;
            handle
                .send_resource_with_options(
                    link,
                    total_len,
                    file,
                    metadata.as_deref().unwrap_or_default(),
                    compression,
                    progress,
                )
                .await
        } else {
            match metadata {
                Some(metadata) => {
                    handle
                        .send_resource_with_metadata(link, total_len, file, &metadata)
                        .await
                }
                None => {
                    handle
                        .send_resource_with_compression(link, total_len, file, compression)
                        .await
                }
            }
        };
        result.map_err(resource_send_error)
    }

    async fn receive_resource_inner(&self, link_id: Buffer) -> CodeResult<ResourceData> {
        let link = marshal::link_id(&link_id)?;
        let handle = self.handle()?;
        let mut collected: Vec<u8> = Vec::new();
        let receipt = handle
            .receive_resource(link, &mut collected)
            .await
            .map_err(|error| code_err(ErrorCode::ResourceReceiveFailed, format!("{error:?}")))?;
        Ok(ResourceData {
            data: Buffer::from(collected),
            metadata: receipt.metadata.map(Buffer::from),
            original_hash: marshal::to_buffer(receipt.original_hash.as_bytes()),
            total_size_bytes: BigInt::from(receipt.total_size_bytes),
        })
    }

    async fn receive_resource_file_inner(
        &self,
        link_id: Buffer,
        path: String,
    ) -> CodeResult<ResourceFileReceipt> {
        let link = marshal::link_id(&link_id)?;
        let handle = self.handle()?;
        let file = tokio::fs::File::create(&path).await.map_err(|error| {
            code_err(
                ErrorCode::InvalidArgument,
                format!("could not create {path}: {error}"),
            )
        })?;
        let receipt = handle
            .receive_resource(link, file)
            .await
            .map_err(|error| code_err(ErrorCode::ResourceReceiveFailed, format!("{error:?}")))?;
        Ok(ResourceFileReceipt {
            metadata: receipt.metadata.map(Buffer::from),
            original_hash: marshal::to_buffer(receipt.original_hash.as_bytes()),
            total_size_bytes: BigInt::from(receipt.total_size_bytes),
        })
    }

    async fn set_resource_strategy_inner(
        &self,
        destination: Buffer,
        strategy: ResourceStrategySpec,
    ) -> CodeResult<bool> {
        let destination = marshal::destination_hash(&destination)?;
        let strategy = parse_resource_strategy(&strategy)?;
        let handle = self.handle()?;
        Ok(handle.set_resource_strategy(destination, strategy).await)
    }

    async fn set_link_resource_strategy_inner(
        &self,
        link_id: Buffer,
        strategy: ResourceStrategySpec,
    ) -> CodeResult<()> {
        let link = marshal::link_id(&link_id)?;
        let strategy = parse_resource_strategy(&strategy)?;
        let handle = self.handle()?;
        handle
            .set_link_resource_strategy(link, strategy)
            .await
            .map_err(resource_strategy_error)
    }

    async fn route_inner(&self, destination: Buffer) -> CodeResult<Option<RouteInfo>> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        Ok(NodeIntrospection::route(&handle, destination)
            .await
            .as_ref()
            .map(route_info))
    }

    async fn destination_identity_hash_inner(
        &self,
        destination: Buffer,
    ) -> CodeResult<Option<Buffer>> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        Ok(handle
            .destination_identity_hash(destination)
            .await
            .map(|identity| marshal::to_buffer(identity.as_bytes())))
    }

    async fn destination_identity_inner(
        &self,
        query: DestinationIdentityQuerySpec,
    ) -> CodeResult<Option<DestinationIdentityInfo>> {
        let query = match (&query.destination, &query.identity) {
            (Some(destination), None) => {
                DestinationIdentityQuery::Destination(marshal::destination_hash(destination)?)
            }
            (None, Some(identity)) => {
                DestinationIdentityQuery::Identity(marshal::identity_hash(identity)?)
            }
            _ => {
                return Err(code_err(
                    ErrorCode::InvalidArgument,
                    "query requires exactly one of destination or identity",
                ))
            }
        };
        let handle = self.handle()?;
        Ok(handle
            .destination_identity(query)
            .await
            .map(|snapshot| DestinationIdentityInfo {
                destination: marshal::to_buffer(snapshot.destination.as_bytes()),
                identity: marshal::to_buffer(snapshot.identity.as_bytes()),
                public_key: marshal::to_buffer(snapshot.public.as_bytes()),
            }))
    }

    async fn drop_route_inner(&self, destination: Buffer) -> CodeResult<bool> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        RoutingControl::drop_route(&handle, destination)
            .await
            .map(|outcome| matches!(outcome, DropRouteOutcome::Dropped))
            .map_err(routing_error)
    }

    async fn drop_routes_via_inner(&self, transport_id: Buffer) -> CodeResult<f64> {
        let transport = marshal::transport_id(&transport_id)?;
        let handle = self.handle()?;
        RoutingControl::drop_routes_via(&handle, transport)
            .await
            .map(|outcome| f64::from(outcome.dropped_routes))
            .map_err(routing_error)
    }

    async fn clear_announce_queues_inner(&self) -> CodeResult<f64> {
        let handle = self.handle()?;
        RoutingControl::clear_announce_queues(&handle)
            .await
            .map(|outcome| f64::from(outcome.dropped_announces))
            .map_err(routing_error)
    }

    async fn blackhole_identity_inner(
        &self,
        identity: Buffer,
        reason: Option<String>,
    ) -> CodeResult<String> {
        let identity = marshal::identity_hash(&identity)?;
        let handle = self.handle()?;
        let entry = BlackholedIdentity {
            identity,
            source: IdentityHash::new([0u8; 16]),
            expiry: BlackholeExpiry::Indefinite,
            reason: reason.as_deref(),
        };
        IdentityBlackholeControl::blackhole_identity(&handle, entry)
            .await
            .map(|outcome| {
                match outcome {
                    BlackholeIdentityOutcome::Added => "added",
                    BlackholeIdentityOutcome::AlreadyPresent => "alreadyPresent",
                }
                .to_string()
            })
            .map_err(|error| code_err(ErrorCode::BlackholeFailed, format!("{error:?}")))
    }

    async fn unblackhole_identity_inner(&self, identity: Buffer) -> CodeResult<String> {
        let identity = marshal::identity_hash(&identity)?;
        let handle = self.handle()?;
        IdentityBlackholeControl::unblackhole_identity(&handle, identity)
            .await
            .map(|outcome| {
                match outcome {
                    UnblackholeIdentityOutcome::Removed => "removed",
                    UnblackholeIdentityOutcome::NotFound => "notFound",
                }
                .to_string()
            })
            .map_err(|error| code_err(ErrorCode::BlackholeFailed, format!("{error:?}")))
    }

    async fn blackholed_identities_inner(&self) -> CodeResult<Vec<BlackholedIdentityInfo>> {
        let handle = self.handle()?;
        IdentityBlackholeSource::blackholed_identities(&handle)
            .await
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| BlackholedIdentityInfo {
                        identity: marshal::to_buffer(entry.identity.as_bytes()),
                        source: marshal::to_buffer(entry.source.as_bytes()),
                        reason: entry.reason,
                        indefinite: matches!(entry.expiry, BlackholeExpiry::Indefinite),
                    })
                    .collect()
            })
            .map_err(|error| code_err(ErrorCode::BlackholeFailed, format!("{error:?}")))
    }

    async fn is_blackholed_inner(&self, identity: Buffer) -> CodeResult<bool> {
        let identity = marshal::identity_hash(&identity)?;
        let handle = self.handle()?;
        IdentityBlackholeSource::is_blackholed(&handle, identity)
            .await
            .map_err(|error| code_err(ErrorCode::BlackholeFailed, format!("{error:?}")))
    }

    async fn mark_destination_used_inner(&self, destination: Buffer) -> CodeResult<String> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        DestinationIdentityRetentionControl::mark_destination_used(&handle, destination)
            .await
            .map(|outcome| {
                match outcome {
                    MarkDestinationUsedOutcome::Recorded => "recorded",
                    MarkDestinationUsedOutcome::Refreshed => "refreshed",
                    MarkDestinationUsedOutcome::Retained => "retained",
                    MarkDestinationUsedOutcome::NotFound => "notFound",
                }
                .to_string()
            })
            .map_err(|error| code_err(ErrorCode::RetentionFailed, format!("{error:?}")))
    }

    async fn retain_destination_inner(&self, destination: Buffer) -> CodeResult<String> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        DestinationIdentityRetentionControl::retain_destination(&handle, destination)
            .await
            .map(|outcome| {
                match outcome {
                    RetainDestinationOutcome::Retained => "retained",
                    RetainDestinationOutcome::AlreadyRetained => "alreadyRetained",
                    RetainDestinationOutcome::NotFound => "notFound",
                }
                .to_string()
            })
            .map_err(|error| code_err(ErrorCode::RetentionFailed, format!("{error:?}")))
    }

    async fn release_destination_inner(&self, destination: Buffer) -> CodeResult<String> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.handle()?;
        DestinationIdentityRetentionControl::release_destination(&handle, destination)
            .await
            .map(|outcome| {
                match outcome {
                    ReleaseDestinationOutcome::Released => "released",
                    ReleaseDestinationOutcome::UseRecorded => "useRecorded",
                    ReleaseDestinationOutcome::UseRefreshed => "useRefreshed",
                    ReleaseDestinationOutcome::NotFound => "notFound",
                }
                .to_string()
            })
            .map_err(|error| code_err(ErrorCode::RetentionFailed, format!("{error:?}")))
    }

    async fn retain_identity_inner(&self, identity: Buffer) -> CodeResult<RetainIdentityResult> {
        let identity = marshal::identity_hash(&identity)?;
        let handle = self.handle()?;
        DestinationIdentityRetentionControl::retain_identity(&handle, identity)
            .await
            .map(|outcome| RetainIdentityResult {
                newly_retained_destination_count: outcome.newly_retained_destination_count,
                already_retained_destination_count: outcome.already_retained_destination_count,
            })
            .map_err(|error| code_err(ErrorCode::RetentionFailed, format!("{error:?}")))
    }

    async fn attach_config_inner(&self, config_text: String) -> CodeResult<ConfigAttachResult> {
        let report = personal_rns::config::parse_and_plan(&config_text)
            .map_err(|errors| code_err(ErrorCode::ConfigInvalid, format!("{errors:?}")))?;
        let warnings = report
            .warnings
            .iter()
            .map(|warning| format!("{warning:?}"))
            .collect();
        let plan = report.value;
        let (attachments, attached, failures) = self
            .on_node_runtime(move |handle| async move {
                let mut attached = Vec::new();
                let mut failures = Vec::new();
                let plan_attachments = attach_plan_with_context(
                    &handle,
                    &plan,
                    &PlanRuntimeContext::default(),
                    &mut |outcome| match outcome {
                        PlanOutcome::Up { interface, id } => {
                            attached.push((interface.name.clone(), *id.as_bytes()));
                        }
                        PlanOutcome::Failed { interface, error } => {
                            failures.push((interface.name.clone(), format!("{error}")));
                        }
                    },
                )
                .await;
                (plan_attachments, attached, failures)
            })
            .await?;
        self.plan_attachments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(attachments);
        Ok(ConfigAttachResult {
            attached: attached
                .into_iter()
                .map(|(name, id)| ConfigAttachment {
                    name,
                    id: marshal::to_buffer(&id),
                })
                .collect(),
            failures: failures
                .into_iter()
                .map(|(name, error)| ConfigFailure { name, error })
                .collect(),
            warnings,
        })
    }
}

fn connection_name(connection: ConnectionState) -> &'static str {
    match connection {
        ConnectionState::Initializing => "initializing",
        ConnectionState::Connected => "connected",
        ConnectionState::Degraded => "degraded",
        ConnectionState::Reconnecting => "reconnecting",
        ConnectionState::Failed => "failed",
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Disabled => "disabled",
        ConnectionState::Unknown => "unknown",
    }
}

fn host_snapshot_info(snapshot: prns_host::HostSnapshot) -> HostSnapshotInfo {
    let backend = crate::BackendInfo {
        backend: snapshot.backend.backend().contract_name().to_string(),
        capabilities: snapshot
            .backend
            .capabilities()
            .map(|capability| capability.contract_name().to_string())
            .collect(),
        interface_kinds: snapshot
            .backend
            .interface_kinds()
            .map(|kind| kind.contract_name().to_string())
            .collect(),
    };
    let interfaces = snapshot
        .interfaces
        .into_iter()
        .map(|interface| HostInterfaceSnapshotInfo {
            interface_id: marshal::to_buffer(interface.interface_id.as_bytes()),
            name: interface.name,
            kind: interface.kind.map(|kind| kind.contract_name().to_string()),
            health: interface.health.contract_name().to_string(),
            failure_detail: interface.failure_detail,
            rx_bytes: BigInt::from(interface.rx_bytes),
            tx_bytes: BigInt::from(interface.tx_bytes),
            rx_bps: interface.rx_bps.map(|value| value as f64),
            tx_bps: interface.tx_bps.map(|value| value as f64),
            route_count: interface.route_count,
            link_count: interface.link_count,
            transported_link_count: interface.transported_link_count,
        })
        .collect();
    let routes = snapshot
        .routes
        .into_iter()
        .map(|route| HostRouteSnapshotInfo {
            destination: marshal::to_buffer(route.destination.as_bytes()),
            hops: u32::from(route.hops),
            via_identity: route
                .via_identity
                .map(|identity| marshal::to_buffer(identity.as_bytes())),
            interface_id: marshal::to_buffer(route.interface_id.as_bytes()),
            learned_at_millis: route.learned_at_millis as f64,
            last_route_activity_at_millis: route.last_route_activity_at_millis as f64,
            expires_at_millis: route.expires_at_millis as f64,
        })
        .collect();
    let destination_identities = snapshot
        .destination_identities
        .into_iter()
        .map(|identity| HostDestinationIdentitySnapshotInfo {
            destination: marshal::to_buffer(identity.destination.as_bytes()),
            identity: marshal::to_buffer(identity.identity.as_bytes()),
        })
        .collect();
    HostSnapshotInfo {
        revision: BigInt::from(snapshot.revision),
        backend,
        interfaces,
        routes,
        active_link_count: snapshot.active_link_count,
        destination_identities,
        runtime: HostRuntimeHealthSnapshotInfo {
            running: snapshot.runtime.running,
            uptime_millis: snapshot.runtime.uptime_millis as f64,
            interface_count: snapshot.runtime.interface_count,
            online_interface_count: snapshot.runtime.online_interface_count,
            route_count: snapshot.runtime.route_count,
            link_count: snapshot.runtime.link_count,
            transported_link_count: snapshot.runtime.transported_link_count,
            rx_bytes: BigInt::from(snapshot.runtime.rx_bytes),
            tx_bytes: BigInt::from(snapshot.runtime.tx_bytes),
            rx_bps: snapshot.runtime.rx_bps as f64,
            tx_bps: snapshot.runtime.tx_bps as f64,
        },
        persistence: HostPersistenceSnapshotInfo {
            persistent: snapshot.persistence.persistent,
            restored: snapshot.persistence.restored,
            last_flush_cause: snapshot
                .persistence
                .last_flush_cause
                .map(|cause| cause.contract_name().to_string()),
            last_failure_detail: snapshot.persistence.last_failure_detail,
        },
    }
}

fn native_snapshot_error(error: NativeSnapshotError) -> crate::errors::CodeError {
    match error {
        NativeSnapshotError::Busy => code_err(ErrorCode::Busy, "snapshot queue is busy"),
        NativeSnapshotError::Stopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        NativeSnapshotError::TimedOut => code_err(ErrorCode::Unavailable, "snapshot timed out"),
    }
}

fn interface_info(snapshot: &InterfaceSnapshot) -> InterfaceInfo {
    InterfaceInfo {
        id: marshal::to_buffer(snapshot.id.as_bytes()),
        kind: snapshot.id.kind().map(|kind| kind.name().to_string()),
        connection: connection_name(snapshot.connection).to_string(),
        failure_reason: snapshot.failure_reason.map(str::to_string),
        rx_bytes: BigInt::from(snapshot.rx_bytes),
        tx_bytes: BigInt::from(snapshot.tx_bytes),
        rx_bps: snapshot.transfer_rates.map(|rates| f64::from(rates.rx_bps)),
        tx_bps: snapshot.transfer_rates.map(|rates| f64::from(rates.tx_bps)),
        destinations: snapshot.destinations,
        links: snapshot.links,
        transported_links: snapshot.transported_links,
        supervisor_id: match snapshot.membership {
            Membership::Independent => None,
            Membership::FleetMember { supervisor_id } => {
                Some(marshal::to_buffer(supervisor_id.as_bytes()))
            }
        },
    }
}

fn route_info(route: &RouteSnapshot) -> RouteInfo {
    let learned_at_millis = route.learned_at.0 as f64;
    let last_route_activity_at_millis = route.last_route_activity_at.0 as f64;
    let expires_at_millis = route.expires_at.0 as f64;
    RouteInfo {
        destination: marshal::to_buffer(route.destination.as_bytes()),
        hops: u32::from(route.hops),
        via: match route.via {
            NextHop::Direct => None,
            NextHop::Via(transport) => Some(marshal::to_buffer(transport.as_bytes())),
        },
        interface_id: marshal::to_buffer(route.interface.as_bytes()),
        learned_at_millis,
        last_route_activity_at_millis,
        expires_at_millis,
    }
}

fn routing_error(error: RoutingControlError) -> crate::errors::CodeError {
    match error {
        RoutingControlError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        other => code_err(ErrorCode::RoutingControlFailed, format!("{other:?}")),
    }
}

fn packet_receipt(receipt: PacketReceiptDelivered) -> PacketReceipt {
    let (evidence, packet_hash) = match receipt.evidence {
        DeliveryEvidence::Proof(DeliveryProof::Explicit(hash)) => {
            ("proofExplicit", Some(marshal::to_buffer(hash.as_bytes())))
        }
        DeliveryEvidence::Proof(DeliveryProof::Implicit(hash)) => {
            ("proofImplicit", Some(marshal::to_buffer(hash.as_bytes())))
        }
        DeliveryEvidence::Response => ("response", None),
    };
    PacketReceipt {
        rtt_millis: receipt.rtt.millis() as f64,
        evidence: evidence.to_string(),
        packet_hash,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        host_snapshot_info, interface_i64, parse_resource_strategy, safe_u64_argument,
        stable_interface_routing_policy, InterfaceRoutingPolicySpec, ResourceStrategySpec,
    };
    use personal_rns::routing::links::resources::ResourceStrategy;
    use prns_host::{
        BackendInfo, BackendKind, Capability, HostSnapshot, InterfaceHealth, InterfaceId,
        InterfaceKind, InterfaceSnapshot, PersistenceFlushCause, PersistenceSnapshot,
        RuntimeHealthSnapshot,
    };

    #[test]
    fn unit_bearing_resource_limit_is_accepted() {
        let strategy = ResourceStrategySpec {
            accept: "all".to_string(),
            max_uncompressed_bytes: Some(4_096.0),
            accept_compressed: None,
        };
        let expected = ResourceStrategy::Accept {
            max_uncompressed_bytes: 4_096,
            accept_compressed: true,
        };

        assert_eq!(parse_resource_strategy(&strategy).ok(), Some(expected));
    }

    #[test]
    fn safe_integer_boundary_is_enforced() {
        assert_eq!(
            safe_u64_argument(prns_host::SAFE_UINT_MAX as f64, "value").ok(),
            Some(prns_host::SAFE_UINT_MAX)
        );
        assert!(safe_u64_argument(9_007_199_254_740_992.0, "value").is_err());
        assert!(safe_u64_argument(1.5, "value").is_err());
        assert_eq!(
            interface_i64(prns_host::SAFE_INT_MIN as f64, "value").ok(),
            Some(prns_host::SAFE_INT_MIN)
        );
        assert!(interface_i64(-9_007_199_254_740_992.0, "value").is_err());
    }

    #[test]
    fn interface_routing_policy_uses_exact_contract_names() {
        let policy = stable_interface_routing_policy(InterfaceRoutingPolicySpec {
            mode: Some("Boundary".to_string()),
            gravity: Some(-73.0),
            recursive_path_requests: Some(true),
            announces_from_internal: Some(false),
            announces_to_internal: Some(true),
        })
        .expect("routing policy should be valid");
        assert_eq!(policy.mode, Some(prns_host::InterfaceMode::Boundary));
        assert_eq!(policy.gravity, Some(-73));
        assert_eq!(policy.recursive_path_requests, Some(true));
        assert_eq!(policy.announces_from_internal, Some(false));
        assert_eq!(policy.announces_to_internal, Some(true));
    }

    #[test]
    fn snapshot_uses_exact_contract_names() {
        let snapshot = HostSnapshot {
            revision: 1,
            backend: BackendInfo::new(
                BackendKind::Native,
                [Capability::TcpClient],
                [InterfaceKind::TcpClient],
            ),
            interfaces: vec![InterfaceSnapshot {
                interface_id: InterfaceId::new([0; 8]),
                name: None,
                kind: Some(InterfaceKind::TcpClient),
                health: InterfaceHealth::Reconnecting,
                failure_detail: None,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_bps: None,
                tx_bps: None,
                route_count: 0,
                link_count: 0,
                transported_link_count: 0,
            }],
            routes: Vec::new(),
            active_link_count: 0,
            destination_identities: Vec::new(),
            runtime: RuntimeHealthSnapshot {
                running: true,
                uptime_millis: 0,
                interface_count: 1,
                online_interface_count: 0,
                route_count: 0,
                link_count: 0,
                transported_link_count: 0,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_bps: 0,
                tx_bps: 0,
            },
            persistence: PersistenceSnapshot {
                persistent: true,
                restored: true,
                last_flush_cause: Some(PersistenceFlushCause::RatchetRotation),
                last_failure_detail: None,
            },
        };

        let projected = host_snapshot_info(snapshot);
        assert_eq!(projected.backend.backend, "Native");
        assert_eq!(projected.backend.capabilities, ["TcpClient"]);
        assert_eq!(projected.backend.interface_kinds, ["TcpClient"]);
        assert_eq!(projected.interfaces[0].kind.as_deref(), Some("TcpClient"));
        assert_eq!(projected.interfaces[0].health, "Reconnecting");
        assert_eq!(
            projected.persistence.last_flush_cause.as_deref(),
            Some("RatchetRotation")
        );
    }
}
