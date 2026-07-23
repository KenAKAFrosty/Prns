use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use napi::bindgen_prelude::{Buffer, Function};
use napi::threadsafe_function::ThreadsafeCallContext;
use napi::Result;
use napi_derive::napi;
use personal_rns::engine::{
    AllowRequester, AnnounceAppData, AnnounceNow, AnnounceTarget, DeliveryEvidence, DeliveryProof,
    EstablishLinkFailure, RatchetPolicy, RequestResponseTimeout,
};
use personal_rns::interfaces::shared_instance as shared_instance_contract;
use personal_rns::interfaces::BitrateBps;
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::routing::request_handlers::RequestPolicy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_router::RespondToken;
use personal_rns::runtime::{RequestPathError, SegmentCompression};
use personal_rns::shared_instance::{SharedInstanceClient, SharedInstanceServer};
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::udp::UdpInterface;
use personal_rns::units::{DurationMillis, RttMillis};
use personal_rns::ResourceStrategy;
use personal_rns::{attach_plan_with_context, PlanOutcome, PlanRuntimeContext};
use personal_rns::{
    load_or_create_identity_secret, try_generate_identity_secret, AttachedInterface,
    AttachedSupervisor, PacketReceiptDelivered, Zeroizing, IDENTITY_SECRET_KEY_LEN,
};

use crate::errors::{code_err, send_error, CodeResult, ErrorCode, Fallible};
use crate::events::bridge::EventSink;
use crate::events::owned::OwnedEvent;
use crate::events::translate;
use crate::marshal;
use crate::runtime::{DestinationConfig, NodeConfig, NodeManager, SingleConfig};

#[napi(object)]
pub struct IdentitySpec {
    pub secret: Option<Buffer>,
    pub path: Option<String>,
}

#[napi(object)]
pub struct RequestPathSpec {
    pub path: String,
    pub policy: Option<String>,
}

#[napi(object)]
pub struct ResourceStrategySpec {
    pub accept: String,
    pub max_uncompressed_len: Option<f64>,
    pub accept_compressed: Option<bool>,
}

#[napi(object)]
pub struct DestinationSpec {
    pub app_name: String,
    pub aspects: Vec<String>,
    pub kind: Option<String>,
    pub identity: Option<IdentitySpec>,
    pub announce_app_data: Option<Buffer>,
    pub proof: Option<String>,
    pub link_requests: Option<String>,
    pub ratchet: Option<String>,
    pub resource_strategy: Option<ResourceStrategySpec>,
    pub request_paths: Option<Vec<RequestPathSpec>>,
}

#[napi(object)]
pub struct NodeOptions {
    pub identity: Option<IdentitySpec>,
    pub transport: Option<bool>,
    pub destinations: Option<Vec<DestinationSpec>>,
}

#[napi(object)]
pub struct AnnounceOptions {
    pub interface_id: Option<Buffer>,
}

#[napi(object)]
pub struct PacketReceipt {
    pub rtt_millis: f64,
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
    pub timeout_ms: Option<f64>,
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
    pub compression: Option<String>,
    pub progress: Option<bool>,
}

#[napi(object)]
pub struct ResourceData {
    pub data: Buffer,
    pub metadata: Option<Buffer>,
    pub original_hash: Buffer,
    pub total_size: f64,
}

#[napi(object)]
pub struct ResourceFileReceipt {
    pub metadata: Option<Buffer>,
    pub original_hash: Buffer,
    pub total_size: f64,
}

fn resolve_identity(spec: &IdentitySpec) -> CodeResult<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>> {
    match (&spec.secret, &spec.path) {
        (Some(secret), None) => marshal::identity_secret(secret),
        (None, Some(path)) => load_or_create_identity_secret(Path::new(path)).map_err(|error| {
            code_err(
                ErrorCode::InvalidIdentityFile,
                format!("identity file at {path}: {error:?}"),
            )
        }),
        _ => Err(code_err(
            ErrorCode::InvalidArgument,
            "identity requires exactly one of secret or path",
        )),
    }
}

fn parse_proof(value: Option<&str>) -> CodeResult<ProofStrategy> {
    match value {
        None | Some("proveAll") => Ok(ProofStrategy::ProveAll),
        Some("proveNone") => Ok(ProofStrategy::ProveNone),
        Some("proveIf") => Ok(ProofStrategy::ProveIf),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("unknown proof strategy {other:?}; expected proveAll, proveNone, or proveIf"),
        )),
    }
}

fn parse_link_requests(value: Option<&str>) -> CodeResult<LinkRequestPolicy> {
    match value {
        None | Some("acceptAll") => Ok(LinkRequestPolicy::AcceptAll),
        Some("acceptNone") => Ok(LinkRequestPolicy::AcceptNone),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("unknown link request policy {other:?}; expected acceptAll or acceptNone"),
        )),
    }
}

fn parse_ratchet(value: Option<&str>) -> CodeResult<RatchetPolicy> {
    match value {
        None | Some("noRatchets") => Ok(RatchetPolicy::NoRatchets),
        Some("ratcheted") => Ok(RatchetPolicy::Ratcheted),
        Some("ratchetsRequired") => Ok(RatchetPolicy::RatchetsRequired),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!(
                "unknown ratchet policy {other:?}; expected noRatchets, ratcheted, or ratchetsRequired"
            ),
        )),
    }
}

fn parse_request_policy(value: Option<&str>) -> CodeResult<RequestPolicy> {
    match value {
        None | Some("allowAll") => Ok(RequestPolicy::AllowAll),
        Some("allowNone") => Ok(RequestPolicy::AllowNone),
        Some("allowList") => Ok(RequestPolicy::AllowList),
        Some(other) => Err(code_err(
            ErrorCode::InvalidArgument,
            format!("unknown request policy {other:?}; expected allowAll, allowNone, or allowList"),
        )),
    }
}

fn link_error(error: personal_rns::SendError<EstablishLinkFailure>) -> crate::errors::CodeError {
    match error {
        personal_rns::SendError::Failed(EstablishLinkFailure::Timeout) => {
            code_err(ErrorCode::LinkTimeout, "link establishment timed out")
        }
        other => send_error(ErrorCode::LinkFailed, other),
    }
}

fn path_error(error: RequestPathError) -> crate::errors::CodeError {
    match error {
        RequestPathError::EntropyUnavailable => {
            code_err(ErrorCode::Internal, "entropy unavailable")
        }
        RequestPathError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        RequestPathError::Failed(failure) => {
            code_err(ErrorCode::PathFailed, format!("{failure:?}"))
        }
    }
}

const DEFAULT_ACCEPT_MAX_UNCOMPRESSED_LEN: u64 = 64 * 1024 * 1024;

fn parse_resource_strategy(spec: &ResourceStrategySpec) -> CodeResult<ResourceStrategy> {
    match spec.accept.as_str() {
        "none" => Ok(ResourceStrategy::AcceptNone),
        "all" => {
            let max_uncompressed_len = match spec.max_uncompressed_len {
                None => DEFAULT_ACCEPT_MAX_UNCOMPRESSED_LEN,
                Some(len) if len.is_finite() && len >= 0.0 => len as u64,
                Some(_) => {
                    return Err(code_err(
                        ErrorCode::InvalidArgument,
                        "maxUncompressedLen must be a non-negative finite number",
                    ))
                }
            };
            Ok(ResourceStrategy::Accept {
                max_uncompressed_len,
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

fn parse_bitrate(value: Option<f64>) -> CodeResult<BitrateBps> {
    match value {
        None => Ok(BitrateBps::guess(65_000_000)),
        Some(bps) if bps.is_finite() && bps >= 1.0 => {
            BitrateBps::new(bps as u64).ok_or_else(|| {
                code_err(
                    ErrorCode::InvalidArgument,
                    "bitrateBps is below the minimum",
                )
            })
        }
        Some(_) => Err(code_err(
            ErrorCode::InvalidArgument,
            "bitrateBps must be a positive finite number",
        )),
    }
}

fn parse_options(options: NodeOptions) -> CodeResult<NodeConfig> {
    let transport = options.transport.unwrap_or(false);
    let node_identity = options
        .identity
        .as_ref()
        .map(resolve_identity)
        .transpose()?;
    let transport_identity = if transport {
        Some(node_identity.clone().ok_or_else(|| {
            code_err(
                ErrorCode::InvalidArgument,
                "transport: true requires a node identity",
            )
        })?)
    } else {
        None
    };
    let mut destinations = Vec::new();
    for spec in options.destinations.unwrap_or_default() {
        let kind = spec.kind.as_deref().unwrap_or("single");
        let single = match kind {
            "plain" => None,
            "single" => {
                let identity = match &spec.identity {
                    Some(identity) => resolve_identity(identity)?,
                    None => try_generate_identity_secret().map_err(|error| {
                        code_err(
                            ErrorCode::Internal,
                            format!("entropy unavailable: {error:?}"),
                        )
                    })?,
                };
                let mut request_paths = Vec::new();
                for path_spec in spec.request_paths.iter().flatten() {
                    request_paths.push((
                        path_spec.path.clone(),
                        parse_request_policy(path_spec.policy.as_deref())?,
                    ));
                }
                Some(SingleConfig {
                    identity,
                    announce_app_data: spec
                        .announce_app_data
                        .as_ref()
                        .map(|data| data.to_vec())
                        .unwrap_or_default(),
                    proof: parse_proof(spec.proof.as_deref())?,
                    link_requests: parse_link_requests(spec.link_requests.as_deref())?,
                    ratchet: parse_ratchet(spec.ratchet.as_deref())?,
                    resource_strategy: spec
                        .resource_strategy
                        .as_ref()
                        .map(parse_resource_strategy)
                        .transpose()?
                        .unwrap_or(ResourceStrategy::AcceptNone),
                    request_paths,
                })
            }
            other => {
                return Err(code_err(
                    ErrorCode::InvalidArgument,
                    format!("unknown destination kind {other:?}; expected single or plain"),
                ))
            }
        };
        if single.is_none()
            && spec
                .request_paths
                .as_ref()
                .is_some_and(|paths| !paths.is_empty())
        {
            return Err(code_err(
                ErrorCode::InvalidArgument,
                "requestPaths require a single destination",
            ));
        }
        destinations.push(DestinationConfig {
            app_name: spec.app_name,
            aspects: spec.aspects,
            single,
        });
    }
    Ok(NodeConfig {
        transport_identity,
        destinations,
    })
}

enum Attachment {
    Interface(AttachedInterface),
    Supervisor(AttachedSupervisor),
}

#[napi]
pub struct InterfaceHandle {
    id_bytes: [u8; 8],
    kind_name: Option<String>,
    attachment: Mutex<Option<Attachment>>,
}

impl InterfaceHandle {
    fn from_interface(attached: AttachedInterface) -> Self {
        let id = attached.id();
        Self {
            id_bytes: *id.as_bytes(),
            kind_name: id.kind().map(|kind| format!("{kind:?}")),
            attachment: Mutex::new(Some(Attachment::Interface(attached))),
        }
    }

    fn from_supervisor(attached: AttachedSupervisor) -> Self {
        let id = attached.id();
        Self {
            id_bytes: *id.as_bytes(),
            kind_name: id.kind().map(|kind| format!("{kind:?}")),
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
            Some(Attachment::Interface(attached)) => {
                attached.teardown();
                true
            }
            Some(Attachment::Supervisor(attached)) => {
                attached.teardown();
                true
            }
            None => false,
        }
    }
}

#[napi]
pub struct PrnsNode {
    manager: Arc<NodeManager>,
    hashes: Vec<[u8; 16]>,
}

#[napi]
pub fn start_node(
    options: NodeOptions,
    #[napi(ts_arg_type = "(event: Record<string, any>) => void")] on_event: Function<(), ()>,
) -> Result<PrnsNode, ErrorCode> {
    let config = parse_options(options)?;
    let hashes = crate::runtime::destination_hashes(&config)?;
    let tsfn = on_event
        .build_threadsafe_function::<OwnedEvent>()
        .build_callback(|ctx: ThreadsafeCallContext<OwnedEvent>| {
            translate::event_to_object(&ctx.env, ctx.value)
        })
        .map_err(|error| code_err(ErrorCode::Internal, format!("{error}")))?;
    let manager = NodeManager::start(config, EventSink::new(tsfn))?;
    Ok(PrnsNode {
        manager: Arc::new(manager),
        hashes,
    })
}

#[napi]
impl PrnsNode {
    #[napi(getter)]
    pub fn destination_hashes(&self) -> Vec<Buffer> {
        self.hashes
            .iter()
            .map(|hash| marshal::to_buffer(hash))
            .collect()
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn ready(&self) -> Result<Fallible<()>> {
        Ok(Fallible(self.manager.ready().await))
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub async fn stop(&self) -> Result<Fallible<()>> {
        Ok(Fallible(self.manager.stop().await))
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
        let handle = self.manager.handle()?;
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
}

impl PrnsNode {
    async fn establish_link_inner(&self, destination: Buffer) -> CodeResult<LinkInfo> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.manager.handle()?;
        let established = handle
            .establish_link_with_rtt(destination)
            .await
            .map_err(link_error)?;
        Ok(LinkInfo {
            link_id: marshal::to_buffer(established.link_id.as_bytes()),
            rtt_millis: established.rtt_ms as f64,
        })
    }

    async fn request_path_inner(&self, destination: Buffer) -> CodeResult<PathInfo> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.manager.handle()?;
        let found = handle.request_path(destination).await.map_err(path_error)?;
        Ok(PathInfo {
            hops: u32::from(found.hops.0),
        })
    }

    async fn identify_inner(&self, link_id: Buffer, identity: Buffer) -> CodeResult<()> {
        let link_id = marshal::link_id(&link_id)?;
        let identity = marshal::identity_hash(&identity)?;
        let handle = self.manager.handle()?;
        handle
            .identify(link_id, identity)
            .await
            .map_err(|error| send_error(ErrorCode::IdentifyFailed, error))
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
        let timeout = match options.and_then(|opts| opts.timeout_ms) {
            Some(ms) if ms.is_finite() && ms >= 0.0 => {
                RequestResponseTimeout::Exact(DurationMillis(ms as u64))
            }
            Some(_) => {
                return Err(code_err(
                    ErrorCode::InvalidArgument,
                    "timeoutMs must be a non-negative finite number",
                ))
            }
            None => RequestResponseTimeout::LinkDefault,
        };
        let handle = self.manager.handle()?;
        let (packed, rtt) = handle
            .request_with_response_timeout(link_id, path_hash, &data, timeout)
            .await
            .map_err(|error| send_error(ErrorCode::RequestFailed, error))?;
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
            rtt: RttMillis::new(token.rtt_millis as u64),
        })
    }

    async fn respond_inner(&self, token: RespondTokenSpec, data: Buffer) -> CodeResult<f64> {
        let token = Self::respond_token(&token)?;
        let handle = self.manager.handle()?;
        handle
            .respond_owned_bytes(token, data.to_vec())
            .map(|rtt| rtt.millis() as f64)
            .ok_or_else(|| code_err(ErrorCode::NodeStopped, "node stopped"))
    }

    async fn respond_file_inner(&self, token: RespondTokenSpec, path: String) -> CodeResult<f64> {
        let token = Self::respond_token(&token)?;
        let handle = self.manager.handle()?;
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
        let handle = self.manager.handle()?;
        handle
            .allow_requester(allow)
            .await
            .map_err(|error| send_error(ErrorCode::AllowFailed, error))
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
        let handle = self.manager.handle()?;
        handle
            .announce_now(AnnounceNow {
                destination,
                target,
                app_data: AnnounceAppData::Registered,
            })
            .await
            .map_err(|error| send_error(ErrorCode::AnnounceFailed, error))
    }

    async fn send_single_packet_inner(
        &self,
        destination: Buffer,
        data: Buffer,
    ) -> CodeResult<PacketReceipt> {
        let destination = marshal::destination_hash(&destination)?;
        let handle = self.manager.handle()?;
        let receipt = handle
            .send_single_packet(destination, &data)
            .await
            .map_err(|error| send_error(ErrorCode::SendFailed, error))?;
        Ok(packet_receipt(receipt))
    }

    async fn attach_tcp_server_inner(
        &self,
        options: TcpServerOptions,
    ) -> CodeResult<InterfaceHandle> {
        let bitrate = parse_bitrate(options.bitrate_bps)?;
        let bind = options.bind;
        let attached = self
            .manager
            .on_node_runtime(move |handle| async move {
                TcpServer::bind(bind.as_str(), bitrate)
                    .await
                    .map(|server| handle.supervise(server))
            })
            .await?
            .map_err(|error| {
                code_err(
                    ErrorCode::AttachFailed,
                    format!("tcp server bind failed: {error}"),
                )
            })?;
        Ok(InterfaceHandle::from_supervisor(attached))
    }

    async fn attach_tcp_client_inner(
        &self,
        options: TcpClientOptions,
    ) -> CodeResult<InterfaceHandle> {
        let bitrate = parse_bitrate(options.bitrate_bps)?;
        let client = TcpClientInterface::new(options.target, bitrate, ReconnectPolicy::STANDARD);
        let handle = self.manager.handle()?;
        let attached = handle.add_interface(client);
        Ok(InterfaceHandle::from_interface(attached))
    }

    async fn attach_udp_inner(&self, options: UdpOptions) -> CodeResult<InterfaceHandle> {
        let bitrate = parse_bitrate(options.bitrate_bps)?;
        let local = options.local;
        let peer = options.peer;
        let attached = self
            .manager
            .on_node_runtime(move |handle| async move {
                UdpInterface::bind(local.as_str(), peer.as_str(), bitrate)
                    .await
                    .map(|udp| handle.add_interface(udp))
            })
            .await?
            .map_err(|error| {
                code_err(ErrorCode::AttachFailed, format!("udp bind failed: {error}"))
            })?;
        Ok(InterfaceHandle::from_interface(attached))
    }

    async fn attach_shared_instance_server_inner(
        &self,
        options: Option<SharedInstanceOptions>,
    ) -> CodeResult<InterfaceHandle> {
        let port = options.and_then(|opts| opts.port);
        let attached = self
            .manager
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
            .manager
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
        let sink = self.manager.sink()?;
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(progress) = progress_rx.recv().await {
                let personal_rns::runtime::ResourceProgress {
                    transferred,
                    total,
                    physical_transferred,
                    segment_index,
                    total_segments,
                } = progress;
                sink.emit(OwnedEvent::ResourceSendProgress {
                    link_id,
                    transferred,
                    total,
                    physical_transferred,
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
        let handle = self.manager.handle()?;
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
        result.map_err(|error| code_err(ErrorCode::ResourceSendFailed, format!("{error:?}")))
    }

    async fn send_resource_file_inner(
        &self,
        link_id: Buffer,
        path: String,
        options: Option<SendResourceOptions>,
    ) -> CodeResult<()> {
        let link = marshal::link_id(&link_id)?;
        let handle = self.manager.handle()?;
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
        result.map_err(|error| code_err(ErrorCode::ResourceSendFailed, format!("{error:?}")))
    }

    async fn receive_resource_inner(&self, link_id: Buffer) -> CodeResult<ResourceData> {
        let link = marshal::link_id(&link_id)?;
        let handle = self.manager.handle()?;
        let mut collected: Vec<u8> = Vec::new();
        let receipt = handle
            .receive_resource(link, &mut collected)
            .await
            .map_err(|error| code_err(ErrorCode::ResourceReceiveFailed, format!("{error:?}")))?;
        Ok(ResourceData {
            data: Buffer::from(collected),
            metadata: receipt.metadata.map(Buffer::from),
            original_hash: marshal::to_buffer(receipt.original_hash.as_bytes()),
            total_size: receipt.total_size as f64,
        })
    }

    async fn receive_resource_file_inner(
        &self,
        link_id: Buffer,
        path: String,
    ) -> CodeResult<ResourceFileReceipt> {
        let link = marshal::link_id(&link_id)?;
        let handle = self.manager.handle()?;
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
            total_size: receipt.total_size as f64,
        })
    }

    async fn set_resource_strategy_inner(
        &self,
        destination: Buffer,
        strategy: ResourceStrategySpec,
    ) -> CodeResult<bool> {
        let destination = marshal::destination_hash(&destination)?;
        let strategy = parse_resource_strategy(&strategy)?;
        let handle = self.manager.handle()?;
        Ok(handle.set_resource_strategy(destination, strategy).await)
    }

    async fn set_link_resource_strategy_inner(
        &self,
        link_id: Buffer,
        strategy: ResourceStrategySpec,
    ) -> CodeResult<()> {
        let link = marshal::link_id(&link_id)?;
        let strategy = parse_resource_strategy(&strategy)?;
        let handle = self.manager.handle()?;
        handle
            .set_link_resource_strategy(link, strategy)
            .await
            .map_err(|error| send_error(ErrorCode::ResourceStrategyFailed, error))
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
            .manager
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
        self.manager.store_plan_attachments(attachments);
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
