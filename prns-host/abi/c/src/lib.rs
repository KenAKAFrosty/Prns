#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

mod readiness;
mod supplied_pipe;

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::str;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use prns_host_core::{
    verify_host_contract, ApplicationEvent, ApplicationEventKind as AbiApplicationEventKind,
    BackendKind as AbiBackendKind, Bitrate, BitrateKind as AbiBitrateKind, BoundedHostQueue,
    Capability as AbiCapability, Capability, CommandFailure,
    CommandFailureKind as AbiCommandFailureKind, CommandOutcome,
    CommandOutcomeKind as AbiCommandOutcomeKind, ConsumerLane, DeliveryEvidence,
    DeliveryEvidenceKind as AbiDeliveryEvidenceKind, DestinationConfig,
    DestinationConfigKind as AbiDestinationConfigKind, DestinationHash, DestinationIdentityConfig,
    DestinationIdentityConfigKind as AbiDestinationIdentityConfigKind, DestinationName,
    DiagnosticEvent, DiagnosticEventKind as AbiDiagnosticEventKind,
    DiscoveryScope as AbiDiscoveryScope, DiscoveryScope, EventField as AbiEventField, HostCommand,
    HostConfig, HostFailure, HostRole as AbiHostRole, HostRole, HostSnapshot as CoreHostSnapshot,
    IdentityConfig, IdentityConfigKind as AbiIdentityConfigKind, IdentityHash, IdentitySecret,
    InterfaceConfig, InterfaceHealth, InterfaceId, InterfaceKind as AbiInterfaceKind,
    InterfaceKind, InterfaceMode, InterfaceRoutingPolicy, LifecyclePhase as AbiLifecyclePhase,
    LifecycleState, LinkClosedReason as AbiLinkClosedReason, LinkClosedReason, LinkId,
    MultiRNodeMemberConfig, MulticastAddressType as AbiMulticastAddressType, MulticastAddressType,
    PersistenceConfig, PersistenceConfigKind as AbiPersistenceConfigKind,
    PersistenceFlushCause as AbiPersistenceFlushCause, PersistenceFlushCause,
    PersistenceFlushTarget as AbiPersistenceFlushTarget, PersistenceFlushTarget,
    PrnsLimits as CoreLimits, RNodeRadioConfig, RequestHandlerConfig, RequestId, RequestPathHash,
    RequestPolicy as AbiRequestPolicy, RequestPolicy, ResourceAvailable, ResourceCompression,
    ResourceCompressionKind as AbiResourceCompressionKind, ResourceStrategy,
    ResourceStrategyKind as AbiResourceStrategyKind, ResponseTimeout,
    ResponseTimeoutKind as AbiResponseTimeoutKind, SerialDataBits as AbiSerialDataBits,
    SerialDataBits, SerialLineConfig, SerialParity as AbiSerialParity, SerialParity,
    SerialStopBits as AbiSerialStopBits, SerialStopBits, Status as AbiStatus,
    StopReason as AbiStopReason, StopReason, WebSocketFramingSelection, HOST_CONTRACT,
    HOST_SCHEMA_VERSION, SAFE_INT_MAX, SAFE_INT_MIN, SAFE_UINT_MAX,
};
use prns_host_native::{
    CommandHandle, CommandWait, IdentityStartError, NativeEventSink, NativeHost,
    NativeSnapshotError, NativeStartError, NativeSubmitError, NativeUpload, PersistenceStartError,
    UploadWriteError,
};
use readiness::{Readiness, ReadinessCallback, RegisteredReadiness};

const NEVER_TIMEOUT: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsByteView {
    pub data: *const u8,
    pub length: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsStringView {
    pub data: *const u8,
    pub length: usize,
}

#[repr(C)]
pub struct PrnsContractInfo {
    pub struct_size: usize,
    pub abi: u32,
    pub schema_version: u32,
    pub product_version: PrnsStringView,
}

#[repr(C)]
pub struct PrnsBackendInfo {
    pub struct_size: usize,
    pub backend: u32,
    pub capabilities: *const u32,
    pub capability_count: usize,
    pub interface_kinds: *const u32,
    pub interface_kind_count: usize,
}

#[repr(C)]
pub struct PrnsInterfaceSnapshot {
    pub struct_size: usize,
    pub interface_id: PrnsByteView,
    pub has_name: u8,
    pub name: PrnsStringView,
    pub has_kind: u8,
    pub kind: u32,
    pub health: u32,
    pub has_failure_detail: u8,
    pub failure_detail: PrnsStringView,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub has_rx_bps: u8,
    pub rx_bps: u64,
    pub has_tx_bps: u8,
    pub tx_bps: u64,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
}

#[repr(C)]
pub struct PrnsRouteSnapshot {
    pub struct_size: usize,
    pub destination: PrnsByteView,
    pub hops: u8,
    pub has_via_identity: u8,
    pub via_identity: PrnsByteView,
    pub interface_id: PrnsByteView,
    pub learned_at_millis: u64,
    pub last_route_activity_at_millis: u64,
    pub expires_at_millis: u64,
}

#[repr(C)]
pub struct PrnsDestinationIdentitySnapshot {
    pub struct_size: usize,
    pub destination: PrnsByteView,
    pub identity: PrnsByteView,
}

#[repr(C)]
pub struct PrnsRuntimeHealthSnapshot {
    pub struct_size: usize,
    pub running: u8,
    pub uptime_millis: u64,
    pub interface_count: u32,
    pub online_interface_count: u32,
    pub route_count: u32,
    pub link_count: u32,
    pub transported_link_count: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: u64,
    pub tx_bps: u64,
}

#[repr(C)]
pub struct PrnsPersistenceSnapshot {
    pub struct_size: usize,
    pub persistent: u8,
    pub restored: u8,
    pub has_last_flush_cause: u8,
    pub last_flush_cause: u32,
    pub has_last_failure_detail: u8,
    pub last_failure_detail: PrnsStringView,
}

#[repr(C)]
pub struct PrnsHostSnapshot {
    pub struct_size: usize,
    pub revision: u64,
    pub backend: PrnsBackendInfo,
    pub interfaces: *const PrnsInterfaceSnapshot,
    pub interface_count: usize,
    pub routes: *const PrnsRouteSnapshot,
    pub route_count: usize,
    pub active_link_count: u32,
    pub destination_identities: *const PrnsDestinationIdentitySnapshot,
    pub destination_identity_count: usize,
    pub runtime: PrnsRuntimeHealthSnapshot,
    pub persistence: PrnsPersistenceSnapshot,
}

#[cfg(unix)]
static NATIVE_CAPABILITIES: [u32; 4] = [
    AbiCapability::TcpClient as u32,
    AbiCapability::TcpServer as u32,
    AbiCapability::Udp as u32,
    AbiCapability::SuppliedPipe as u32,
];

#[cfg(not(unix))]
static NATIVE_CAPABILITIES: [u32; 3] = [
    AbiCapability::TcpClient as u32,
    AbiCapability::TcpServer as u32,
    AbiCapability::Udp as u32,
];

static NATIVE_INTERFACE_KINDS: [u32; 3] = [
    AbiInterfaceKind::TcpClient as u32,
    AbiInterfaceKind::TcpServer as u32,
    AbiInterfaceKind::Udp as u32,
];

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsLimits {
    pub struct_size: usize,
    pub pending_commands: usize,
    pub application_events: usize,
    pub retained_event_bytes: usize,
    pub diagnostics: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsIdentityConfig {
    pub struct_size: usize,
    pub kind: u32,
    pub secret: PrnsByteView,
    pub path: PrnsStringView,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsPersistenceConfig {
    pub struct_size: usize,
    pub kind: u32,
    pub path: PrnsStringView,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsDestinationName {
    pub struct_size: usize,
    pub app_name: PrnsStringView,
    pub aspects: *const PrnsStringView,
    pub aspect_count: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsRequestHandlerConfig {
    pub struct_size: usize,
    pub path: PrnsStringView,
    pub policy: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsSerialLineConfig {
    pub struct_size: usize,
    pub baud: u32,
    pub data_bits: u32,
    pub parity: u32,
    pub stop_bits: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsRNodeRadioConfig {
    pub struct_size: usize,
    pub frequency_hz: u64,
    pub bandwidth_hz: u32,
    pub tx_power_dbm: i16,
    pub spreading_factor: u8,
    pub coding_rate: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsMultiRNodeMemberConfig {
    pub struct_size: usize,
    pub name: PrnsStringView,
    pub virtual_port: u8,
    pub radio: PrnsRNodeRadioConfig,
    pub flow_control: u8,
    pub outgoing: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsInterfaceConfig {
    pub struct_size: usize,
    pub kind: u32,
    pub has_group_id: u8,
    pub group_id: PrnsStringView,
    pub has_discovery_scope: u8,
    pub discovery_scope: u32,
    pub has_discovery_port: u8,
    pub discovery_port: u16,
    pub has_data_port: u8,
    pub data_port: u16,
    pub devices: *const PrnsStringView,
    pub device_count: usize,
    pub ignored_devices: *const PrnsStringView,
    pub ignored_device_count: usize,
    pub has_multicast_address_type: u8,
    pub multicast_address_type: u32,
    pub target: PrnsStringView,
    pub bind: PrnsStringView,
    pub local: PrnsStringView,
    pub peer: PrnsStringView,
    pub bitrate_kind: u32,
    pub bitrate_bps: u64,
    pub port: PrnsStringView,
    pub line: PrnsSerialLineConfig,
    pub flow_control: u8,
    pub preamble_millis: u32,
    pub transmit_tail_millis: u32,
    pub persistence: u8,
    pub slot_time_millis: u32,
    pub has_station_callsign: u8,
    pub station_callsign: PrnsStringView,
    pub has_station_interval_seconds: u8,
    pub station_interval_seconds: u64,
    pub callsign: PrnsStringView,
    pub ssid: u8,
    pub radio: PrnsRNodeRadioConfig,
    pub has_airtime_limit_short_centi_percent: u8,
    pub airtime_limit_short_centi_percent: u16,
    pub has_airtime_limit_long_centi_percent: u8,
    pub airtime_limit_long_centi_percent: u16,
    pub members: *const PrnsMultiRNodeMemberConfig,
    pub member_count: usize,
    pub command: *const PrnsStringView,
    pub command_count: usize,
    pub respawn_delay_millis: u64,
    pub peers: *const PrnsStringView,
    pub peer_count: usize,
    pub connectable: u8,
    pub url: PrnsStringView,
    pub websocket_framing_selection: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsInterfaceRoutingPolicy {
    pub struct_size: usize,
    pub has_mode: u8,
    pub mode: u32,
    pub has_gravity: u8,
    pub gravity: i64,
    pub has_recursive_path_requests: u8,
    pub recursive_path_requests: u8,
    pub has_announces_from_internal: u8,
    pub announces_from_internal: u8,
    pub has_announces_to_internal: u8,
    pub announces_to_internal: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsDestinationConfig {
    pub struct_size: usize,
    pub kind: u32,
    pub name: PrnsDestinationName,
    pub identity_kind: u32,
    pub dedicated_identity: PrnsIdentityConfig,
    pub announce_app_data: PrnsByteView,
    pub request_handlers: *const PrnsRequestHandlerConfig,
    pub request_handler_count: usize,
    pub has_maximum_request_bytes: u8,
    pub maximum_request_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PrnsHostOptions {
    pub struct_size: usize,
    pub required_abi: u32,
    pub required_schema_version: u32,
    pub required_product_version: PrnsStringView,
    pub limits: PrnsLimits,
    pub role: u32,
    pub identity: PrnsIdentityConfig,
    pub destinations: *const PrnsDestinationConfig,
    pub destination_count: usize,
    pub required_capabilities: *const u32,
    pub required_capability_count: usize,
    pub persistence: PrnsPersistenceConfig,
}

struct HostOptionsInput {
    required_abi: u32,
    required_schema_version: u32,
    required_product_version: PrnsStringView,
    limits: PrnsLimits,
    role: u32,
    identity: PrnsIdentityConfig,
    destinations: *const PrnsDestinationConfig,
    destination_count: usize,
    required_capabilities: *const u32,
    required_capability_count: usize,
    persistence: PrnsPersistenceConfig,
}

#[repr(C)]
pub struct PrnsLifecycle {
    pub struct_size: usize,
    pub revision: u64,
    pub phase: u32,
    pub reason: u32,
}

#[repr(C)]
pub struct PrnsCommandResult {
    pub struct_size: usize,
    pub outcome: u32,
    pub failure: u32,
    pub evidence: u32,
    pub rtt_millis: u64,
    pub value: PrnsByteView,
    pub detail: PrnsStringView,
}

struct Shared {
    queue: Mutex<BoundedHostQueue<()>>,
    resources: Mutex<BTreeMap<u64, PrnsResourceStream>>,
    ready: Condvar,
    application_readiness: Arc<Readiness>,
    diagnostic_readiness: Arc<Readiness>,
    stop_requested: AtomicBool,
}

impl Shared {
    fn readiness(&self, lane: ConsumerLane) -> &Arc<Readiness> {
        match lane {
            ConsumerLane::ApplicationEvents => &self.application_readiness,
            ConsumerLane::Diagnostics => &self.diagnostic_readiness,
        }
    }

    fn notify_lane(&self, lane: ConsumerLane) {
        self.ready.notify_all();
        self.readiness(lane).notify();
    }

    fn notify_all(&self) {
        self.ready.notify_all();
        self.application_readiness.notify();
        self.diagnostic_readiness.notify();
    }
}

pub struct HostPublisher {
    shared: Arc<Shared>,
}

impl Clone for HostPublisher {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl HostPublisher {
    pub fn publish_application(&self, event: ApplicationEvent) -> Result<(), ApplicationEvent> {
        let mut queue = lock(&self.shared.queue);
        if matches!(
            queue.lifecycle().state,
            LifecycleState::Stopping | LifecycleState::Stopped(_) | LifecycleState::Failed(_)
        ) {
            return Err(event);
        }
        match queue.push_application_event(event) {
            Ok(()) => {
                drop(queue);
                self.shared.notify_lane(ConsumerLane::ApplicationEvents);
                Ok(())
            }
            Err(rejected) => {
                drop(queue);
                self.shared.notify_all();
                Err(*rejected.event)
            }
        }
    }

    pub fn publish_resource(
        &self,
        event: ResourceAvailable,
        body: Vec<u8>,
    ) -> Result<(), ResourceAvailable> {
        if u64::try_from(body.len()) != Ok(event.total_bytes) {
            return Err(event);
        }
        let stream_id = event.stream_id.get();
        let mut chunks = std::collections::VecDeque::new();
        chunks.push_back(body);
        let rejected_event = event.clone();
        lock(&self.shared.resources).insert(
            stream_id,
            PrnsResourceStream {
                state: Mutex::new(ResourceState {
                    chunks,
                    active: None,
                    offset: 0,
                }),
            },
        );
        match self.publish_application(ApplicationEvent::ResourceAvailable(event)) {
            Ok(()) => Ok(()),
            Err(ApplicationEvent::ResourceAvailable(event)) => {
                lock(&self.shared.resources).remove(&stream_id);
                Err(event)
            }
            Err(_) => {
                lock(&self.shared.resources).remove(&stream_id);
                Err(rejected_event)
            }
        }
    }

    pub fn publish_diagnostic(&self, event: DiagnosticEvent) {
        lock(&self.shared.queue).push_diagnostic(event);
        self.shared.notify_lane(ConsumerLane::Diagnostics);
    }

    pub fn backend_exited(&self) {
        self.finish_stop();
    }

    fn transition_running(&self) {
        let mut queue = lock(&self.shared.queue);
        if matches!(queue.lifecycle().state, LifecycleState::Starting) {
            let _ = queue.transition(LifecycleState::Running);
        }
        drop(queue);
        self.shared.notify_all();
    }

    fn request_stop(&self) {
        self.shared.stop_requested.store(true, Ordering::Release);
        let mut queue = lock(&self.shared.queue);
        if matches!(
            queue.lifecycle().state,
            LifecycleState::Starting | LifecycleState::Running
        ) {
            let _ = queue.transition(LifecycleState::Stopping);
        }
        drop(queue);
        self.shared.notify_all();
    }

    fn finish_stop(&self) {
        let mut queue = lock(&self.shared.queue);
        let state = queue.lifecycle().state;
        if matches!(state, LifecycleState::Starting | LifecycleState::Running) {
            let _ = queue.transition(LifecycleState::Stopping);
        }
        if matches!(queue.lifecycle().state, LifecycleState::Stopping) {
            let reason = if self.shared.stop_requested.load(Ordering::Acquire) {
                StopReason::Requested
            } else {
                StopReason::BackendExited
            };
            let _ = queue.transition(LifecycleState::Stopped(reason));
        }
        drop(queue);
        self.shared.notify_all();
    }

    fn fail(&self, detail: String) {
        let mut queue = lock(&self.shared.queue);
        if !queue.lifecycle().state.is_terminal() {
            let _ = queue.transition(LifecycleState::Failed(HostFailure::BackendFailed {
                component: "native".to_string(),
                detail,
            }));
        }
        drop(queue);
        self.shared.notify_all();
    }
}

impl NativeEventSink for HostPublisher {
    fn running(&self) {
        self.transition_running();
    }

    fn publish_application(&self, event: ApplicationEvent) -> bool {
        HostPublisher::publish_application(self, event).is_ok()
    }

    fn publish_resource(&self, event: ResourceAvailable, body: Vec<u8>) -> bool {
        HostPublisher::publish_resource(self, event, body).is_ok()
    }

    fn publish_diagnostic(&self, event: DiagnosticEvent) {
        HostPublisher::publish_diagnostic(self, event);
    }

    fn stopped(&self) {
        self.finish_stop();
    }

    fn failed(&self, detail: String) {
        self.fail(detail);
    }
}

pub struct PrnsHost {
    shared: Arc<Shared>,
    native: Mutex<Option<NativeHost>>,
    identity_hash: IdentityHash,
    destination_hashes: Vec<DestinationHash>,
}

pub struct PrnsHostInspection {
    snapshot: CoreHostSnapshot,
    interfaces: Vec<PrnsInterfaceSnapshot>,
    routes: Vec<PrnsRouteSnapshot>,
    destination_identities: Vec<PrnsDestinationIdentitySnapshot>,
}

pub struct PrnsEventStream {
    shared: Arc<Shared>,
    lane: ConsumerLane,
    pending_diagnostics_gap: Mutex<u128>,
    interrupted: AtomicBool,
    readiness_registration: Mutex<Option<Arc<RegisteredReadiness>>>,
}

impl Drop for PrnsEventStream {
    fn drop(&mut self) {
        let registration = lock(&self.readiness_registration).take();
        if let Some(registration) = registration {
            self.shared.readiness(self.lane).unregister(&registration);
        }
        lock(&self.shared.queue).release_consumer(self.lane);
        self.shared.notify_all();
    }
}

pub struct PrnsReadinessRegistration {
    readiness: Arc<Readiness>,
    registered: Arc<RegisteredReadiness>,
}

impl Drop for PrnsReadinessRegistration {
    fn drop(&mut self) {
        self.readiness.unregister(&self.registered);
    }
}

enum EventValue {
    Application(ApplicationEvent),
    Diagnostic(DiagnosticEvent),
    DiagnosticsDropped(u128),
}

pub struct PrnsEvent {
    value: EventValue,
    resource: Mutex<Option<PrnsResourceStream>>,
}

pub struct PrnsResourceStream {
    state: Mutex<ResourceState>,
}

pub struct PrnsResourceUpload {
    upload: NativeUpload,
    readiness: Arc<Readiness>,
}

pub struct PrnsIssuedCommand {
    handle: CommandHandle,
    cached: Mutex<Option<CachedCommandResult>>,
    readiness: Arc<Readiness>,
    readiness_registration: Mutex<Option<Arc<RegisteredReadiness>>>,
}

impl Drop for PrnsIssuedCommand {
    fn drop(&mut self) {
        let registration = lock(&self.readiness_registration).take();
        if let Some(registration) = registration {
            self.readiness.unregister(&registration);
        }
    }
}

struct CachedCommandResult {
    outcome: u32,
    failure: u32,
    evidence: u32,
    rtt_millis: u64,
    value: Vec<u8>,
    detail: String,
}

struct ResourceState {
    chunks: std::collections::VecDeque<Vec<u8>>,
    active: Option<Vec<u8>>,
    offset: usize,
}

pub fn host_capsule(limits: CoreLimits) -> (PrnsHost, HostPublisher) {
    host_capsule_with_phase(limits, true)
}

fn host_capsule_with_phase(limits: CoreLimits, running: bool) -> (PrnsHost, HostPublisher) {
    let mut queue = BoundedHostQueue::new(limits);
    if running {
        let _ = queue.transition(LifecycleState::Running);
    }
    let shared = Arc::new(Shared {
        queue: Mutex::new(queue),
        resources: Mutex::new(BTreeMap::new()),
        ready: Condvar::new(),
        application_readiness: Arc::new(Readiness::new()),
        diagnostic_readiness: Arc::new(Readiness::new()),
        stop_requested: AtomicBool::new(false),
    });
    (
        PrnsHost {
            shared: Arc::clone(&shared),
            native: Mutex::new(None),
            identity_hash: IdentityHash::new([0; 16]),
            destination_hashes: Vec::new(),
        },
        HostPublisher { shared },
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn status(value: AbiStatus) -> u32 {
    value as u32
}

fn catch_status(run: impl FnOnce() -> u32) -> u32 {
    catch_unwind(AssertUnwindSafe(run)).unwrap_or(status(AbiStatus::Panic))
}

fn bytes_view(bytes: &[u8]) -> PrnsByteView {
    PrnsByteView {
        data: bytes.as_ptr(),
        length: bytes.len(),
    }
}

fn string_view(value: &str) -> PrnsStringView {
    PrnsStringView {
        data: value.as_ptr(),
        length: value.len(),
    }
}

unsafe fn required_ref<'a, T>(value: *const T) -> Result<&'a T, u32> {
    unsafe { value.as_ref() }.ok_or(status(AbiStatus::InvalidArgument))
}

unsafe fn required_mut<'a, T>(value: *mut T) -> Result<&'a mut T, u32> {
    unsafe { value.as_mut() }.ok_or(status(AbiStatus::InvalidArgument))
}

unsafe fn read_string<'a>(value: PrnsStringView) -> Result<&'a str, u32> {
    if value.length > isize::MAX as usize || value.data.is_null() && value.length != 0 {
        return Err(status(AbiStatus::InvalidArgument));
    }
    let bytes = if value.length == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(value.data, value.length) }
    };
    str::from_utf8(bytes).map_err(|_| status(AbiStatus::InvalidArgument))
}

unsafe fn read_bytes<'a>(value: PrnsByteView) -> Result<&'a [u8], u32> {
    if value.length > isize::MAX as usize || value.data.is_null() && value.length != 0 {
        return Err(status(AbiStatus::InvalidArgument));
    }
    Ok(if value.length == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(value.data, value.length) }
    })
}

unsafe fn read_array<'a, T>(data: *const T, length: usize) -> Result<&'a [T], u32> {
    let byte_length = match length.checked_mul(size_of::<T>()) {
        Some(byte_length) => byte_length,
        None => return Err(status(AbiStatus::InvalidArgument)),
    };
    if byte_length > isize::MAX as usize || data.is_null() && length != 0 {
        return Err(status(AbiStatus::InvalidArgument));
    }
    Ok(if length == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(data, length) }
    })
}

unsafe fn read_fixed<const N: usize>(value: PrnsByteView) -> Result<[u8; N], u32> {
    unsafe { read_bytes(value) }?
        .try_into()
        .map_err(|_| status(AbiStatus::InvalidArgument))
}

fn validate_size(actual: usize, required: usize) -> Result<(), u32> {
    if actual < required {
        Err(status(AbiStatus::InvalidArgument))
    } else {
        Ok(())
    }
}

unsafe fn read_host_options(value: *const PrnsHostOptions) -> Result<HostOptionsInput, u32> {
    if value.is_null() {
        return Err(status(AbiStatus::InvalidArgument));
    }
    let struct_size = unsafe { value.cast::<usize>().read() };
    if struct_size < size_of::<PrnsHostOptions>() {
        return Err(status(AbiStatus::InvalidArgument));
    }
    let value = unsafe { value.read() };
    Ok(HostOptionsInput {
        required_abi: value.required_abi,
        required_schema_version: value.required_schema_version,
        required_product_version: value.required_product_version,
        limits: value.limits,
        role: value.role,
        identity: value.identity,
        destinations: value.destinations,
        destination_count: value.destination_count,
        required_capabilities: value.required_capabilities,
        required_capability_count: value.required_capability_count,
        persistence: value.persistence,
    })
}

unsafe fn parse_identity(config: &PrnsIdentityConfig) -> Result<IdentityConfig, u32> {
    validate_size(config.struct_size, size_of::<PrnsIdentityConfig>())?;
    match AbiIdentityConfigKind::try_from(config.kind)
        .map_err(|_| status(AbiStatus::InvalidArgument))?
    {
        AbiIdentityConfigKind::Existing => {
            Ok(IdentityConfig::Existing(IdentitySecret::new(unsafe {
                read_fixed(config.secret)
            }?)))
        }
        AbiIdentityConfigKind::GenerateEphemeral => Ok(IdentityConfig::GenerateEphemeral),
        AbiIdentityConfigKind::LoadOrCreate => Ok(IdentityConfig::LoadOrCreate {
            path: unsafe { read_string(config.path) }?.to_string(),
        }),
    }
}

unsafe fn parse_persistence(config: PrnsPersistenceConfig) -> Result<PersistenceConfig, u32> {
    validate_size(config.struct_size, size_of::<PrnsPersistenceConfig>())?;
    match AbiPersistenceConfigKind::try_from(config.kind)
        .map_err(|_| status(AbiStatus::InvalidArgument))?
    {
        AbiPersistenceConfigKind::Ephemeral => Ok(PersistenceConfig::Ephemeral),
        AbiPersistenceConfigKind::Directory => Ok(PersistenceConfig::Directory {
            path: unsafe { read_string(config.path) }?.to_string(),
        }),
    }
}

fn parse_capability(value: u32) -> Result<Capability, u32> {
    match AbiCapability::try_from(value).map_err(|_| status(AbiStatus::InvalidArgument))? {
        AbiCapability::Loopback => Ok(Capability::Loopback),
        AbiCapability::TcpClient => Ok(Capability::TcpClient),
        AbiCapability::TcpServer => Ok(Capability::TcpServer),
        AbiCapability::Udp => Ok(Capability::Udp),
        AbiCapability::Serial => Ok(Capability::Serial),
        AbiCapability::Usb => Ok(Capability::Usb),
        AbiCapability::Bluetooth => Ok(Capability::Bluetooth),
        AbiCapability::Wifi => Ok(Capability::Wifi),
        AbiCapability::WebSocket => Ok(Capability::WebSocket),
        AbiCapability::BrowserRendezvous => Ok(Capability::BrowserRendezvous),
        AbiCapability::I2p => Ok(Capability::I2p),
        AbiCapability::Weave => Ok(Capability::Weave),
        AbiCapability::SuppliedPipe => Ok(Capability::SuppliedPipe),
    }
}

unsafe fn parse_destination_name(value: &PrnsDestinationName) -> Result<DestinationName, u32> {
    validate_size(value.struct_size, size_of::<PrnsDestinationName>())?;
    let app_name = unsafe { read_string(value.app_name) }?.to_string();
    let aspects = unsafe { read_array(value.aspects, value.aspect_count) }?
        .iter()
        .map(|aspect| unsafe { read_string(*aspect) }.map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?;
    DestinationName::try_new(app_name, aspects).map_err(|_| status(AbiStatus::InvalidArgument))
}

unsafe fn parse_destination(value: &PrnsDestinationConfig) -> Result<DestinationConfig, u32> {
    validate_size(value.struct_size, size_of::<PrnsDestinationConfig>())?;
    let name = unsafe { parse_destination_name(&value.name) }?;
    match AbiDestinationConfigKind::try_from(value.kind)
        .map_err(|_| status(AbiStatus::InvalidArgument))?
    {
        AbiDestinationConfigKind::Plain => Ok(DestinationConfig::Plain(name)),
        AbiDestinationConfigKind::Single => {
            let identity = match AbiDestinationIdentityConfigKind::try_from(value.identity_kind)
                .map_err(|_| status(AbiStatus::InvalidArgument))?
            {
                AbiDestinationIdentityConfigKind::HostIdentity => {
                    DestinationIdentityConfig::HostIdentity
                }
                AbiDestinationIdentityConfigKind::DedicatedIdentity => {
                    DestinationIdentityConfig::Dedicated(unsafe {
                        parse_identity(&value.dedicated_identity)
                    }?)
                }
            };
            let request_handlers =
                unsafe { read_array(value.request_handlers, value.request_handler_count) }?
                    .iter()
                    .map(|handler| {
                        validate_size(handler.struct_size, size_of::<PrnsRequestHandlerConfig>())?;
                        let path = unsafe { read_string(handler.path) }?.to_string();
                        let policy = match AbiRequestPolicy::try_from(handler.policy)
                            .map_err(|_| status(AbiStatus::InvalidArgument))?
                        {
                            AbiRequestPolicy::AllowNone => RequestPolicy::AllowNone,
                            AbiRequestPolicy::AllowAll => RequestPolicy::AllowAll,
                            AbiRequestPolicy::AllowList => RequestPolicy::AllowList,
                        };
                        Ok(RequestHandlerConfig { path, policy })
                    })
                    .collect::<Result<Vec<_>, u32>>()?;
            Ok(DestinationConfig::Single(
                prns_host_core::SingleDestinationConfig {
                    name,
                    identity,
                    announce_app_data: unsafe { read_bytes(value.announce_app_data) }?.to_vec(),
                    maximum_request_bytes: optional_safe_uint(
                        value.has_maximum_request_bytes,
                        value.maximum_request_bytes,
                    )?,
                    proof: prns_host_core::DestinationProofStrategy::ProveAll,
                    link_requests: prns_host_core::DestinationLinkRequestPolicy::AcceptAll,
                    ratchet: prns_host_core::DestinationRatchetPolicy::NoRatchets,
                    resource_strategy: prns_host_core::ResourceStrategy::Refuse,
                    request_handlers,
                },
            ))
        }
    }
}

fn parse_bitrate(kind: u32, bits_per_second: u64) -> Result<Bitrate, u32> {
    match AbiBitrateKind::try_from(kind).map_err(|_| status(AbiStatus::InvalidArgument))? {
        AbiBitrateKind::Auto => Ok(Bitrate::Auto),
        AbiBitrateKind::BitsPerSecond if bits_per_second >= 5 => {
            Ok(Bitrate::BitsPerSecond(bits_per_second))
        }
        AbiBitrateKind::BitsPerSecond => Err(status(AbiStatus::InvalidArgument)),
    }
}

fn optional_safe_uint(has_value: u8, value: u64) -> Result<Option<u64>, u32> {
    if has_value == 0 {
        Ok(None)
    } else if value <= SAFE_UINT_MAX {
        Ok(Some(value))
    } else {
        Err(status(AbiStatus::InvalidArgument))
    }
}

unsafe fn optional_safe_uint_pointer(value: *const u64) -> Result<Option<u64>, u32> {
    if value.is_null() {
        Ok(None)
    } else {
        optional_safe_uint(1, unsafe { *value })
    }
}

unsafe fn read_strings(data: *const PrnsStringView, length: usize) -> Result<Vec<String>, u32> {
    unsafe { read_array(data, length) }?
        .iter()
        .map(|value| unsafe { read_string(*value) }.map(str::to_string))
        .collect()
}

unsafe fn optional_string(has_value: u8, value: PrnsStringView) -> Result<Option<String>, u32> {
    if has_value == 0 {
        Ok(None)
    } else {
        Ok(Some(unsafe { read_string(value) }?.to_string()))
    }
}

fn parse_discovery_scope(value: u32) -> Result<DiscoveryScope, u32> {
    match AbiDiscoveryScope::try_from(value).map_err(|_| status(AbiStatus::InvalidArgument))? {
        AbiDiscoveryScope::Link => Ok(DiscoveryScope::Link),
        AbiDiscoveryScope::Admin => Ok(DiscoveryScope::Admin),
        AbiDiscoveryScope::Site => Ok(DiscoveryScope::Site),
        AbiDiscoveryScope::Organization => Ok(DiscoveryScope::Organization),
        AbiDiscoveryScope::Global => Ok(DiscoveryScope::Global),
    }
}

fn parse_multicast_address_type(value: u32) -> Result<MulticastAddressType, u32> {
    match AbiMulticastAddressType::try_from(value)
        .map_err(|_| status(AbiStatus::InvalidArgument))?
    {
        AbiMulticastAddressType::Temporary => Ok(MulticastAddressType::Temporary),
        AbiMulticastAddressType::Permanent => Ok(MulticastAddressType::Permanent),
    }
}

fn parse_serial_line(value: &PrnsSerialLineConfig) -> Result<SerialLineConfig, u32> {
    validate_size(value.struct_size, size_of::<PrnsSerialLineConfig>())?;
    let data_bits = match AbiSerialDataBits::try_from(value.data_bits)
        .map_err(|_| status(AbiStatus::InvalidArgument))?
    {
        AbiSerialDataBits::Five => SerialDataBits::Five,
        AbiSerialDataBits::Six => SerialDataBits::Six,
        AbiSerialDataBits::Seven => SerialDataBits::Seven,
        AbiSerialDataBits::Eight => SerialDataBits::Eight,
    };
    let parity = match AbiSerialParity::try_from(value.parity)
        .map_err(|_| status(AbiStatus::InvalidArgument))?
    {
        AbiSerialParity::None => SerialParity::None,
        AbiSerialParity::Even => SerialParity::Even,
        AbiSerialParity::Odd => SerialParity::Odd,
    };
    let stop_bits = match AbiSerialStopBits::try_from(value.stop_bits)
        .map_err(|_| status(AbiStatus::InvalidArgument))?
    {
        AbiSerialStopBits::One => SerialStopBits::One,
        AbiSerialStopBits::Two => SerialStopBits::Two,
    };
    Ok(SerialLineConfig {
        baud: value.baud,
        data_bits,
        parity,
        stop_bits,
    })
}

fn parse_rnode_radio(value: &PrnsRNodeRadioConfig) -> Result<RNodeRadioConfig, u32> {
    validate_size(value.struct_size, size_of::<PrnsRNodeRadioConfig>())?;
    Ok(RNodeRadioConfig {
        frequency_hz: value.frequency_hz,
        bandwidth_hz: value.bandwidth_hz,
        tx_power_dbm: value.tx_power_dbm,
        spreading_factor: value.spreading_factor,
        coding_rate: value.coding_rate,
    })
}

unsafe fn parse_interface_config(value: &PrnsInterfaceConfig) -> Result<InterfaceConfig, u32> {
    validate_size(value.struct_size, size_of::<PrnsInterfaceConfig>())?;
    let kind =
        AbiInterfaceKind::try_from(value.kind).map_err(|_| status(AbiStatus::InvalidArgument))?;
    let optional_scope = || {
        if value.has_discovery_scope == 0 {
            Ok(None)
        } else {
            parse_discovery_scope(value.discovery_scope).map(Some)
        }
    };
    let optional_multicast = || {
        if value.has_multicast_address_type == 0 {
            Ok(None)
        } else {
            parse_multicast_address_type(value.multicast_address_type).map(Some)
        }
    };
    let optional_station =
        || unsafe { optional_string(value.has_station_callsign, value.station_callsign) };
    let optional_interval =
        || (value.has_station_interval_seconds != 0).then_some(value.station_interval_seconds);
    match kind {
        AbiInterfaceKind::AutoLan => Ok(InterfaceConfig::AutoLan {
            group_id: unsafe { optional_string(value.has_group_id, value.group_id) }?,
            discovery_scope: optional_scope()?,
            discovery_port: (value.has_discovery_port != 0).then_some(value.discovery_port),
            data_port: (value.has_data_port != 0).then_some(value.data_port),
            devices: unsafe { read_strings(value.devices, value.device_count) }?,
            ignored_devices: unsafe {
                read_strings(value.ignored_devices, value.ignored_device_count)
            }?,
            multicast_address_type: optional_multicast()?,
        }),
        AbiInterfaceKind::TcpClient => Ok(InterfaceConfig::TcpClient {
            target: unsafe { read_string(value.target) }?.to_string(),
            bitrate: parse_bitrate(value.bitrate_kind, value.bitrate_bps)?,
        }),
        AbiInterfaceKind::TcpServer => Ok(InterfaceConfig::TcpServer {
            bind: unsafe { read_string(value.bind) }?.to_string(),
            bitrate: parse_bitrate(value.bitrate_kind, value.bitrate_bps)?,
        }),
        AbiInterfaceKind::Udp => Ok(InterfaceConfig::Udp {
            local: unsafe { read_string(value.local) }?.to_string(),
            peer: unsafe { read_string(value.peer) }?.to_string(),
            bitrate: parse_bitrate(value.bitrate_kind, value.bitrate_bps)?,
        }),
        AbiInterfaceKind::Serial => Ok(InterfaceConfig::Serial {
            port: unsafe { read_string(value.port) }?.to_string(),
            line: parse_serial_line(&value.line)?,
        }),
        AbiInterfaceKind::Kiss => Ok(InterfaceConfig::Kiss {
            port: unsafe { read_string(value.port) }?.to_string(),
            line: parse_serial_line(&value.line)?,
            flow_control: value.flow_control != 0,
            preamble_millis: value.preamble_millis,
            transmit_tail_millis: value.transmit_tail_millis,
            persistence: value.persistence,
            slot_time_millis: value.slot_time_millis,
            station_callsign: optional_station()?,
            station_interval_seconds: optional_interval(),
        }),
        AbiInterfaceKind::Ax25Kiss => Ok(InterfaceConfig::Ax25Kiss {
            port: unsafe { read_string(value.port) }?.to_string(),
            line: parse_serial_line(&value.line)?,
            flow_control: value.flow_control != 0,
            preamble_millis: value.preamble_millis,
            transmit_tail_millis: value.transmit_tail_millis,
            persistence: value.persistence,
            slot_time_millis: value.slot_time_millis,
            callsign: unsafe { read_string(value.callsign) }?.to_string(),
            ssid: value.ssid,
        }),
        AbiInterfaceKind::RNode => Ok(InterfaceConfig::RNode {
            port: unsafe { read_string(value.port) }?.to_string(),
            radio: parse_rnode_radio(&value.radio)?,
            flow_control: value.flow_control != 0,
            station_callsign: optional_station()?,
            station_interval_seconds: optional_interval(),
            airtime_limit_short_centi_percent: (value.has_airtime_limit_short_centi_percent != 0)
                .then_some(value.airtime_limit_short_centi_percent),
            airtime_limit_long_centi_percent: (value.has_airtime_limit_long_centi_percent != 0)
                .then_some(value.airtime_limit_long_centi_percent),
        }),
        AbiInterfaceKind::MultiRNode => {
            let members = unsafe { read_array(value.members, value.member_count) }?
                .iter()
                .map(|member| {
                    validate_size(member.struct_size, size_of::<PrnsMultiRNodeMemberConfig>())?;
                    Ok(MultiRNodeMemberConfig {
                        name: unsafe { read_string(member.name) }?.to_string(),
                        virtual_port: member.virtual_port,
                        radio: parse_rnode_radio(&member.radio)?,
                        flow_control: member.flow_control != 0,
                        outgoing: member.outgoing != 0,
                    })
                })
                .collect::<Result<Vec<_>, u32>>()?;
            Ok(InterfaceConfig::MultiRNode {
                port: unsafe { read_string(value.port) }?.to_string(),
                station_callsign: optional_station()?,
                station_interval_seconds: optional_interval(),
                members,
            })
        }
        AbiInterfaceKind::Pipe => Ok(InterfaceConfig::Pipe {
            command: unsafe { read_strings(value.command, value.command_count) }?,
            respawn_delay_millis: value.respawn_delay_millis,
        }),
        AbiInterfaceKind::BackboneClient => Ok(InterfaceConfig::BackboneClient {
            target: unsafe { read_string(value.target) }?.to_string(),
            bitrate: parse_bitrate(value.bitrate_kind, value.bitrate_bps)?,
        }),
        AbiInterfaceKind::BackboneServer => Ok(InterfaceConfig::BackboneServer {
            bind: unsafe { read_string(value.bind) }?.to_string(),
            bitrate: parse_bitrate(value.bitrate_kind, value.bitrate_bps)?,
        }),
        AbiInterfaceKind::I2p => Ok(InterfaceConfig::I2p {
            peers: unsafe { read_strings(value.peers, value.peer_count) }?,
            connectable: value.connectable != 0,
        }),
        AbiInterfaceKind::Weave => Ok(InterfaceConfig::Weave {
            port: unsafe { read_string(value.port) }?.to_string(),
        }),
        AbiInterfaceKind::AutomaticUsb => Ok(InterfaceConfig::AutomaticUsb),
        AbiInterfaceKind::AutomaticBluetoothLe => Ok(InterfaceConfig::AutomaticBluetoothLe),
        AbiInterfaceKind::WebSocketClient => Ok(InterfaceConfig::WebSocketClient {
            target: unsafe { read_string(value.target) }?.to_string(),
            framing: parse_websocket_framing_selection(value.websocket_framing_selection)?,
        }),
        AbiInterfaceKind::WebSocketServer => Ok(InterfaceConfig::WebSocketServer {
            bind: unsafe { read_string(value.bind) }?.to_string(),
            framing: parse_websocket_framing_selection(value.websocket_framing_selection)?,
        }),
        AbiInterfaceKind::BrowserRendezvous => Ok(InterfaceConfig::BrowserRendezvous {
            url: unsafe { read_string(value.url) }?.to_string(),
        }),
    }
}

fn parse_websocket_framing_selection(value: u32) -> Result<WebSocketFramingSelection, u32> {
    WebSocketFramingSelection::try_from(value).map_err(|_| status(AbiStatus::InvalidArgument))
}

fn parse_interface_routing_policy(
    value: &PrnsInterfaceRoutingPolicy,
) -> Result<InterfaceRoutingPolicy, u32> {
    validate_size(value.struct_size, size_of::<PrnsInterfaceRoutingPolicy>())?;
    let mode = if value.has_mode == 0 {
        None
    } else {
        Some(InterfaceMode::try_from(value.mode).map_err(|_| status(AbiStatus::InvalidArgument))?)
    };
    let gravity = if value.has_gravity == 0 {
        None
    } else if (SAFE_INT_MIN..=SAFE_INT_MAX).contains(&value.gravity) {
        Some(value.gravity)
    } else {
        return Err(status(AbiStatus::InvalidArgument));
    };
    Ok(InterfaceRoutingPolicy {
        mode,
        gravity,
        recursive_path_requests: (value.has_recursive_path_requests != 0)
            .then_some(value.recursive_path_requests != 0),
        announces_from_internal: (value.has_announces_from_internal != 0)
            .then_some(value.announces_from_internal != 0),
        announces_to_internal: (value.has_announces_to_internal != 0)
            .then_some(value.announces_to_internal != 0),
    })
}

fn parse_response_timeout(kind: u32, millis: u64) -> Result<ResponseTimeout, u32> {
    match AbiResponseTimeoutKind::try_from(kind).map_err(|_| status(AbiStatus::InvalidArgument))? {
        AbiResponseTimeoutKind::LinkDefault => Ok(ResponseTimeout::LinkDefault),
        AbiResponseTimeoutKind::Exact => Ok(ResponseTimeout::Exact { millis }),
    }
}

fn parse_resource_compression(kind: u32) -> Result<ResourceCompression, u32> {
    match AbiResourceCompressionKind::try_from(kind)
        .map_err(|_| status(AbiStatus::InvalidArgument))?
    {
        AbiResourceCompressionKind::Auto => Ok(ResourceCompression::Auto),
        AbiResourceCompressionKind::Never => Ok(ResourceCompression::Never),
    }
}

fn parse_resource_strategy(
    kind: u32,
    maximum_uncompressed_bytes: u64,
    accept_compressed: u8,
) -> Result<ResourceStrategy, u32> {
    match AbiResourceStrategyKind::try_from(kind).map_err(|_| status(AbiStatus::InvalidArgument))? {
        AbiResourceStrategyKind::Refuse => Ok(ResourceStrategy::Refuse),
        AbiResourceStrategyKind::Accept if maximum_uncompressed_bytes > 0 => {
            Ok(ResourceStrategy::Accept {
                maximum_uncompressed_bytes,
                accept_compressed: accept_compressed != 0,
            })
        }
        AbiResourceStrategyKind::Accept => Err(status(AbiStatus::InvalidArgument)),
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_contract_info(out_info: *mut PrnsContractInfo) -> u32 {
    catch_status(|| {
        let out = match unsafe { required_mut(out_info) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        if let Err(error) = validate_size(out.struct_size, size_of::<PrnsContractInfo>()) {
            return error;
        }
        out.abi = HOST_CONTRACT.abi;
        out.schema_version = HOST_SCHEMA_VERSION;
        out.product_version = string_view(HOST_CONTRACT.product_version);
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_backend_info(out_info: *mut PrnsBackendInfo) -> u32 {
    catch_status(|| {
        let out = match unsafe { required_mut(out_info) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        if let Err(error) = validate_size(out.struct_size, size_of::<PrnsBackendInfo>()) {
            return error;
        }
        out.backend = AbiBackendKind::Native as u32;
        out.capabilities = NATIVE_CAPABILITIES.as_ptr();
        out.capability_count = NATIVE_CAPABILITIES.len();
        out.interface_kinds = NATIVE_INTERFACE_KINDS.as_ptr();
        out.interface_kind_count = NATIVE_INTERFACE_KINDS.len();
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_create(
    options: *const PrnsHostOptions,
    out_host: *mut *mut PrnsHost,
) -> u32 {
    catch_status(|| {
        let options = match unsafe { read_host_options(options) } {
            Ok(options) => options,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_host) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        if let Err(error) = validate_size(options.limits.struct_size, size_of::<PrnsLimits>()) {
            return error;
        }
        let version = match unsafe { read_string(options.required_product_version) } {
            Ok(version) => version,
            Err(error) => return error,
        };
        if verify_host_contract(
            options.required_abi,
            options.required_schema_version,
            version,
        )
        .is_err()
        {
            return status(AbiStatus::ContractMismatch);
        }
        let limits = match CoreLimits::try_new(
            options.limits.pending_commands,
            options.limits.application_events,
            options.limits.retained_event_bytes,
            options.limits.diagnostics,
        ) {
            Ok(limits) => limits,
            Err(_) => return status(AbiStatus::InvalidArgument),
        };
        let role = match AbiHostRole::try_from(options.role) {
            Ok(AbiHostRole::Endpoint) => HostRole::Endpoint,
            Ok(AbiHostRole::Transport) => HostRole::Transport,
            Err(_) => return status(AbiStatus::InvalidArgument),
        };
        let identity = match unsafe { parse_identity(&options.identity) } {
            Ok(identity) => identity,
            Err(error) => return error,
        };
        let persistence = match unsafe { parse_persistence(options.persistence) } {
            Ok(persistence) => persistence,
            Err(error) => return error,
        };
        let destination_values =
            match unsafe { read_array(options.destinations, options.destination_count) } {
                Ok(values) => values,
                Err(error) => return error,
            };
        let destinations = match destination_values
            .iter()
            .map(|destination| unsafe { parse_destination(destination) })
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(destinations) => destinations,
            Err(error) => return error,
        };
        let capability_values = match unsafe {
            read_array(
                options.required_capabilities,
                options.required_capability_count,
            )
        } {
            Ok(values) => values,
            Err(error) => return error,
        };
        let required_capabilities = match capability_values
            .iter()
            .copied()
            .map(parse_capability)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(capabilities) => capabilities,
            Err(error) => return error,
        };
        let (mut host, publisher) = host_capsule_with_phase(limits, false);
        let native = match NativeHost::start(
            HostConfig {
                identity,
                persistence,
                role,
                destinations,
                required_capabilities,
                limits,
            },
            Arc::new(publisher),
        ) {
            Ok(native) => native,
            Err(error) => return native_start_status(error),
        };
        host.identity_hash = native.identity_hash();
        host.destination_hashes = native.destination_hashes().to_vec();
        *lock(&host.native) = Some(native);
        *out = Box::into_raw(Box::new(host));
        status(AbiStatus::Ok)
    })
}

fn native_start_status(error: NativeStartError) -> u32 {
    match error {
        NativeStartError::MissingCapabilities(_) => status(AbiStatus::Unsupported),
        NativeStartError::Identity(IdentityStartError::PermissionDenied { .. })
        | NativeStartError::Persistence(PersistenceStartError::PermissionDenied { .. }) => {
            status(AbiStatus::PermissionDenied)
        }
        NativeStartError::Identity(IdentityStartError::Unavailable { .. })
        | NativeStartError::Persistence(PersistenceStartError::Unavailable { .. }) => {
            status(AbiStatus::Unavailable)
        }
        NativeStartError::Identity(
            IdentityStartError::Malformed { .. } | IdentityStartError::InvalidMaterial,
        )
        | NativeStartError::Destination(_)
        | NativeStartError::Persistence(PersistenceStartError::NotDirectory { .. }) => {
            status(AbiStatus::InvalidArgument)
        }
        NativeStartError::TimedOut => status(AbiStatus::TimedOut),
        NativeStartError::Identity(IdentityStartError::EntropyUnavailable)
        | NativeStartError::Runtime(_)
        | NativeStartError::Thread(_) => status(AbiStatus::BackendFailed),
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_release(host: *mut PrnsHost) {
    if !host.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(host));
        }));
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_lifecycle(
    host: *const PrnsHost,
    out_lifecycle: *mut PrnsLifecycle,
) -> u32 {
    catch_status(|| {
        let host = match unsafe { required_ref(host) } {
            Ok(host) => host,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_lifecycle) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        if let Err(error) = validate_size(out.struct_size, size_of::<PrnsLifecycle>()) {
            return error;
        }
        let lifecycle = lock(&host.shared.queue).lifecycle();
        out.revision = lifecycle.revision;
        out.reason = 0;
        match lifecycle.state {
            LifecycleState::Starting => out.phase = AbiLifecyclePhase::Starting as u32,
            LifecycleState::Running => out.phase = AbiLifecyclePhase::Running as u32,
            LifecycleState::Stopping => out.phase = AbiLifecyclePhase::Stopping as u32,
            LifecycleState::Stopped(reason) => {
                out.phase = AbiLifecyclePhase::Stopped as u32;
                out.reason = match reason {
                    StopReason::Requested => AbiStopReason::Requested as u32,
                    StopReason::BackendExited => AbiStopReason::BackendExited as u32,
                };
            }
            LifecycleState::Failed(_) => out.phase = AbiLifecyclePhase::Failed as u32,
        }
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_identity_hash(
    host: *const PrnsHost,
    out_hash: *mut PrnsByteView,
) -> u32 {
    catch_status(|| {
        let host = match unsafe { required_ref(host) } {
            Ok(host) => host,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_hash) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = bytes_view(host.identity_hash.as_bytes());
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_destination_count(host: *const PrnsHost) -> usize {
    catch_unwind(AssertUnwindSafe(|| unsafe { host.as_ref() }))
        .ok()
        .flatten()
        .map_or(0, |host| host.destination_hashes.len())
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_destination_hash(
    host: *const PrnsHost,
    index: usize,
    out_hash: *mut PrnsByteView,
) -> u32 {
    catch_status(|| {
        let host = match unsafe { required_ref(host) } {
            Ok(host) => host,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_hash) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        let Some(hash) = host.destination_hashes.get(index) else {
            return status(AbiStatus::InvalidArgument);
        };
        *out = bytes_view(hash.as_bytes());
        status(AbiStatus::Ok)
    })
}

impl PrnsHostInspection {
    fn new(snapshot: CoreHostSnapshot) -> Box<Self> {
        let mut inspection = Box::new(Self {
            snapshot,
            interfaces: Vec::new(),
            routes: Vec::new(),
            destination_identities: Vec::new(),
        });
        inspection.interfaces = inspection
            .snapshot
            .interfaces
            .iter()
            .map(|interface| PrnsInterfaceSnapshot {
                struct_size: size_of::<PrnsInterfaceSnapshot>(),
                interface_id: bytes_view(interface.interface_id.as_bytes()),
                has_name: u8::from(interface.name.is_some()),
                name: interface
                    .name
                    .as_deref()
                    .map_or_else(|| string_view(""), string_view),
                has_kind: u8::from(interface.kind.is_some()),
                kind: interface.kind.map_or(0, interface_kind_value),
                health: interface_health_value(interface.health),
                has_failure_detail: u8::from(interface.failure_detail.is_some()),
                failure_detail: interface
                    .failure_detail
                    .as_deref()
                    .map_or_else(|| string_view(""), string_view),
                rx_bytes: interface.rx_bytes,
                tx_bytes: interface.tx_bytes,
                has_rx_bps: u8::from(interface.rx_bps.is_some()),
                rx_bps: interface.rx_bps.unwrap_or_default(),
                has_tx_bps: u8::from(interface.tx_bps.is_some()),
                tx_bps: interface.tx_bps.unwrap_or_default(),
                route_count: interface.route_count,
                link_count: interface.link_count,
                transported_link_count: interface.transported_link_count,
            })
            .collect();
        inspection.routes = inspection
            .snapshot
            .routes
            .iter()
            .map(|route| PrnsRouteSnapshot {
                struct_size: size_of::<PrnsRouteSnapshot>(),
                destination: bytes_view(route.destination.as_bytes()),
                hops: route.hops,
                has_via_identity: u8::from(route.via_identity.is_some()),
                via_identity: route.via_identity.as_ref().map_or_else(
                    || bytes_view(&[]),
                    |identity| bytes_view(identity.as_bytes()),
                ),
                interface_id: bytes_view(route.interface_id.as_bytes()),
                learned_at_millis: route.learned_at_millis,
                last_route_activity_at_millis: route.last_route_activity_at_millis,
                expires_at_millis: route.expires_at_millis,
            })
            .collect();
        inspection.destination_identities = inspection
            .snapshot
            .destination_identities
            .iter()
            .map(|identity| PrnsDestinationIdentitySnapshot {
                struct_size: size_of::<PrnsDestinationIdentitySnapshot>(),
                destination: bytes_view(identity.destination.as_bytes()),
                identity: bytes_view(identity.identity.as_bytes()),
            })
            .collect();
        inspection
    }
}

fn interface_kind_value(kind: InterfaceKind) -> u32 {
    match kind {
        InterfaceKind::AutoLan => AbiInterfaceKind::AutoLan as u32,
        InterfaceKind::TcpClient => AbiInterfaceKind::TcpClient as u32,
        InterfaceKind::TcpServer => AbiInterfaceKind::TcpServer as u32,
        InterfaceKind::Udp => AbiInterfaceKind::Udp as u32,
        InterfaceKind::Serial => AbiInterfaceKind::Serial as u32,
        InterfaceKind::Kiss => AbiInterfaceKind::Kiss as u32,
        InterfaceKind::Ax25Kiss => AbiInterfaceKind::Ax25Kiss as u32,
        InterfaceKind::RNode => AbiInterfaceKind::RNode as u32,
        InterfaceKind::MultiRNode => AbiInterfaceKind::MultiRNode as u32,
        InterfaceKind::Pipe => AbiInterfaceKind::Pipe as u32,
        InterfaceKind::BackboneClient => AbiInterfaceKind::BackboneClient as u32,
        InterfaceKind::BackboneServer => AbiInterfaceKind::BackboneServer as u32,
        InterfaceKind::I2p => AbiInterfaceKind::I2p as u32,
        InterfaceKind::Weave => AbiInterfaceKind::Weave as u32,
        InterfaceKind::AutomaticUsb => AbiInterfaceKind::AutomaticUsb as u32,
        InterfaceKind::AutomaticBluetoothLe => AbiInterfaceKind::AutomaticBluetoothLe as u32,
        InterfaceKind::WebSocketClient => AbiInterfaceKind::WebSocketClient as u32,
        InterfaceKind::WebSocketServer => AbiInterfaceKind::WebSocketServer as u32,
        InterfaceKind::BrowserRendezvous => AbiInterfaceKind::BrowserRendezvous as u32,
    }
}

fn interface_health_value(health: InterfaceHealth) -> u32 {
    use prns_host_core::InterfaceHealth as AbiInterfaceHealth;
    match health {
        InterfaceHealth::Initializing => AbiInterfaceHealth::Initializing as u32,
        InterfaceHealth::Connected => AbiInterfaceHealth::Connected as u32,
        InterfaceHealth::Degraded => AbiInterfaceHealth::Degraded as u32,
        InterfaceHealth::Reconnecting => AbiInterfaceHealth::Reconnecting as u32,
        InterfaceHealth::Failed => AbiInterfaceHealth::Failed as u32,
        InterfaceHealth::Disconnected => AbiInterfaceHealth::Disconnected as u32,
        InterfaceHealth::Disabled => AbiInterfaceHealth::Disabled as u32,
        InterfaceHealth::Unknown => AbiInterfaceHealth::Unknown as u32,
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_snapshot(
    host: *const PrnsHost,
    timeout_millis: u32,
    out_snapshot: *mut *mut PrnsHostInspection,
) -> u32 {
    catch_status(|| {
        let host = match unsafe { required_ref(host) } {
            Ok(host) => host,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_snapshot) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        let native = lock(&host.native);
        let Some(native) = native.as_ref() else {
            return status(AbiStatus::Stopped);
        };
        let timeout = if timeout_millis == NEVER_TIMEOUT {
            None
        } else {
            Some(Duration::from_millis(u64::from(timeout_millis)))
        };
        let snapshot = match native.snapshot(timeout) {
            Ok(snapshot) => snapshot,
            Err(NativeSnapshotError::Busy) => return status(AbiStatus::QueueFull),
            Err(NativeSnapshotError::Stopped) => return status(AbiStatus::Stopped),
            Err(NativeSnapshotError::TimedOut) => return status(AbiStatus::TimedOut),
        };
        *out = Box::into_raw(PrnsHostInspection::new(snapshot));
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_snapshot_read(
    snapshot: *const PrnsHostInspection,
    out_snapshot: *mut PrnsHostSnapshot,
) -> u32 {
    catch_status(|| {
        let snapshot = match unsafe { required_ref(snapshot) } {
            Ok(snapshot) => snapshot,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_snapshot) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        if let Err(error) = validate_size(out.struct_size, size_of::<PrnsHostSnapshot>()) {
            return error;
        }
        let runtime = snapshot.snapshot.runtime;
        let persistence = &snapshot.snapshot.persistence;
        *out = PrnsHostSnapshot {
            struct_size: size_of::<PrnsHostSnapshot>(),
            revision: snapshot.snapshot.revision,
            backend: PrnsBackendInfo {
                struct_size: size_of::<PrnsBackendInfo>(),
                backend: AbiBackendKind::Native as u32,
                capabilities: NATIVE_CAPABILITIES.as_ptr(),
                capability_count: NATIVE_CAPABILITIES.len(),
                interface_kinds: NATIVE_INTERFACE_KINDS.as_ptr(),
                interface_kind_count: NATIVE_INTERFACE_KINDS.len(),
            },
            interfaces: snapshot.interfaces.as_ptr(),
            interface_count: snapshot.interfaces.len(),
            routes: snapshot.routes.as_ptr(),
            route_count: snapshot.routes.len(),
            active_link_count: snapshot.snapshot.active_link_count,
            destination_identities: snapshot.destination_identities.as_ptr(),
            destination_identity_count: snapshot.destination_identities.len(),
            runtime: PrnsRuntimeHealthSnapshot {
                struct_size: size_of::<PrnsRuntimeHealthSnapshot>(),
                running: u8::from(runtime.running),
                uptime_millis: runtime.uptime_millis,
                interface_count: runtime.interface_count,
                online_interface_count: runtime.online_interface_count,
                route_count: runtime.route_count,
                link_count: runtime.link_count,
                transported_link_count: runtime.transported_link_count,
                rx_bytes: runtime.rx_bytes,
                tx_bytes: runtime.tx_bytes,
                rx_bps: runtime.rx_bps,
                tx_bps: runtime.tx_bps,
            },
            persistence: PrnsPersistenceSnapshot {
                struct_size: size_of::<PrnsPersistenceSnapshot>(),
                persistent: u8::from(persistence.persistent),
                restored: u8::from(persistence.restored),
                has_last_flush_cause: u8::from(persistence.last_flush_cause.is_some()),
                last_flush_cause: persistence
                    .last_flush_cause
                    .map_or(0, |cause| persistence_cause(cause) as u32),
                has_last_failure_detail: u8::from(persistence.last_failure_detail.is_some()),
                last_failure_detail: persistence
                    .last_failure_detail
                    .as_deref()
                    .map_or_else(|| string_view(""), string_view),
            },
        };
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_snapshot_release(snapshot: *mut PrnsHostInspection) {
    if !snapshot.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(snapshot));
        }));
    }
}

unsafe fn submit_host_command(
    host: *mut PrnsHost,
    command: HostCommand,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    let host = match unsafe { required_ref(host) } {
        Ok(host) => host,
        Err(error) => return error,
    };
    let out = match unsafe { required_mut(out_command) } {
        Ok(out) => out,
        Err(error) => return error,
    };
    *out = ptr::null_mut();
    let native = lock(&host.native);
    let Some(native) = native.as_ref() else {
        return status(AbiStatus::Stopped);
    };
    let readiness = Arc::new(Readiness::new());
    let weak_readiness = Arc::downgrade(&readiness);
    let command_readiness = Arc::new(move || {
        if let Some(readiness) = weak_readiness.upgrade() {
            readiness.notify();
        }
    });
    let handle = match native.submit_with_readiness(command, Some(command_readiness)) {
        Ok(handle) => handle,
        Err(NativeSubmitError::Busy) => return status(AbiStatus::QueueFull),
        Err(NativeSubmitError::Stopped) => return status(AbiStatus::Stopped),
    };
    *out = issued_command(handle, readiness);
    status(AbiStatus::Ok)
}

fn issued_command(handle: CommandHandle, readiness: Arc<Readiness>) -> *mut PrnsIssuedCommand {
    Box::into_raw(Box::new(PrnsIssuedCommand {
        handle,
        cached: Mutex::new(None),
        readiness,
        readiness_registration: Mutex::new(None),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_announce(
    host: *mut PrnsHost,
    destination: PrnsByteView,
    interface_id: *const PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let destination = match unsafe { read_fixed(destination) } {
            Ok(destination) => DestinationHash::new(destination),
            Err(error) => return error,
        };
        let interface = match unsafe { interface_id.as_ref() } {
            Some(interface) => match unsafe { read_fixed(*interface) } {
                Ok(interface) => Some(InterfaceId::new(interface)),
                Err(error) => return error,
            },
            None => None,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::Announce {
                    destination,
                    interface,
                },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_send_single_packet(
    host: *mut PrnsHost,
    destination: PrnsByteView,
    payload: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let destination = match unsafe { read_fixed(destination) } {
            Ok(destination) => DestinationHash::new(destination),
            Err(error) => return error,
        };
        let payload = match unsafe { read_bytes(payload) } {
            Ok(payload) => payload.to_vec(),
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::SendSinglePacket {
                    destination,
                    payload,
                },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_close_link(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        unsafe { submit_host_command(host, HostCommand::CloseLink { link_id }, out_command) }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_attach_tcp_server(
    host: *mut PrnsHost,
    bind: PrnsStringView,
    bitrate_kind: u32,
    bitrate_bps: u64,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let bind = match unsafe { read_string(bind) } {
            Ok(bind) => bind.to_string(),
            Err(error) => return error,
        };
        let bitrate = match parse_bitrate(bitrate_kind, bitrate_bps) {
            Ok(bitrate) => bitrate,
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::AttachTcpServer { bind, bitrate },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_attach_tcp_client(
    host: *mut PrnsHost,
    target: PrnsStringView,
    bitrate_kind: u32,
    bitrate_bps: u64,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let target = match unsafe { read_string(target) } {
            Ok(target) => target.to_string(),
            Err(error) => return error,
        };
        let bitrate = match parse_bitrate(bitrate_kind, bitrate_bps) {
            Ok(bitrate) => bitrate,
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::AttachTcpClient { target, bitrate },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_attach_udp(
    host: *mut PrnsHost,
    local: PrnsStringView,
    peer: PrnsStringView,
    bitrate_kind: u32,
    bitrate_bps: u64,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let local = match unsafe { read_string(local) } {
            Ok(local) => local.to_string(),
            Err(error) => return error,
        };
        let peer = match unsafe { read_string(peer) } {
            Ok(peer) => peer.to_string(),
            Err(error) => return error,
        };
        let bitrate = match parse_bitrate(bitrate_kind, bitrate_bps) {
            Ok(bitrate) => bitrate,
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::AttachUdp {
                    local,
                    peer,
                    bitrate,
                },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_attach_interface(
    host: *mut PrnsHost,
    config: *const PrnsInterfaceConfig,
    routing: *const PrnsInterfaceRoutingPolicy,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let config = match unsafe { required_ref(config) } {
            Ok(config) => config,
            Err(error) => return error,
        };
        let config = match unsafe { parse_interface_config(config) } {
            Ok(config) => config,
            Err(error) => return error,
        };
        let routing = if routing.is_null() {
            None
        } else {
            match unsafe { required_ref(routing) }.and_then(parse_interface_routing_policy) {
                Ok(routing) => Some(routing),
                Err(error) => return error,
            }
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::AttachInterface { config, routing },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_detach_interface(
    host: *mut PrnsHost,
    interface_id: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let interface = match unsafe { read_fixed(interface_id) } {
            Ok(interface) => InterfaceId::new(interface),
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::DetachInterface { interface },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_establish_link(
    host: *mut PrnsHost,
    destination: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let destination = match unsafe { read_fixed(destination) } {
            Ok(destination) => DestinationHash::new(destination),
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::EstablishLink { destination },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_request_path(
    host: *mut PrnsHost,
    destination: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let destination = match unsafe { read_fixed(destination) } {
            Ok(destination) => DestinationHash::new(destination),
            Err(error) => return error,
        };
        unsafe { submit_host_command(host, HostCommand::RequestPath { destination }, out_command) }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_identify(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    identity: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        let identity = match unsafe { read_fixed(identity) } {
            Ok(identity) => IdentityHash::new(identity),
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::Identify { link_id, identity },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_send_link_packet(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    payload: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        let payload = match unsafe { read_bytes(payload) } {
            Ok(payload) => payload.to_vec(),
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::SendLinkPacket { link_id, payload },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_request(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    path_hash: PrnsByteView,
    payload: PrnsByteView,
    timeout_kind: u32,
    timeout_millis: u64,
    maximum_response_bytes: *const u64,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        let path_hash = match unsafe { read_fixed(path_hash) } {
            Ok(path_hash) => RequestPathHash::new(path_hash),
            Err(error) => return error,
        };
        let payload = match unsafe { read_bytes(payload) } {
            Ok(payload) => payload.to_vec(),
            Err(error) => return error,
        };
        let timeout = match parse_response_timeout(timeout_kind, timeout_millis) {
            Ok(timeout) => timeout,
            Err(error) => return error,
        };
        let maximum_response_bytes =
            match unsafe { optional_safe_uint_pointer(maximum_response_bytes) } {
                Ok(maximum_response_bytes) => maximum_response_bytes,
                Err(error) => return error,
            };
        unsafe {
            submit_host_command(
                host,
                HostCommand::Request {
                    link_id,
                    path_hash,
                    payload,
                    timeout,
                    maximum_response_bytes,
                },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_respond(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    request_id: PrnsByteView,
    request_rtt_millis: u64,
    payload: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        let request_id = match unsafe { read_fixed(request_id) } {
            Ok(request_id) => RequestId::new(request_id),
            Err(error) => return error,
        };
        let payload = match unsafe { read_bytes(payload) } {
            Ok(payload) => payload.to_vec(),
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::Respond {
                    link_id,
                    request_id,
                    request_rtt_millis,
                    payload,
                },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_send_resource(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    payload: PrnsByteView,
    packed_metadata: *const PrnsByteView,
    compression_kind: u32,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        let payload = match unsafe { read_bytes(payload) } {
            Ok(payload) => payload.to_vec(),
            Err(error) => return error,
        };
        let packed_metadata = match unsafe { packed_metadata.as_ref() } {
            Some(metadata) => match unsafe { read_bytes(*metadata) } {
                Ok(metadata) => Some(metadata.to_vec()),
                Err(error) => return error,
            },
            None => None,
        };
        let compression = match parse_resource_compression(compression_kind) {
            Ok(compression) => compression,
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::SendResource {
                    link_id,
                    payload,
                    packed_metadata,
                    compression,
                },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_begin_resource_upload(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    declared_length: u64,
    packed_metadata: *const PrnsByteView,
    compression_kind: u32,
    out_upload: *mut *mut PrnsResourceUpload,
) -> u32 {
    catch_status(|| {
        let host = match unsafe { required_ref(host) } {
            Ok(host) => host,
            Err(error) => return error,
        };
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        let packed_metadata = match unsafe { packed_metadata.as_ref() } {
            Some(metadata) => match unsafe { read_bytes(*metadata) } {
                Ok(metadata) => Some(metadata.to_vec()),
                Err(error) => return error,
            },
            None => None,
        };
        let compression = match parse_resource_compression(compression_kind) {
            Ok(compression) => compression,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_upload) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        let native = lock(&host.native);
        let Some(native) = native.as_ref() else {
            return status(AbiStatus::Stopped);
        };
        let readiness = Arc::new(Readiness::new());
        let weak_readiness = Arc::downgrade(&readiness);
        let command_readiness = Arc::new(move || {
            if let Some(readiness) = weak_readiness.upgrade() {
                readiness.notify();
            }
        });
        let upload = match native.begin_resource_upload_with_readiness(
            link_id,
            declared_length,
            packed_metadata,
            compression,
            Some(command_readiness),
        ) {
            Ok(upload) => upload,
            Err(NativeSubmitError::Busy) => return status(AbiStatus::QueueFull),
            Err(NativeSubmitError::Stopped) => return status(AbiStatus::Stopped),
        };
        *out = Box::into_raw(Box::new(PrnsResourceUpload { upload, readiness }));
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_resource_upload_write(
    upload: *mut PrnsResourceUpload,
    chunk: PrnsByteView,
) -> u32 {
    catch_status(|| {
        let upload = match unsafe { required_ref(upload) } {
            Ok(upload) => upload,
            Err(error) => return error,
        };
        let chunk = match unsafe { read_bytes(chunk) } {
            Ok(chunk) => chunk,
            Err(error) => return error,
        };
        match upload.upload.write(chunk) {
            Ok(()) => status(AbiStatus::Ok),
            Err(UploadWriteError::WouldBlock) => status(AbiStatus::WouldBlock),
            Err(UploadWriteError::Stopped) => status(AbiStatus::Stopped),
            Err(
                UploadWriteError::ChunkTooLarge
                | UploadWriteError::LengthOverrun
                | UploadWriteError::Finished,
            ) => status(AbiStatus::InvalidArgument),
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_resource_upload_is_writable(
    upload: *const PrnsResourceUpload,
    out_writable: *mut u8,
) -> u32 {
    catch_status(|| {
        let upload = match unsafe { required_ref(upload) } {
            Ok(upload) => upload,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_writable) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = u8::from(upload.upload.is_writable());
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_resource_upload_finish(
    upload: *mut PrnsResourceUpload,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let upload = match unsafe { required_ref(upload) } {
            Ok(upload) => upload,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_command) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = issued_command(upload.upload.finish(), Arc::clone(&upload.readiness));
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_resource_upload_abort(upload: *mut PrnsResourceUpload) {
    if let Ok(Some(upload)) = catch_unwind(AssertUnwindSafe(|| unsafe { upload.as_ref() })) {
        upload.upload.abort();
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_resource_upload_release(upload: *mut PrnsResourceUpload) {
    if !upload.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(upload));
        }));
    }
}

unsafe fn submit_resource_strategy_command(
    host: *mut PrnsHost,
    link_id: Option<LinkId>,
    destination: Option<DestinationHash>,
    strategy_kind: u32,
    maximum_uncompressed_bytes: u64,
    accept_compressed: u8,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    let strategy =
        match parse_resource_strategy(strategy_kind, maximum_uncompressed_bytes, accept_compressed)
        {
            Ok(strategy) => strategy,
            Err(error) => return error,
        };
    let command = match (link_id, destination) {
        (Some(link_id), None) => HostCommand::SetLinkResourceStrategy { link_id, strategy },
        (None, Some(destination)) => HostCommand::SetDestinationResourceStrategy {
            destination,
            strategy,
        },
        _ => return status(AbiStatus::InvalidArgument),
    };
    unsafe { submit_host_command(host, command, out_command) }
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_set_link_resource_strategy(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    strategy_kind: u32,
    maximum_uncompressed_bytes: u64,
    accept_compressed: u8,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        unsafe {
            submit_resource_strategy_command(
                host,
                Some(link_id),
                None,
                strategy_kind,
                maximum_uncompressed_bytes,
                accept_compressed,
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_set_destination_resource_strategy(
    host: *mut PrnsHost,
    destination: PrnsByteView,
    strategy_kind: u32,
    maximum_uncompressed_bytes: u64,
    accept_compressed: u8,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let destination = match unsafe { read_fixed(destination) } {
            Ok(destination) => DestinationHash::new(destination),
            Err(error) => return error,
        };
        unsafe {
            submit_resource_strategy_command(
                host,
                None,
                Some(destination),
                strategy_kind,
                maximum_uncompressed_bytes,
                accept_compressed,
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_send_channel_message(
    host: *mut PrnsHost,
    link_id: PrnsByteView,
    message_type: u16,
    payload: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let link_id = match unsafe { read_fixed(link_id) } {
            Ok(link_id) => LinkId::new(link_id),
            Err(error) => return error,
        };
        let payload = match unsafe { read_bytes(payload) } {
            Ok(payload) => payload.to_vec(),
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::SendChannelMessage {
                    link_id,
                    message_type,
                    payload,
                },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_allow_requester(
    host: *mut PrnsHost,
    destination: PrnsByteView,
    path_hash: PrnsByteView,
    identity: PrnsByteView,
    out_command: *mut *mut PrnsIssuedCommand,
) -> u32 {
    catch_status(|| {
        let destination = match unsafe { read_fixed(destination) } {
            Ok(destination) => DestinationHash::new(destination),
            Err(error) => return error,
        };
        let path_hash = match unsafe { read_fixed(path_hash) } {
            Ok(path_hash) => RequestPathHash::new(path_hash),
            Err(error) => return error,
        };
        let identity = match unsafe { read_fixed(identity) } {
            Ok(identity) => IdentityHash::new(identity),
            Err(error) => return error,
        };
        unsafe {
            submit_host_command(
                host,
                HostCommand::AllowRequester {
                    destination,
                    path_hash,
                    identity,
                },
                out_command,
            )
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_stop(host: *mut PrnsHost) -> u32 {
    catch_status(|| {
        let host = match unsafe { required_ref(host) } {
            Ok(host) => host,
            Err(error) => return error,
        };
        if lock(&host.shared.queue).lifecycle().state.is_terminal() {
            return status(AbiStatus::Ok);
        }
        let publisher = HostPublisher {
            shared: Arc::clone(&host.shared),
        };
        publisher.request_stop();
        if let Some(native) = lock(&host.native).as_ref() {
            native.stop();
        } else {
            publisher.finish_stop();
        }
        status(AbiStatus::Ok)
    })
}

fn cache_command_result(result: Result<CommandOutcome, CommandFailure>) -> CachedCommandResult {
    let mut cached = CachedCommandResult {
        outcome: 0,
        failure: 0,
        evidence: 0,
        rtt_millis: 0,
        value: Vec::new(),
        detail: String::new(),
    };
    match result {
        Ok(CommandOutcome::Announced) => {
            cached.outcome = AbiCommandOutcomeKind::Announced as u32;
        }
        Ok(CommandOutcome::PacketDelivered {
            rtt_millis,
            evidence,
        }) => {
            cached.outcome = AbiCommandOutcomeKind::PacketDelivered as u32;
            cached.rtt_millis = rtt_millis;
            match evidence {
                DeliveryEvidence::ExplicitProof(hash) => {
                    cached.evidence = AbiDeliveryEvidenceKind::ExplicitProof as u32;
                    cached.value.extend_from_slice(hash.as_bytes());
                }
                DeliveryEvidence::ImplicitProof(hash) => {
                    cached.evidence = AbiDeliveryEvidenceKind::ImplicitProof as u32;
                    cached.value.extend_from_slice(hash.as_bytes());
                }
                DeliveryEvidence::Response => {
                    cached.evidence = AbiDeliveryEvidenceKind::Response as u32;
                }
            }
        }
        Ok(CommandOutcome::LinkCloseQueued) => {
            cached.outcome = AbiCommandOutcomeKind::LinkCloseQueued as u32;
        }
        Ok(CommandOutcome::InterfaceAttached { interface }) => {
            cached.outcome = AbiCommandOutcomeKind::InterfaceAttached as u32;
            cached.value.extend_from_slice(interface.as_bytes());
        }
        Ok(CommandOutcome::InterfaceDetached { interface }) => {
            cached.outcome = AbiCommandOutcomeKind::InterfaceDetached as u32;
            cached.value.extend_from_slice(interface.as_bytes());
        }
        Ok(CommandOutcome::LinkEstablished {
            link_id,
            rtt_millis,
        }) => {
            cached.outcome = AbiCommandOutcomeKind::LinkEstablished as u32;
            cached.rtt_millis = rtt_millis;
            cached.value.extend_from_slice(link_id.as_bytes());
        }
        Ok(CommandOutcome::PathDiscovered { hops }) => {
            cached.outcome = AbiCommandOutcomeKind::PathDiscovered as u32;
            cached.value.push(hops);
        }
        Ok(CommandOutcome::Identified) => {
            cached.outcome = AbiCommandOutcomeKind::Identified as u32;
        }
        Ok(CommandOutcome::ResponseReceived { data, rtt_millis }) => {
            cached.outcome = AbiCommandOutcomeKind::ResponseReceived as u32;
            cached.rtt_millis = rtt_millis;
            cached.value = data;
        }
        Ok(CommandOutcome::ResponseSent { rtt_millis }) => {
            cached.outcome = AbiCommandOutcomeKind::ResponseSent as u32;
            cached.rtt_millis = rtt_millis;
        }
        Ok(CommandOutcome::ResourceSent) => {
            cached.outcome = AbiCommandOutcomeKind::ResourceSent as u32;
        }
        Ok(CommandOutcome::ResourceStrategySet) => {
            cached.outcome = AbiCommandOutcomeKind::ResourceStrategySet as u32;
        }
        Ok(CommandOutcome::RequesterAllowed) => {
            cached.outcome = AbiCommandOutcomeKind::RequesterAllowed as u32;
        }
        Err(failure) => {
            cached.failure = match failure {
                CommandFailure::NodeStopped => AbiCommandFailureKind::NodeStopped as u32,
                CommandFailure::Busy => AbiCommandFailureKind::Busy as u32,
                CommandFailure::PayloadTooLarge => AbiCommandFailureKind::PayloadTooLarge as u32,
                CommandFailure::UnknownDestination => {
                    AbiCommandFailureKind::UnknownDestination as u32
                }
                CommandFailure::NotSingleDestination => {
                    AbiCommandFailureKind::NotSingleDestination as u32
                }
                CommandFailure::AnnounceAppDataTooLong => {
                    AbiCommandFailureKind::AnnounceAppDataTooLong as u32
                }
                CommandFailure::UnknownInterface => AbiCommandFailureKind::UnknownInterface as u32,
                CommandFailure::NoRouteToDestination => {
                    AbiCommandFailureKind::NoRouteToDestination as u32
                }
                CommandFailure::NotDirectlyReachable => {
                    AbiCommandFailureKind::NotDirectlyReachable as u32
                }
                CommandFailure::PacketCulled => AbiCommandFailureKind::PacketCulled as u32,
                CommandFailure::DeliveryTimedOut => AbiCommandFailureKind::DeliveryTimedOut as u32,
                CommandFailure::InvalidBitrate => AbiCommandFailureKind::InvalidBitrate as u32,
                CommandFailure::BindFailed { detail } => {
                    cached.detail = detail;
                    AbiCommandFailureKind::BindFailed as u32
                }
                CommandFailure::WriteFailed { detail } => {
                    cached.detail = detail;
                    AbiCommandFailureKind::WriteFailed as u32
                }
                CommandFailure::UnsupportedByBackend => {
                    AbiCommandFailureKind::UnsupportedByBackend as u32
                }
                CommandFailure::UnknownLink => AbiCommandFailureKind::UnknownLink as u32,
                CommandFailure::LinkNotActive => AbiCommandFailureKind::LinkNotActive as u32,
                CommandFailure::EntropyUnavailable => {
                    AbiCommandFailureKind::EntropyUnavailable as u32
                }
                CommandFailure::NotLinkInitiator => AbiCommandFailureKind::NotLinkInitiator as u32,
                CommandFailure::IdentityNotHeld => AbiCommandFailureKind::IdentityNotHeld as u32,
                CommandFailure::UnknownRequestHandler => {
                    AbiCommandFailureKind::UnknownRequestHandler as u32
                }
                CommandFailure::RequestPolicyNotAllowList => {
                    AbiCommandFailureKind::RequestPolicyNotAllowList as u32
                }
                CommandFailure::RequestAllowListFull => {
                    AbiCommandFailureKind::RequestAllowListFull as u32
                }
                CommandFailure::LinkBusy => AbiCommandFailureKind::LinkBusy as u32,
                CommandFailure::ResourceTableFull => {
                    AbiCommandFailureKind::ResourceTableFull as u32
                }
                CommandFailure::ResourceMetadataTooLarge => {
                    AbiCommandFailureKind::ResourceMetadataTooLarge as u32
                }
                CommandFailure::ResourceRejectedByPeer => {
                    AbiCommandFailureKind::ResourceRejectedByPeer as u32
                }
                CommandFailure::ResourceSequencingFailed => {
                    AbiCommandFailureKind::ResourceSequencingFailed as u32
                }
                CommandFailure::ResourcePredecessorFailed => {
                    AbiCommandFailureKind::ResourcePredecessorFailed as u32
                }
                CommandFailure::ChannelWindowFull => {
                    AbiCommandFailureKind::ChannelWindowFull as u32
                }
                CommandFailure::ChannelUntrackable => {
                    AbiCommandFailureKind::ChannelUntrackable as u32
                }
                CommandFailure::InvalidChannelMessageType => {
                    AbiCommandFailureKind::InvalidChannelMessageType as u32
                }
                CommandFailure::InvalidConfiguration { detail } => {
                    cached.detail = detail;
                    AbiCommandFailureKind::InvalidConfiguration as u32
                }
                CommandFailure::ResourceUploadCancelled => {
                    AbiCommandFailureKind::ResourceUploadCancelled as u32
                }
                CommandFailure::ResourceEarlyEof => AbiCommandFailureKind::ResourceEarlyEof as u32,
                CommandFailure::ResourceLengthOverrun => {
                    AbiCommandFailureKind::ResourceLengthOverrun as u32
                }
                CommandFailure::PermissionDenied { detail } => {
                    cached.detail = detail;
                    AbiCommandFailureKind::PermissionDenied as u32
                }
                CommandFailure::DeviceUnavailable { detail } => {
                    cached.detail = detail;
                    AbiCommandFailureKind::DeviceUnavailable as u32
                }
                CommandFailure::ConnectFailed { detail } => {
                    cached.detail = detail;
                    AbiCommandFailureKind::ConnectFailed as u32
                }
                CommandFailure::BackendFailed { detail } => {
                    cached.detail = detail;
                    AbiCommandFailureKind::BackendFailed as u32
                }
                CommandFailure::ResponseTooLarge => AbiCommandFailureKind::ResponseTooLarge as u32,
            };
        }
    }
    cached
}

#[no_mangle]
pub unsafe extern "C" fn prns_command_wait(
    command: *mut PrnsIssuedCommand,
    timeout_millis: u32,
    out_result: *mut PrnsCommandResult,
) -> u32 {
    catch_status(|| {
        let command = match unsafe { required_ref(command) } {
            Ok(command) => command,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_result) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        if let Err(error) = validate_size(out.struct_size, size_of::<PrnsCommandResult>()) {
            return error;
        }
        let mut cached = lock(&command.cached);
        if cached.is_none() {
            let timeout = if timeout_millis == NEVER_TIMEOUT {
                None
            } else {
                Some(Duration::from_millis(u64::from(timeout_millis)))
            };
            match command.handle.wait(timeout) {
                CommandWait::Completed(result) => {
                    *cached = Some(cache_command_result(result));
                }
                CommandWait::TimedOut => return status(AbiStatus::TimedOut),
                CommandWait::Interrupted => return status(AbiStatus::Interrupted),
            }
        }
        let Some(cached) = cached.as_ref() else {
            return status(AbiStatus::BackendFailed);
        };
        out.outcome = cached.outcome;
        out.failure = cached.failure;
        out.evidence = cached.evidence;
        out.rtt_millis = cached.rtt_millis;
        out.value = bytes_view(&cached.value);
        out.detail = string_view(&cached.detail);
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_command_interrupt_wait(command: *mut PrnsIssuedCommand) {
    if let Ok(Some(command)) = catch_unwind(AssertUnwindSafe(|| unsafe { command.as_ref() })) {
        command.handle.interrupt_wait();
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_command_register_readiness(
    command: *mut PrnsIssuedCommand,
    callback: Option<ReadinessCallback>,
    context: *mut c_void,
    out_registration: *mut *mut PrnsReadinessRegistration,
) -> u32 {
    catch_status(|| {
        let command = match unsafe { required_ref(command) } {
            Ok(command) => command,
            Err(error) => return error,
        };
        let out_registration = match unsafe { required_mut(out_registration) } {
            Ok(out_registration) => out_registration,
            Err(error) => return error,
        };
        *out_registration = ptr::null_mut();
        let Some(callback) = callback else {
            return status(AbiStatus::InvalidArgument);
        };
        let readiness = Arc::clone(&command.readiness);
        let registered = match readiness.register(callback, context) {
            Ok(registered) => registered,
            Err(_) => return status(AbiStatus::AlreadyClaimed),
        };
        *lock(&command.readiness_registration) = Some(Arc::clone(&registered));
        *out_registration = Box::into_raw(Box::new(PrnsReadinessRegistration {
            readiness,
            registered,
        }));
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_command_release(command: *mut PrnsIssuedCommand) {
    if !command.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(command));
        }));
    }
}

unsafe fn claim_stream(
    host: *mut PrnsHost,
    lane: ConsumerLane,
    out_stream: *mut *mut PrnsEventStream,
) -> u32 {
    let host = match unsafe { required_ref(host) } {
        Ok(host) => host,
        Err(error) => return error,
    };
    let out = match unsafe { required_mut(out_stream) } {
        Ok(out) => out,
        Err(error) => return error,
    };
    *out = ptr::null_mut();
    if lock(&host.shared.queue).claim_consumer(lane).is_err() {
        return status(AbiStatus::AlreadyClaimed);
    }
    *out = Box::into_raw(Box::new(PrnsEventStream {
        shared: Arc::clone(&host.shared),
        lane,
        pending_diagnostics_gap: Mutex::new(0),
        interrupted: AtomicBool::new(false),
        readiness_registration: Mutex::new(None),
    }));
    status(AbiStatus::Ok)
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_claim_application_events(
    host: *mut PrnsHost,
    out_stream: *mut *mut PrnsEventStream,
) -> u32 {
    catch_status(|| unsafe { claim_stream(host, ConsumerLane::ApplicationEvents, out_stream) })
}

#[no_mangle]
pub unsafe extern "C" fn prns_host_claim_diagnostics(
    host: *mut PrnsHost,
    out_stream: *mut *mut PrnsEventStream,
) -> u32 {
    catch_status(|| unsafe { claim_stream(host, ConsumerLane::Diagnostics, out_stream) })
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_stream_release(stream: *mut PrnsEventStream) {
    if !stream.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(stream));
        }));
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_stream_interrupt_wait(stream: *mut PrnsEventStream) {
    if let Ok(Some(stream)) = catch_unwind(AssertUnwindSafe(|| unsafe { stream.as_ref() })) {
        stream.interrupted.store(true, Ordering::Release);
        stream.shared.notify_lane(stream.lane);
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_stream_register_readiness(
    stream: *mut PrnsEventStream,
    callback: Option<ReadinessCallback>,
    context: *mut c_void,
    out_registration: *mut *mut PrnsReadinessRegistration,
) -> u32 {
    catch_status(|| {
        let stream = match unsafe { required_ref(stream) } {
            Ok(stream) => stream,
            Err(error) => return error,
        };
        let out_registration = match unsafe { required_mut(out_registration) } {
            Ok(out_registration) => out_registration,
            Err(error) => return error,
        };
        *out_registration = ptr::null_mut();
        let Some(callback) = callback else {
            return status(AbiStatus::InvalidArgument);
        };
        let readiness = Arc::clone(stream.shared.readiness(stream.lane));
        let registered = match readiness.register(callback, context) {
            Ok(registered) => registered,
            Err(_) => return status(AbiStatus::AlreadyClaimed),
        };
        *lock(&stream.readiness_registration) = Some(Arc::clone(&registered));
        *out_registration = Box::into_raw(Box::new(PrnsReadinessRegistration {
            readiness,
            registered,
        }));
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_readiness_registration_release(
    registration: *mut PrnsReadinessRegistration,
) {
    if !registration.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(registration));
        }));
    }
}

fn pop_event(stream: &PrnsEventStream, queue: &mut BoundedHostQueue<()>) -> Option<PrnsEvent> {
    let mut pending_gap = lock(&stream.pending_diagnostics_gap);
    if *pending_gap > 0 {
        let dropped = std::mem::take(&mut *pending_gap);
        return Some(PrnsEvent {
            value: EventValue::DiagnosticsDropped(dropped),
            resource: Mutex::new(None),
        });
    }
    match stream.lane {
        ConsumerLane::ApplicationEvents => queue.pop_application_event().map(|event| {
            let resource = match &event {
                ApplicationEvent::ResourceAvailable(value) => {
                    lock(&stream.shared.resources).remove(&value.stream_id.get())
                }
                _ => None,
            };
            PrnsEvent {
                value: EventValue::Application(event),
                resource: Mutex::new(resource),
            }
        }),
        ConsumerLane::Diagnostics => {
            let mut batch = queue.drain_diagnostics(1);
            if let Some(event) = batch.events.pop() {
                *pending_gap = batch.dropped_newest;
                Some(PrnsEvent {
                    value: EventValue::Diagnostic(event),
                    resource: Mutex::new(None),
                })
            } else if batch.dropped_newest > 0 {
                Some(PrnsEvent {
                    value: EventValue::DiagnosticsDropped(batch.dropped_newest),
                    resource: Mutex::new(None),
                })
            } else {
                None
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_stream_next(
    stream: *mut PrnsEventStream,
    timeout_millis: u32,
    out_event: *mut *mut PrnsEvent,
) -> u32 {
    catch_status(|| {
        let stream = match unsafe { required_ref(stream) } {
            Ok(stream) => stream,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_event) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        let deadline = if timeout_millis == NEVER_TIMEOUT {
            None
        } else {
            Instant::now().checked_add(Duration::from_millis(u64::from(timeout_millis)))
        };
        let mut queue = lock(&stream.shared.queue);
        loop {
            if stream.interrupted.swap(false, Ordering::AcqRel) {
                return status(AbiStatus::Interrupted);
            }
            if let Some(event) = pop_event(stream, &mut queue) {
                *out = Box::into_raw(Box::new(event));
                return status(AbiStatus::Ok);
            }
            if queue.lifecycle().state.is_terminal() {
                return status(AbiStatus::Stopped);
            }
            if timeout_millis == 0 {
                return status(AbiStatus::WouldBlock);
            }
            match deadline {
                None => {
                    queue = stream
                        .shared
                        .ready
                        .wait(queue)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return status(AbiStatus::TimedOut);
                    }
                    let waited = stream
                        .shared
                        .ready
                        .wait_timeout(queue, remaining)
                        .unwrap_or_else(PoisonError::into_inner);
                    if waited.1.timed_out() {
                        return status(AbiStatus::TimedOut);
                    }
                    queue = waited.0;
                }
            }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_release(event: *mut PrnsEvent) {
    if !event.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(event));
        }));
    }
}

fn application_kind(event: &ApplicationEvent) -> u32 {
    match event {
        ApplicationEvent::SingleDelivery(_) => AbiApplicationEventKind::SingleDelivery as u32,
        ApplicationEvent::LinkDelivery(_) => AbiApplicationEventKind::LinkDelivery as u32,
        ApplicationEvent::Request(_) => AbiApplicationEventKind::Request as u32,
        ApplicationEvent::Response(_) => AbiApplicationEventKind::Response as u32,
        ApplicationEvent::ResponseSegment(_) => AbiApplicationEventKind::ResponseSegment as u32,
        ApplicationEvent::ResourceAvailable(_) => AbiApplicationEventKind::ResourceAvailable as u32,
        ApplicationEvent::ResourceSegment(_) => AbiApplicationEventKind::ResourceSegment as u32,
        ApplicationEvent::ResourceNeedsDecompression(_) => {
            AbiApplicationEventKind::ResourceNeedsDecompression as u32
        }
        ApplicationEvent::ChannelMessage(_) => AbiApplicationEventKind::ChannelMessage as u32,
    }
}

fn diagnostic_kind(event: &DiagnosticEvent) -> u32 {
    match event {
        DiagnosticEvent::AnnounceHeard { .. } => AbiDiagnosticEventKind::AnnounceHeard as u32,
        DiagnosticEvent::LinkEstablished { .. } => AbiDiagnosticEventKind::LinkEstablished as u32,
        DiagnosticEvent::PeerIdentified { .. } => AbiDiagnosticEventKind::PeerIdentified as u32,
        DiagnosticEvent::LinkClosed { .. } => AbiDiagnosticEventKind::LinkClosed as u32,
        DiagnosticEvent::LinkInterfaceMismatch { .. } => {
            AbiDiagnosticEventKind::LinkInterfaceMismatch as u32
        }
        DiagnosticEvent::ResourceAssembled { .. } => {
            AbiDiagnosticEventKind::ResourceAssembled as u32
        }
        DiagnosticEvent::ResourceFailed { .. } => AbiDiagnosticEventKind::ResourceFailed as u32,
        DiagnosticEvent::ResourceSendProgress { .. } => {
            AbiDiagnosticEventKind::ResourceSendProgress as u32
        }
        DiagnosticEvent::SelfRatchetRotated { .. } => {
            AbiDiagnosticEventKind::SelfRatchetRotated as u32
        }
        DiagnosticEvent::AnnounceHeldDropped { .. } => {
            AbiDiagnosticEventKind::AnnounceHeldDropped as u32
        }
        DiagnosticEvent::Delivered { .. } => AbiDiagnosticEventKind::Delivered as u32,
        DiagnosticEvent::RouteExpired { .. } => AbiDiagnosticEventKind::RouteExpired as u32,
        DiagnosticEvent::RouteEvicted { .. } => AbiDiagnosticEventKind::RouteEvicted as u32,
        DiagnosticEvent::RouteInterfaceGone { .. } => {
            AbiDiagnosticEventKind::RouteInterfaceGone as u32
        }
        DiagnosticEvent::RouteDropped { .. } => AbiDiagnosticEventKind::RouteDropped as u32,
        DiagnosticEvent::BackendDiagnostic { .. } => {
            AbiDiagnosticEventKind::BackendDiagnostic as u32
        }
        DiagnosticEvent::PersistenceRestored { .. } => {
            AbiDiagnosticEventKind::PersistenceRestored as u32
        }
        DiagnosticEvent::PersistenceFlushed { .. } => {
            AbiDiagnosticEventKind::PersistenceFlushed as u32
        }
        DiagnosticEvent::PersistenceFlushFailed { .. } => {
            AbiDiagnosticEventKind::PersistenceFlushFailed as u32
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_kind(event: *const PrnsEvent) -> u32 {
    match catch_unwind(AssertUnwindSafe(|| {
        let event = unsafe { event.as_ref() }?;
        Some(match &event.value {
            EventValue::Application(event) => application_kind(event),
            EventValue::Diagnostic(event) => diagnostic_kind(event),
            EventValue::DiagnosticsDropped(_) => AbiDiagnosticEventKind::DiagnosticsDropped as u32,
        })
    })) {
        Ok(Some(kind)) => kind,
        _ => 0,
    }
}

fn event_bytes(event: &PrnsEvent, field: AbiEventField) -> Option<&[u8]> {
    match (&event.value, field) {
        (
            EventValue::Application(ApplicationEvent::SingleDelivery(value)),
            AbiEventField::Destination,
        ) => Some(value.destination.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::SingleDelivery(value)),
            AbiEventField::SourceInterface,
        ) => Some(value.source_interface.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::SingleDelivery(value)),
            AbiEventField::Plaintext,
        ) => Some(&value.plaintext),
        (EventValue::Application(ApplicationEvent::LinkDelivery(value)), AbiEventField::LinkId) => {
            Some(value.link_id.as_bytes())
        }
        (
            EventValue::Application(ApplicationEvent::LinkDelivery(value)),
            AbiEventField::SourceInterface,
        ) => Some(value.source_interface.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::LinkDelivery(value)),
            AbiEventField::Plaintext,
        ) => Some(&value.plaintext),
        (EventValue::Application(ApplicationEvent::Request(value)), AbiEventField::Destination) => {
            Some(value.destination.as_bytes())
        }
        (EventValue::Application(ApplicationEvent::Request(value)), AbiEventField::LinkId) => {
            Some(value.link_id.as_bytes())
        }
        (EventValue::Application(ApplicationEvent::Request(value)), AbiEventField::RequestId) => {
            Some(value.request_id.as_bytes())
        }
        (EventValue::Application(ApplicationEvent::Request(value)), AbiEventField::Requester) => {
            value
                .requester
                .as_ref()
                .map(|item| item.as_bytes().as_slice())
        }
        (EventValue::Application(ApplicationEvent::Request(value)), AbiEventField::PathHash) => {
            Some(value.path_hash.as_bytes())
        }
        (EventValue::Application(ApplicationEvent::Request(value)), AbiEventField::Data) => {
            Some(&value.data)
        }
        (EventValue::Application(ApplicationEvent::Response(value)), AbiEventField::LinkId) => {
            Some(value.link_id.as_bytes())
        }
        (
            EventValue::Application(ApplicationEvent::ResponseSegment(value)),
            AbiEventField::LinkId,
        ) => Some(value.link_id.as_bytes()),
        (EventValue::Application(ApplicationEvent::Response(value)), AbiEventField::RequestId) => {
            Some(value.request_id.as_bytes())
        }
        (
            EventValue::Application(ApplicationEvent::ResponseSegment(value)),
            AbiEventField::RequestId,
        ) => Some(value.request_id.as_bytes()),
        (EventValue::Application(ApplicationEvent::Response(value)), AbiEventField::Data) => {
            Some(&value.data)
        }
        (
            EventValue::Application(ApplicationEvent::ResponseSegment(value)),
            AbiEventField::Data,
        ) => Some(&value.data),
        (
            EventValue::Application(ApplicationEvent::ResourceAvailable(value)),
            AbiEventField::LinkId,
        ) => Some(value.link_id.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::ResourceAvailable(value)),
            AbiEventField::Hash,
        ) => Some(value.hash.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::ResourceAvailable(value)),
            AbiEventField::Metadata,
        ) => value.metadata.as_deref(),
        (
            EventValue::Application(ApplicationEvent::ResourceSegment(value)),
            AbiEventField::LinkId,
        ) => Some(value.link_id.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::ResourceSegment(value)),
            AbiEventField::OriginalHash,
        ) => Some(value.original_hash.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::ResourceSegment(value)),
            AbiEventField::Metadata,
        ) => value.metadata.as_deref(),
        (
            EventValue::Application(ApplicationEvent::ResourceSegment(value)),
            AbiEventField::Data,
        ) => Some(&value.data),
        (
            EventValue::Application(ApplicationEvent::ResourceNeedsDecompression(value)),
            AbiEventField::LinkId,
        ) => Some(value.link_id.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::ResourceNeedsDecompression(value)),
            AbiEventField::Hash,
        ) => Some(value.hash.as_bytes()),
        (
            EventValue::Application(ApplicationEvent::ResourceNeedsDecompression(value)),
            AbiEventField::Stream,
        ) => Some(&value.stream),
        (
            EventValue::Application(ApplicationEvent::ChannelMessage(value)),
            AbiEventField::LinkId,
        ) => Some(value.link_id.as_bytes()),
        (EventValue::Application(ApplicationEvent::ChannelMessage(value)), AbiEventField::Data) => {
            Some(&value.data)
        }
        (
            EventValue::Diagnostic(DiagnosticEvent::AnnounceHeard { destination, .. }),
            AbiEventField::Destination,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::SelfRatchetRotated { destination }),
            AbiEventField::Destination,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::AnnounceHeldDropped { destination, .. }),
            AbiEventField::Destination,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::RouteExpired { destination }),
            AbiEventField::Destination,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::RouteEvicted { destination }),
            AbiEventField::Destination,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::RouteInterfaceGone { destination }),
            AbiEventField::Destination,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::RouteDropped { destination }),
            AbiEventField::Destination,
        ) => Some(destination.as_bytes()),
        (
            EventValue::Diagnostic(DiagnosticEvent::AnnounceHeard {
                source_interface, ..
            }),
            AbiEventField::SourceInterface,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::AnnounceHeldDropped {
                source_interface, ..
            }),
            AbiEventField::SourceInterface,
        ) => Some(source_interface.as_bytes()),
        (
            EventValue::Diagnostic(DiagnosticEvent::AnnounceHeard { app_data, .. }),
            AbiEventField::AppData,
        ) => Some(app_data),
        (
            EventValue::Diagnostic(DiagnosticEvent::LinkEstablished { link_id, .. }),
            AbiEventField::LinkId,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::PeerIdentified { link_id, .. }),
            AbiEventField::LinkId,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::LinkClosed { link_id, .. }),
            AbiEventField::LinkId,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::LinkInterfaceMismatch { link_id, .. }),
            AbiEventField::LinkId,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::ResourceAssembled { link_id, .. }),
            AbiEventField::LinkId,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::ResourceFailed { link_id, .. }),
            AbiEventField::LinkId,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::ResourceSendProgress { link_id, .. }),
            AbiEventField::LinkId,
        ) => Some(link_id.as_bytes()),
        (
            EventValue::Diagnostic(DiagnosticEvent::PeerIdentified { identity, .. }),
            AbiEventField::Identity,
        ) => Some(identity.as_bytes()),
        (
            EventValue::Diagnostic(DiagnosticEvent::LinkInterfaceMismatch {
                attached_interface,
                ..
            }),
            AbiEventField::AttachedInterface,
        ) => Some(attached_interface.as_bytes()),
        (
            EventValue::Diagnostic(DiagnosticEvent::LinkInterfaceMismatch { arrived_on, .. }),
            AbiEventField::ArrivedOn,
        ) => Some(arrived_on.as_bytes()),
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceAssembled { original_hash, .. }),
            AbiEventField::OriginalHash,
        ) => Some(original_hash.as_bytes()),
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceFailed { hash, .. }),
            AbiEventField::Hash,
        ) => Some(hash.as_bytes()),
        _ => None,
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_bytes(
    event: *const PrnsEvent,
    field: u32,
    out_value: *mut PrnsByteView,
) -> u32 {
    catch_status(|| {
        let event = match unsafe { required_ref(event) } {
            Ok(event) => event,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        let field = match AbiEventField::try_from(field) {
            Ok(field) => field,
            Err(()) => return status(AbiStatus::InvalidArgument),
        };
        let value = match event_bytes(event, field) {
            Some(value) => value,
            None => return status(AbiStatus::InvalidArgument),
        };
        *out = bytes_view(value);
        status(AbiStatus::Ok)
    })
}

fn event_string(event: &PrnsEvent, field: AbiEventField) -> Option<&str> {
    match (&event.value, field) {
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceFailed { cause, .. }),
            AbiEventField::Cause,
        )
        | (
            EventValue::Diagnostic(DiagnosticEvent::AnnounceHeldDropped { cause, .. }),
            AbiEventField::Cause,
        ) => Some(cause),
        (EventValue::Diagnostic(DiagnosticEvent::Delivered { detail }), AbiEventField::Detail) => {
            Some(detail)
        }
        (
            EventValue::Diagnostic(DiagnosticEvent::BackendDiagnostic { kind, .. }),
            AbiEventField::Kind,
        ) => Some(kind),
        (
            EventValue::Diagnostic(DiagnosticEvent::BackendDiagnostic { detail, .. }),
            AbiEventField::Detail,
        ) => Some(detail),
        _ => None,
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_string(
    event: *const PrnsEvent,
    field: u32,
    out_value: *mut PrnsStringView,
) -> u32 {
    catch_status(|| {
        let event = match unsafe { required_ref(event) } {
            Ok(event) => event,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        let field = match AbiEventField::try_from(field) {
            Ok(field) => field,
            Err(()) => return status(AbiStatus::InvalidArgument),
        };
        let value = match event_string(event, field) {
            Some(value) => value,
            None => return status(AbiStatus::InvalidArgument),
        };
        *out = string_view(value);
        status(AbiStatus::Ok)
    })
}

fn link_reason(reason: LinkClosedReason) -> u64 {
    match reason {
        LinkClosedReason::Timeout => AbiLinkClosedReason::Timeout as u64,
        LinkClosedReason::PeerClosed => AbiLinkClosedReason::PeerClosed as u64,
        LinkClosedReason::MalformedRtt => AbiLinkClosedReason::MalformedRtt as u64,
    }
}

fn persistence_cause(cause: PersistenceFlushCause) -> u64 {
    match cause {
        PersistenceFlushCause::Startup => AbiPersistenceFlushCause::Startup as u64,
        PersistenceFlushCause::Interval => AbiPersistenceFlushCause::Interval as u64,
        PersistenceFlushCause::RouteChange => AbiPersistenceFlushCause::RouteChange as u64,
        PersistenceFlushCause::RatchetRotation => AbiPersistenceFlushCause::RatchetRotation as u64,
        PersistenceFlushCause::Shutdown => AbiPersistenceFlushCause::Shutdown as u64,
    }
}

fn persistence_target(target: PersistenceFlushTarget) -> u64 {
    match target {
        PersistenceFlushTarget::RoutingState => AbiPersistenceFlushTarget::RoutingState as u64,
        PersistenceFlushTarget::Ratchets => AbiPersistenceFlushTarget::Ratchets as u64,
    }
}

fn event_u64(event: &PrnsEvent, field: AbiEventField) -> Option<u64> {
    match (&event.value, field) {
        (
            EventValue::Application(ApplicationEvent::ChannelMessage(value)),
            AbiEventField::MessageType,
        ) => Some(u64::from(value.message_type)),
        (EventValue::Application(ApplicationEvent::Request(value)), AbiEventField::RttMillis) => {
            Some(value.rtt_millis)
        }
        (
            EventValue::Application(ApplicationEvent::ResponseSegment(value)),
            AbiEventField::SegmentIndex,
        ) => Some(value.segment_index),
        (
            EventValue::Application(ApplicationEvent::ResourceSegment(value)),
            AbiEventField::SegmentIndex,
        ) => Some(value.segment_index),
        (
            EventValue::Application(ApplicationEvent::ResponseSegment(value)),
            AbiEventField::TotalSegments,
        ) => Some(value.total_segments),
        (
            EventValue::Application(ApplicationEvent::ResourceSegment(value)),
            AbiEventField::TotalSegments,
        ) => Some(value.total_segments),
        (
            EventValue::Application(ApplicationEvent::ResourceAvailable(value)),
            AbiEventField::TotalBytes,
        ) => Some(value.total_bytes),
        (
            EventValue::Application(ApplicationEvent::ResourceAvailable(value)),
            AbiEventField::StreamId,
        ) => Some(value.stream_id.get()),
        (
            EventValue::Application(ApplicationEvent::ResourceNeedsDecompression(value)),
            AbiEventField::UncompressedDataBytes,
        ) => Some(value.uncompressed_data_bytes),
        (
            EventValue::Diagnostic(DiagnosticEvent::AnnounceHeard { hops, .. }),
            AbiEventField::Hops,
        ) => Some(u64::from(*hops)),
        (
            EventValue::Diagnostic(DiagnosticEvent::LinkEstablished { rtt_millis, .. }),
            AbiEventField::RttMillis,
        ) => Some(*rtt_millis),
        (
            EventValue::Diagnostic(DiagnosticEvent::LinkClosed { reason, .. }),
            AbiEventField::Reason,
        ) => Some(link_reason(*reason)),
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceAssembled {
                total_size_bytes, ..
            }),
            AbiEventField::TotalSizeBytes,
        ) => Some(*total_size_bytes),
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceSendProgress {
                transferred_bytes, ..
            }),
            AbiEventField::TransferredBytes,
        ) => Some(*transferred_bytes),
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceSendProgress { total_bytes, .. }),
            AbiEventField::TotalBytes,
        ) => Some(*total_bytes),
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceSendProgress {
                physical_transferred_bytes,
                ..
            }),
            AbiEventField::PhysicalTransferredBytes,
        ) => Some(*physical_transferred_bytes),
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceSendProgress { segment_index, .. }),
            AbiEventField::SegmentIndex,
        ) => Some(*segment_index),
        (
            EventValue::Diagnostic(DiagnosticEvent::ResourceSendProgress {
                total_segments, ..
            }),
            AbiEventField::TotalSegments,
        ) => Some(*total_segments),
        (
            EventValue::Diagnostic(DiagnosticEvent::PersistenceRestored { routes, .. }),
            AbiEventField::Routes,
        ) => Some(*routes),
        (
            EventValue::Diagnostic(DiagnosticEvent::PersistenceRestored {
                destination_identities,
                ..
            }),
            AbiEventField::DestinationIdentities,
        ) => Some(*destination_identities),
        (
            EventValue::Diagnostic(DiagnosticEvent::PersistenceRestored { tunnels, .. }),
            AbiEventField::Tunnels,
        ) => Some(*tunnels),
        (
            EventValue::Diagnostic(DiagnosticEvent::PersistenceRestored { ratchets, .. }),
            AbiEventField::Ratchets,
        ) => Some(*ratchets),
        (
            EventValue::Diagnostic(DiagnosticEvent::PersistenceRestored { refused, .. }),
            AbiEventField::Refused,
        ) => Some(*refused),
        (
            EventValue::Diagnostic(DiagnosticEvent::PersistenceRestored { dropped, .. }),
            AbiEventField::Dropped,
        ) => Some(*dropped),
        (
            EventValue::Diagnostic(
                DiagnosticEvent::PersistenceFlushed { cause, .. }
                | DiagnosticEvent::PersistenceFlushFailed { cause, .. },
            ),
            AbiEventField::PersistenceCause,
        ) => Some(persistence_cause(*cause)),
        (
            EventValue::Diagnostic(
                DiagnosticEvent::PersistenceFlushed { target, .. }
                | DiagnosticEvent::PersistenceFlushFailed { target, .. },
            ),
            AbiEventField::PersistenceTarget,
        ) => Some(persistence_target(*target)),
        _ => None,
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_u64(
    event: *const PrnsEvent,
    field: u32,
    out_value: *mut u64,
) -> u32 {
    catch_status(|| {
        let event = match unsafe { required_ref(event) } {
            Ok(event) => event,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_value) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        let field = match AbiEventField::try_from(field) {
            Ok(field) => field,
            Err(()) => return status(AbiStatus::InvalidArgument),
        };
        let value = match event_u64(event, field) {
            Some(value) => value,
            None => return status(AbiStatus::InvalidArgument),
        };
        *out = value;
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_u128(
    event: *const PrnsEvent,
    field: u32,
    out_low: *mut u64,
    out_high: *mut u64,
) -> u32 {
    catch_status(|| {
        let event = match unsafe { required_ref(event) } {
            Ok(event) => event,
            Err(error) => return error,
        };
        let low = match unsafe { required_mut(out_low) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        let high = match unsafe { required_mut(out_high) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        if AbiEventField::try_from(field) != Ok(AbiEventField::DroppedCount) {
            return status(AbiStatus::InvalidArgument);
        }
        let EventValue::DiagnosticsDropped(value) = &event.value else {
            return status(AbiStatus::InvalidArgument);
        };
        *low = *value as u64;
        *high = (*value >> 64) as u64;
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_event_resource_stream(
    event: *mut PrnsEvent,
    out_stream: *mut *mut PrnsResourceStream,
) -> u32 {
    catch_status(|| {
        let event = match unsafe { required_ref(event) } {
            Ok(event) => event,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_stream) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        if !matches!(
            &event.value,
            EventValue::Application(ApplicationEvent::ResourceAvailable(_))
        ) {
            return status(AbiStatus::InvalidArgument);
        }
        let claimed = lock(&event.resource).take();
        let stream = match claimed {
            Some(stream) => stream,
            None => return status(AbiStatus::AlreadyClaimed),
        };
        *out = Box::into_raw(Box::new(stream));
        status(AbiStatus::Ok)
    })
}

#[no_mangle]
pub unsafe extern "C" fn prns_resource_stream_release(stream: *mut PrnsResourceStream) {
    if !stream.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(stream));
        }));
    }
}

#[no_mangle]
pub unsafe extern "C" fn prns_resource_stream_next(
    stream: *mut PrnsResourceStream,
    maximum_bytes: usize,
    out_chunk: *mut PrnsByteView,
    out_finished: *mut u8,
) -> u32 {
    catch_status(|| {
        let stream = match unsafe { required_ref(stream) } {
            Ok(stream) => stream,
            Err(error) => return error,
        };
        let chunk = match unsafe { required_mut(out_chunk) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        let finished = match unsafe { required_mut(out_finished) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        if maximum_bytes == 0 {
            return status(AbiStatus::InvalidArgument);
        }
        let mut state = lock(&stream.state);
        loop {
            let exhausted = state
                .active
                .as_ref()
                .is_none_or(|active| state.offset >= active.len());
            if exhausted {
                state.active = state.chunks.pop_front();
                state.offset = 0;
            }
            let Some(active) = state.active.as_ref() else {
                *chunk = bytes_view(&[]);
                *finished = 1;
                break;
            };
            if active.is_empty() {
                state.active = None;
                continue;
            }
            let start = state.offset;
            let end = start.saturating_add(maximum_bytes).min(active.len());
            state.offset = end;
            let active = state.active.as_deref().unwrap_or(&[]);
            *chunk = bytes_view(&active[start..end]);
            *finished = 0;
            break;
        }
        status(AbiStatus::Ok)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_host_core::{
        DestinationHash, InterfaceId, LinkDelivery, LinkId, ResourceHash, ResourceStreamId,
        SingleDelivery,
    };
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn response_too_large_preserves_its_failure_kind() {
        let cached = cache_command_result(Err(CommandFailure::ResponseTooLarge));
        assert_eq!(
            cached.failure,
            AbiCommandFailureKind::ResponseTooLarge as u32
        );
    }

    #[test]
    fn optional_safe_uint_rejects_values_outside_the_contract_range() {
        assert_eq!(optional_safe_uint(0, u64::MAX), Ok(None));
        assert_eq!(
            optional_safe_uint(1, SAFE_UINT_MAX),
            Ok(Some(SAFE_UINT_MAX))
        );
        assert_eq!(
            optional_safe_uint(1, SAFE_UINT_MAX + 1),
            Err(status(AbiStatus::InvalidArgument))
        );
    }

    #[test]
    fn websocket_framing_selection_rejects_zero_and_unknown_discriminants() {
        assert_eq!(
            parse_websocket_framing_selection(WebSocketFramingSelection::Auto as u32),
            Ok(WebSocketFramingSelection::Auto)
        );
        assert_eq!(
            parse_websocket_framing_selection(0),
            Err(status(AbiStatus::InvalidArgument))
        );
        assert_eq!(
            parse_websocket_framing_selection(u32::MAX),
            Err(status(AbiStatus::InvalidArgument))
        );
    }

    #[test]
    fn backend_info_reports_exact_compiled_capabilities() {
        let mut info = PrnsBackendInfo {
            struct_size: size_of::<PrnsBackendInfo>(),
            backend: 0,
            capabilities: ptr::null(),
            capability_count: 0,
            interface_kinds: ptr::null(),
            interface_kind_count: 0,
        };
        assert_eq!(
            unsafe { prns_backend_info(&mut info) },
            status(AbiStatus::Ok)
        );
        assert_eq!(info.backend, AbiBackendKind::Native as u32);
        let capabilities =
            unsafe { slice::from_raw_parts(info.capabilities, info.capability_count) };
        let interface_kinds =
            unsafe { slice::from_raw_parts(info.interface_kinds, info.interface_kind_count) };
        assert_eq!(capabilities, NATIVE_CAPABILITIES);
        assert_eq!(interface_kinds, NATIVE_INTERFACE_KINDS);
    }
    use std::sync::Barrier;

    fn limits() -> CoreLimits {
        CoreLimits::try_new(1, 2, 64, 1).unwrap_or_else(|_| CoreLimits::balanced())
    }

    unsafe extern "C" fn count_readiness(context: *mut c_void) {
        let counter = unsafe { &*context.cast::<AtomicUsize>() };
        counter.fetch_add(1, Ordering::AcqRel);
    }

    struct BlockingReadiness {
        entered: Barrier,
        resume: Barrier,
    }

    unsafe extern "C" fn block_readiness(context: *mut c_void) {
        let readiness = unsafe { &*context.cast::<BlockingReadiness>() };
        readiness.entered.wait();
        readiness.resume.wait();
    }

    #[test]
    fn views_reject_unrepresentable_and_malformed_inputs() {
        let invalid = status(AbiStatus::InvalidArgument);
        let byte_cases = [
            PrnsByteView {
                data: ptr::null(),
                length: 1,
            },
            PrnsByteView {
                data: ptr::dangling(),
                length: isize::MAX as usize + 1,
            },
        ];
        for value in byte_cases {
            assert_eq!(unsafe { read_bytes(value) }.map(|_| ()), Err(invalid));
        }

        let invalid_utf8 = [u8::MAX];
        let string_cases = [
            PrnsStringView {
                data: ptr::null(),
                length: 1,
            },
            PrnsStringView {
                data: ptr::dangling(),
                length: isize::MAX as usize + 1,
            },
            PrnsStringView {
                data: invalid_utf8.as_ptr(),
                length: invalid_utf8.len(),
            },
        ];
        for value in string_cases {
            assert_eq!(unsafe { read_string(value) }.map(|_| ()), Err(invalid));
        }

        let array_cases = [
            (ptr::null(), 1),
            (ptr::dangling(), isize::MAX as usize / size_of::<u32>() + 1),
            (ptr::dangling(), usize::MAX),
        ];
        for (data, length) in array_cases {
            assert_eq!(
                unsafe { read_array::<u32>(data, length) }.map(|_| ()),
                Err(invalid)
            );
        }
    }

    #[test]
    fn capsule_preserves_single_consumer_and_event_memory() {
        let (mut host, publisher) = host_capsule(limits());
        let mut stream = ptr::null_mut();
        let mut duplicate = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_claim_application_events(&mut host, &mut stream) },
            status(AbiStatus::Ok)
        );
        assert_eq!(
            unsafe { prns_host_claim_application_events(&mut host, &mut duplicate) },
            status(AbiStatus::AlreadyClaimed)
        );
        let expected = vec![1, 2, 3, 4];
        assert!(publisher
            .publish_application(ApplicationEvent::SingleDelivery(SingleDelivery {
                destination: DestinationHash::new([7; 16]),
                source_interface: InterfaceId::new([8; 8]),
                plaintext: expected.clone(),
            }))
            .is_ok());
        let mut event = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_stream_next(stream, 0, &mut event) },
            status(AbiStatus::Ok)
        );
        let mut view = PrnsByteView {
            data: ptr::null(),
            length: 0,
        };
        assert_eq!(
            unsafe {
                prns_event_bytes(
                    event,
                    AbiEventField::Plaintext as u32,
                    &mut view as *mut PrnsByteView,
                )
            },
            status(AbiStatus::Ok)
        );
        let actual = unsafe { slice::from_raw_parts(view.data, view.length) };
        assert_eq!(actual, expected);
        unsafe {
            prns_event_release(event);
            prns_event_stream_release(stream);
        }
        assert_eq!(
            unsafe { prns_host_claim_application_events(&mut host, &mut duplicate) },
            status(AbiStatus::Ok)
        );
        unsafe {
            prns_event_stream_release(duplicate);
        }
    }

    #[test]
    fn capsule_projects_link_delivery_fields() {
        let (mut host, publisher) = host_capsule(limits());
        let mut stream = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_claim_application_events(&mut host, &mut stream) },
            status(AbiStatus::Ok)
        );
        let link_id = LinkId::new([7; 16]);
        let source_interface = InterfaceId::new([8; 8]);
        let plaintext = vec![1, 2, 3, 4];
        assert!(publisher
            .publish_application(ApplicationEvent::LinkDelivery(LinkDelivery {
                link_id,
                source_interface,
                plaintext: plaintext.clone(),
            }))
            .is_ok());
        let mut event = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_stream_next(stream, 0, &mut event) },
            status(AbiStatus::Ok)
        );
        assert_eq!(
            unsafe { prns_event_kind(event) },
            AbiApplicationEventKind::LinkDelivery as u32
        );
        for (field, expected) in [
            (AbiEventField::LinkId, link_id.as_bytes().as_slice()),
            (
                AbiEventField::SourceInterface,
                source_interface.as_bytes().as_slice(),
            ),
            (AbiEventField::Plaintext, plaintext.as_slice()),
        ] {
            let mut view = PrnsByteView {
                data: ptr::null(),
                length: 0,
            };
            assert_eq!(
                unsafe { prns_event_bytes(event, field as u32, &mut view) },
                status(AbiStatus::Ok)
            );
            assert_eq!(
                unsafe { slice::from_raw_parts(view.data, view.length) },
                expected
            );
        }
        unsafe {
            prns_event_release(event);
            prns_event_stream_release(stream);
        }
    }

    #[test]
    fn capsule_projects_announce_application_data() {
        let (mut host, publisher) = host_capsule(limits());
        let mut stream = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_claim_diagnostics(&mut host, &mut stream) },
            status(AbiStatus::Ok)
        );
        let destination = DestinationHash::new([7; 16]);
        let source_interface = InterfaceId::new([8; 8]);
        let app_data = vec![0, 1, 2, 255];
        publisher.publish_diagnostic(DiagnosticEvent::AnnounceHeard {
            destination,
            hops: 3,
            source_interface,
            app_data: app_data.clone(),
        });
        let mut event = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_stream_next(stream, 0, &mut event) },
            status(AbiStatus::Ok)
        );
        assert_eq!(
            unsafe { prns_event_kind(event) },
            AbiDiagnosticEventKind::AnnounceHeard as u32
        );
        for (field, expected) in [
            (
                AbiEventField::Destination,
                destination.as_bytes().as_slice(),
            ),
            (
                AbiEventField::SourceInterface,
                source_interface.as_bytes().as_slice(),
            ),
            (AbiEventField::AppData, app_data.as_slice()),
        ] {
            let mut view = PrnsByteView {
                data: ptr::null(),
                length: 0,
            };
            assert_eq!(
                unsafe { prns_event_bytes(event, field as u32, &mut view) },
                status(AbiStatus::Ok)
            );
            assert_eq!(
                unsafe { slice::from_raw_parts(view.data, view.length) },
                expected
            );
        }
        let mut hops = 0;
        assert_eq!(
            unsafe { prns_event_u64(event, AbiEventField::Hops as u32, &mut hops) },
            status(AbiStatus::Ok)
        );
        assert_eq!(hops, 3);
        unsafe {
            prns_event_release(event);
            prns_event_stream_release(stream);
        }
    }

    #[test]
    fn readiness_registration_signals_without_owning_event_delivery() {
        let (mut host, publisher) = host_capsule(limits());
        let mut stream = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_claim_application_events(&mut host, &mut stream) },
            status(AbiStatus::Ok)
        );
        let readiness_count = AtomicUsize::new(0);
        let context = ptr::from_ref(&readiness_count).cast_mut().cast::<c_void>();
        let mut registration = ptr::null_mut();
        assert_eq!(
            unsafe {
                prns_event_stream_register_readiness(
                    stream,
                    Some(count_readiness),
                    context,
                    &mut registration,
                )
            },
            status(AbiStatus::Ok)
        );
        assert_eq!(readiness_count.load(Ordering::Acquire), 0);

        let mut duplicate = ptr::null_mut();
        assert_eq!(
            unsafe {
                prns_event_stream_register_readiness(
                    stream,
                    Some(count_readiness),
                    context,
                    &mut duplicate,
                )
            },
            status(AbiStatus::AlreadyClaimed)
        );
        assert!(duplicate.is_null());

        assert!(publisher
            .publish_application(ApplicationEvent::SingleDelivery(SingleDelivery {
                destination: DestinationHash::new([7; 16]),
                source_interface: InterfaceId::new([8; 8]),
                plaintext: vec![1, 2, 3],
            }))
            .is_ok());
        assert_eq!(readiness_count.load(Ordering::Acquire), 1);

        let mut event = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_stream_next(stream, 0, &mut event) },
            status(AbiStatus::Ok)
        );
        unsafe {
            prns_event_release(event);
            prns_readiness_registration_release(registration);
        }
        assert!(publisher
            .publish_application(ApplicationEvent::SingleDelivery(SingleDelivery {
                destination: DestinationHash::new([9; 16]),
                source_interface: InterfaceId::new([10; 8]),
                plaintext: vec![4, 5, 6],
            }))
            .is_ok());
        assert_eq!(readiness_count.load(Ordering::Acquire), 1);
        unsafe {
            prns_event_stream_release(stream);
        }
    }

    #[test]
    fn stream_release_quiesces_its_readiness_callback() {
        let (mut host, publisher) = host_capsule(limits());
        let mut stream = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_claim_diagnostics(&mut host, &mut stream) },
            status(AbiStatus::Ok)
        );
        let readiness = BlockingReadiness {
            entered: Barrier::new(2),
            resume: Barrier::new(2),
        };
        let context = ptr::from_ref(&readiness).cast_mut().cast::<c_void>();
        let mut registration = ptr::null_mut();
        assert_eq!(
            unsafe {
                prns_event_stream_register_readiness(
                    stream,
                    Some(block_readiness),
                    context,
                    &mut registration,
                )
            },
            status(AbiStatus::Ok)
        );
        let publishing = std::thread::spawn(move || {
            publisher.publish_diagnostic(DiagnosticEvent::Delivered {
                detail: "during-release".into(),
            });
        });
        readiness.entered.wait();
        let stream_address = stream as usize;
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (released_tx, released_rx) = std::sync::mpsc::channel();
        let releasing = std::thread::spawn(move || {
            let _ = started_tx.send(());
            unsafe {
                prns_event_stream_release(stream_address as *mut PrnsEventStream);
            }
            let _ = released_tx.send(());
        });
        assert!(started_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(released_rx.recv_timeout(Duration::from_millis(20)).is_err());
        readiness.resume.wait();
        assert!(publishing.join().is_ok());
        assert!(released_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(releasing.join().is_ok());
        unsafe {
            prns_readiness_registration_release(registration);
        }
    }

    #[test]
    fn diagnostics_report_exact_gap() {
        let (mut host, publisher) = host_capsule(limits());
        publisher.publish_diagnostic(DiagnosticEvent::Delivered {
            detail: "kept".into(),
        });
        publisher.publish_diagnostic(DiagnosticEvent::Delivered {
            detail: "dropped-one".into(),
        });
        publisher.publish_diagnostic(DiagnosticEvent::Delivered {
            detail: "dropped-two".into(),
        });
        let mut stream = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_claim_diagnostics(&mut host, &mut stream) },
            status(AbiStatus::Ok)
        );
        let mut event = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_stream_next(stream, 0, &mut event) },
            status(AbiStatus::Ok)
        );
        unsafe {
            prns_event_release(event);
        }
        event = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_stream_next(stream, 0, &mut event) },
            status(AbiStatus::Ok)
        );
        assert_eq!(
            unsafe { prns_event_kind(event) },
            AbiDiagnosticEventKind::DiagnosticsDropped as u32
        );
        let mut low = 0;
        let mut high = 0;
        assert_eq!(
            unsafe {
                prns_event_u128(
                    event,
                    AbiEventField::DroppedCount as u32,
                    &mut low,
                    &mut high,
                )
            },
            status(AbiStatus::Ok)
        );
        assert_eq!((high, low), (0, 2));
        unsafe {
            prns_event_release(event);
            prns_event_stream_release(stream);
        }
    }

    #[test]
    fn resource_body_transfers_to_exactly_one_stream() {
        let (mut host, publisher) = host_capsule(limits());
        assert!(publisher
            .publish_resource(
                ResourceAvailable {
                    stream_id: ResourceStreamId::new(9),
                    link_id: LinkId::new([3; 16]),
                    hash: ResourceHash::new([4; 32]),
                    metadata: None,
                    total_bytes: 5,
                },
                vec![1, 2, 3, 4, 5],
            )
            .is_ok());
        let mut events = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_claim_application_events(&mut host, &mut events) },
            status(AbiStatus::Ok)
        );
        let mut event = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_stream_next(events, 0, &mut event) },
            status(AbiStatus::Ok)
        );
        let mut resource = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_resource_stream(event, &mut resource) },
            status(AbiStatus::Ok)
        );
        let mut duplicate = ptr::null_mut();
        assert_eq!(
            unsafe { prns_event_resource_stream(event, &mut duplicate) },
            status(AbiStatus::AlreadyClaimed)
        );
        let mut collected = Vec::new();
        loop {
            let mut view = PrnsByteView {
                data: ptr::null(),
                length: 0,
            };
            let mut finished = 0;
            assert_eq!(
                unsafe { prns_resource_stream_next(resource, 1, &mut view, &mut finished) },
                status(AbiStatus::Ok)
            );
            if finished != 0 {
                break;
            }
            collected.extend_from_slice(unsafe { slice::from_raw_parts(view.data, view.length) });
        }
        assert_eq!(collected, vec![1, 2, 3, 4, 5]);
        unsafe {
            prns_resource_stream_release(resource);
            prns_event_release(event);
            prns_event_stream_release(events);
        }
    }

    #[test]
    fn creation_gates_contract_and_lifecycle() {
        let version = HOST_CONTRACT.product_version.as_bytes();
        let selected = CoreLimits::try_new(2, 2, 64, 1).unwrap_or_else(|_| CoreLimits::balanced());
        let native_limits = PrnsLimits {
            struct_size: size_of::<PrnsLimits>(),
            pending_commands: selected.pending_commands(),
            application_events: selected.application_events(),
            retained_event_bytes: selected.retained_event_bytes(),
            diagnostics: selected.diagnostics(),
        };
        let mut options = PrnsHostOptions {
            struct_size: size_of::<PrnsHostOptions>(),
            required_abi: HOST_CONTRACT.abi + 1,
            required_schema_version: HOST_CONTRACT.schema_version,
            required_product_version: PrnsStringView {
                data: version.as_ptr(),
                length: version.len(),
            },
            limits: native_limits,
            role: AbiHostRole::Endpoint as u32,
            identity: PrnsIdentityConfig {
                struct_size: size_of::<PrnsIdentityConfig>(),
                kind: AbiIdentityConfigKind::GenerateEphemeral as u32,
                secret: PrnsByteView {
                    data: ptr::null(),
                    length: 0,
                },
                path: PrnsStringView {
                    data: ptr::null(),
                    length: 0,
                },
            },
            destinations: ptr::null(),
            destination_count: 0,
            required_capabilities: ptr::null(),
            required_capability_count: 0,
            persistence: PrnsPersistenceConfig {
                struct_size: size_of::<PrnsPersistenceConfig>(),
                kind: AbiPersistenceConfigKind::Ephemeral as u32,
                path: PrnsStringView {
                    data: ptr::null(),
                    length: 0,
                },
            },
        };
        let mut host = ptr::null_mut();
        options.struct_size = size_of::<PrnsHostOptions>() - 1;
        assert_eq!(
            unsafe { prns_host_create(&options, &mut host) },
            status(AbiStatus::InvalidArgument)
        );
        options.struct_size = size_of::<PrnsHostOptions>() + 64;
        assert_eq!(
            unsafe { prns_host_create(&options, &mut host) },
            status(AbiStatus::ContractMismatch)
        );
        options.struct_size = size_of::<PrnsHostOptions>();
        options.required_abi = HOST_CONTRACT.abi;
        options.required_schema_version = HOST_CONTRACT.schema_version + 1;
        assert_eq!(
            unsafe { prns_host_create(&options, &mut host) },
            status(AbiStatus::ContractMismatch)
        );
        options.required_schema_version = HOST_CONTRACT.schema_version;
        let incompatible_version = b"0.0.0";
        options.required_product_version = PrnsStringView {
            data: incompatible_version.as_ptr(),
            length: incompatible_version.len(),
        };
        assert_eq!(
            unsafe { prns_host_create(&options, &mut host) },
            status(AbiStatus::ContractMismatch)
        );
        options.required_product_version = PrnsStringView {
            data: version.as_ptr(),
            length: version.len(),
        };
        assert!(host.is_null());
        options.required_abi = HOST_CONTRACT.abi;
        assert_eq!(
            unsafe { prns_host_create(&options, &mut host) },
            status(AbiStatus::Ok)
        );
        let mut lifecycle = PrnsLifecycle {
            struct_size: size_of::<PrnsLifecycle>(),
            revision: 0,
            phase: 0,
            reason: 0,
        };
        assert_eq!(
            unsafe { prns_host_lifecycle(host, &mut lifecycle) },
            status(AbiStatus::Ok)
        );
        assert_eq!(lifecycle.phase, AbiLifecyclePhase::Running as u32);

        let mut inspection = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_snapshot(host, NEVER_TIMEOUT, &mut inspection) },
            status(AbiStatus::Ok)
        );
        let mut snapshot = PrnsHostSnapshot {
            struct_size: size_of::<PrnsHostSnapshot>(),
            revision: 0,
            backend: PrnsBackendInfo {
                struct_size: 0,
                backend: 0,
                capabilities: ptr::null(),
                capability_count: 0,
                interface_kinds: ptr::null(),
                interface_kind_count: 0,
            },
            interfaces: ptr::null(),
            interface_count: 0,
            routes: ptr::null(),
            route_count: 0,
            active_link_count: 0,
            destination_identities: ptr::null(),
            destination_identity_count: 0,
            runtime: PrnsRuntimeHealthSnapshot {
                struct_size: 0,
                running: 0,
                uptime_millis: 0,
                interface_count: 0,
                online_interface_count: 0,
                route_count: 0,
                link_count: 0,
                transported_link_count: 0,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_bps: 0,
                tx_bps: 0,
            },
            persistence: PrnsPersistenceSnapshot {
                struct_size: 0,
                persistent: 0,
                restored: 0,
                has_last_flush_cause: 0,
                last_flush_cause: 0,
                has_last_failure_detail: 0,
                last_failure_detail: string_view(""),
            },
        };
        assert_eq!(
            unsafe { prns_host_snapshot_read(inspection, &mut snapshot) },
            status(AbiStatus::Ok)
        );
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.backend.backend, AbiBackendKind::Native as u32);
        assert_eq!(snapshot.runtime.running, 1);
        assert_eq!(snapshot.persistence.persistent, 0);
        unsafe {
            prns_host_snapshot_release(inspection);
        }

        let target = b"127.0.0.1:9";
        let mut command = ptr::null_mut();
        assert_eq!(
            unsafe {
                prns_host_attach_tcp_client(
                    host,
                    PrnsStringView {
                        data: target.as_ptr(),
                        length: target.len(),
                    },
                    AbiBitrateKind::Auto as u32,
                    0,
                    &mut command,
                )
            },
            status(AbiStatus::Ok)
        );
        let readiness_count = AtomicUsize::new(0);
        let context = ptr::from_ref(&readiness_count).cast_mut().cast::<c_void>();
        let mut registration = ptr::null_mut();
        assert_eq!(
            unsafe {
                prns_command_register_readiness(
                    command,
                    Some(count_readiness),
                    context,
                    &mut registration,
                )
            },
            status(AbiStatus::Ok)
        );
        unsafe {
            prns_command_interrupt_wait(command);
        }
        assert!(readiness_count.load(Ordering::Acquire) > 0);
        unsafe {
            prns_readiness_registration_release(registration);
            prns_command_release(command);
        }

        let mut generic: PrnsInterfaceConfig = unsafe { std::mem::zeroed() };
        generic.struct_size = size_of::<PrnsInterfaceConfig>();
        generic.kind = AbiInterfaceKind::BrowserRendezvous as u32;
        generic.url = string_view("ws://127.0.0.1:4242");
        let mut routing = PrnsInterfaceRoutingPolicy {
            struct_size: size_of::<PrnsInterfaceRoutingPolicy>(),
            has_mode: 1,
            mode: InterfaceMode::Boundary as u32,
            has_gravity: 1,
            gravity: -73,
            has_recursive_path_requests: 1,
            recursive_path_requests: 1,
            has_announces_from_internal: 1,
            announces_from_internal: 0,
            has_announces_to_internal: 1,
            announces_to_internal: 1,
        };
        command = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_attach_interface(host, &generic, &routing, &mut command) },
            status(AbiStatus::Ok)
        );
        let mut generic_result = PrnsCommandResult {
            struct_size: size_of::<PrnsCommandResult>(),
            outcome: 0,
            failure: 0,
            evidence: 0,
            rtt_millis: 0,
            value: bytes_view(&[]),
            detail: string_view(""),
        };
        assert_eq!(
            unsafe { prns_command_wait(command, NEVER_TIMEOUT, &mut generic_result) },
            status(AbiStatus::Ok)
        );
        assert_eq!(
            generic_result.failure,
            AbiCommandFailureKind::UnsupportedByBackend as u32
        );
        unsafe {
            prns_command_release(command);
        }
        routing.mode = u32::MAX;
        command = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_attach_interface(host, &generic, &routing, &mut command) },
            status(AbiStatus::InvalidArgument)
        );
        assert!(command.is_null());
        routing.mode = InterfaceMode::Full as u32;
        routing.gravity = SAFE_INT_MAX + 1;
        assert_eq!(
            unsafe { prns_host_attach_interface(host, &generic, &routing, &mut command) },
            status(AbiStatus::InvalidArgument)
        );

        let link = [0u8; 16];
        let mut upload = ptr::null_mut();
        assert_eq!(
            unsafe {
                prns_host_begin_resource_upload(
                    host,
                    PrnsByteView {
                        data: link.as_ptr(),
                        length: link.len(),
                    },
                    1,
                    ptr::null(),
                    AbiResourceCompressionKind::Auto as u32,
                    &mut upload,
                )
            },
            status(AbiStatus::Ok)
        );
        let mut writable = 0;
        assert_eq!(
            unsafe { prns_resource_upload_is_writable(upload, &mut writable) },
            status(AbiStatus::Ok)
        );
        assert_eq!(writable, 1);
        let bytes = [1u8, 2];
        assert_eq!(
            unsafe {
                prns_resource_upload_write(
                    upload,
                    PrnsByteView {
                        data: bytes.as_ptr(),
                        length: bytes.len(),
                    },
                )
            },
            status(AbiStatus::InvalidArgument)
        );
        command = ptr::null_mut();
        assert_eq!(
            unsafe { prns_resource_upload_finish(upload, &mut command) },
            status(AbiStatus::Ok)
        );
        let mut result = PrnsCommandResult {
            struct_size: size_of::<PrnsCommandResult>(),
            outcome: 0,
            failure: 0,
            evidence: 0,
            rtt_millis: 0,
            value: PrnsByteView {
                data: ptr::null(),
                length: 0,
            },
            detail: PrnsStringView {
                data: ptr::null(),
                length: 0,
            },
        };
        assert_eq!(
            unsafe { prns_command_wait(command, NEVER_TIMEOUT, &mut result) },
            status(AbiStatus::Ok)
        );
        assert_eq!(
            result.failure,
            AbiCommandFailureKind::ResourceLengthOverrun as u32
        );
        unsafe {
            prns_command_release(command);
            prns_resource_upload_release(upload);
        }

        assert_eq!(unsafe { prns_host_stop(host) }, status(AbiStatus::Ok));
        assert_eq!(
            unsafe { prns_host_lifecycle(host, &mut lifecycle) },
            status(AbiStatus::Ok)
        );
        assert_eq!(lifecycle.phase, AbiLifecyclePhase::Stopped as u32);
        assert_eq!(lifecycle.reason, AbiStopReason::Requested as u32);
        unsafe {
            prns_host_release(host);
        }
    }
}
