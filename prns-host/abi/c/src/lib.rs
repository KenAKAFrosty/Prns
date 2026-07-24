#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::str;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use prns_host_core::{
    verify_host_contract, AbiApplicationEventKind, AbiDiagnosticEventKind, AbiEventField,
    AbiLifecyclePhase, AbiLinkClosedReason, AbiStatus, AbiStopReason, ApplicationEvent,
    BoundedHostQueue, ConsumerLane, DiagnosticEvent, LifecycleState, LinkClosedReason,
    PrnsLimits as CoreLimits, ResourceAvailable, StopReason, HOST_CONTRACT, HOST_SCHEMA_VERSION,
};

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
#[derive(Clone, Copy)]
pub struct PrnsLimits {
    pub struct_size: usize,
    pub pending_commands: usize,
    pub application_events: usize,
    pub retained_event_bytes: usize,
    pub diagnostics: usize,
}

#[repr(C)]
pub struct PrnsHostOptions {
    pub struct_size: usize,
    pub required_abi: u32,
    pub required_product_version: PrnsStringView,
    pub limits: PrnsLimits,
}

#[repr(C)]
pub struct PrnsLifecycle {
    pub struct_size: usize,
    pub revision: u64,
    pub phase: u32,
    pub reason: u32,
}

struct Shared {
    queue: Mutex<BoundedHostQueue<()>>,
    resources: Mutex<BTreeMap<u64, PrnsResourceStream>>,
    ready: Condvar,
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
                self.shared.ready.notify_all();
                Ok(())
            }
            Err(rejected) => {
                drop(queue);
                self.shared.ready.notify_all();
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
        self.shared.ready.notify_all();
    }

    pub fn backend_exited(&self) {
        let mut queue = lock(&self.shared.queue);
        let state = queue.lifecycle().state;
        if matches!(state, LifecycleState::Starting | LifecycleState::Running) {
            let _ = queue.transition(LifecycleState::Stopping);
        }
        if matches!(queue.lifecycle().state, LifecycleState::Stopping) {
            let _ = queue.transition(LifecycleState::Stopped(StopReason::BackendExited));
        }
        drop(queue);
        self.shared.ready.notify_all();
    }
}

pub struct PrnsHost {
    shared: Arc<Shared>,
}

pub struct PrnsEventStream {
    shared: Arc<Shared>,
    lane: ConsumerLane,
    pending_diagnostics_gap: Mutex<u128>,
}

impl Drop for PrnsEventStream {
    fn drop(&mut self) {
        lock(&self.shared.queue).release_consumer(self.lane);
        self.shared.ready.notify_all();
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

struct ResourceState {
    chunks: std::collections::VecDeque<Vec<u8>>,
    active: Option<Vec<u8>>,
    offset: usize,
}

pub fn host_capsule(limits: CoreLimits) -> (PrnsHost, HostPublisher) {
    let mut queue = BoundedHostQueue::new(limits);
    let _ = queue.transition(LifecycleState::Running);
    let shared = Arc::new(Shared {
        queue: Mutex::new(queue),
        resources: Mutex::new(BTreeMap::new()),
        ready: Condvar::new(),
    });
    (
        PrnsHost {
            shared: Arc::clone(&shared),
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
    if value.data.is_null() && value.length != 0 {
        return Err(status(AbiStatus::InvalidArgument));
    }
    let bytes = if value.length == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(value.data, value.length) }
    };
    str::from_utf8(bytes).map_err(|_| status(AbiStatus::InvalidArgument))
}

fn validate_size(actual: usize, required: usize) -> Result<(), u32> {
    if actual < required {
        Err(status(AbiStatus::InvalidArgument))
    } else {
        Ok(())
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
pub unsafe extern "C" fn prns_host_create(
    options: *const PrnsHostOptions,
    out_host: *mut *mut PrnsHost,
) -> u32 {
    catch_status(|| {
        let options = match unsafe { required_ref(options) } {
            Ok(options) => options,
            Err(error) => return error,
        };
        let out = match unsafe { required_mut(out_host) } {
            Ok(out) => out,
            Err(error) => return error,
        };
        *out = ptr::null_mut();
        if let Err(error) = validate_size(options.struct_size, size_of::<PrnsHostOptions>()) {
            return error;
        }
        if let Err(error) = validate_size(options.limits.struct_size, size_of::<PrnsLimits>()) {
            return error;
        }
        let version = match unsafe { read_string(options.required_product_version) } {
            Ok(version) => version,
            Err(error) => return error,
        };
        if verify_host_contract(options.required_abi, version).is_err() {
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
        let (host, _) = host_capsule(limits);
        *out = Box::into_raw(Box::new(host));
        status(AbiStatus::Ok)
    })
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
pub unsafe extern "C" fn prns_host_stop(host: *mut PrnsHost) -> u32 {
    catch_status(|| {
        let host = match unsafe { required_ref(host) } {
            Ok(host) => host,
            Err(error) => return error,
        };
        let mut queue = lock(&host.shared.queue);
        if queue.lifecycle().state.is_terminal() {
            return status(AbiStatus::Ok);
        }
        if queue.transition(LifecycleState::Stopping).is_err()
            || queue
                .transition(LifecycleState::Stopped(StopReason::Requested))
                .is_err()
        {
            return status(AbiStatus::BackendFailed);
        }
        drop(queue);
        host.shared.ready.notify_all();
        status(AbiStatus::Ok)
    })
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
            EventValue::Application(ApplicationEvent::ChannelMessage(value)),
            AbiEventField::MessageType,
        ) => Some(&value.message_type),
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

fn event_u64(event: &PrnsEvent, field: AbiEventField) -> Option<u64> {
    match (&event.value, field) {
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
        DestinationHash, InterfaceId, LinkId, ResourceHash, ResourceStreamId, SingleDelivery,
    };

    fn limits() -> CoreLimits {
        CoreLimits::try_new(1, 2, 64, 1).unwrap_or_else(|_| CoreLimits::balanced())
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
        let selected = limits();
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
            required_product_version: PrnsStringView {
                data: version.as_ptr(),
                length: version.len(),
            },
            limits: native_limits,
        };
        let mut host = ptr::null_mut();
        assert_eq!(
            unsafe { prns_host_create(&options, &mut host) },
            status(AbiStatus::ContractMismatch)
        );
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
