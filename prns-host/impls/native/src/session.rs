//! Shared bounded native event session. All ABI adapters use this owner so queue,
//! resource, consumer, and lifecycle semantics have one implementation.

use crate::{lock, remote_control_service::NativeRemoteControlEvent, NativeEventSink, Readiness};
use prns_host::{
    ApplicationEvent, BoundedHostQueue, ConsumerLane, ConsumerUnavailable, DiagnosticEvent,
    HostFailure, LifecycleSnapshot, LifecycleState, PrnsLimits, ResourceAvailable, StopReason,
};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[allow(clippy::large_enum_variant)]
enum QueuedApplicationEvent {
    Host(ApplicationEvent),
    RemoteControl(NativeRemoteControlEvent),
}

impl prns_host::RetainedApplicationEvent for QueuedApplicationEvent {
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Host(event) => event.retained_bytes(),
            Self::RemoteControl(event) => event.retained_bytes(),
        }
    }
}

struct Shared {
    queue: Mutex<BoundedHostQueue<(), QueuedApplicationEvent>>,
    resources: Mutex<BTreeMap<u64, NativeResourceStream>>,
    ready: Condvar,
    application_readiness: Arc<Readiness>,
    diagnostic_readiness: Arc<Readiness>,
    stop_requested: AtomicBool,
    pending_diagnostics_gap: Mutex<u128>,
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

pub struct NativeSessionEvents {
    shared: Arc<Shared>,
}

impl Clone for NativeSessionEvents {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl NativeSessionEvents {
    pub fn publish_application(&self, event: ApplicationEvent) -> Result<(), ApplicationEvent> {
        self.publish_queued(QueuedApplicationEvent::Host(event))
            .map_err(|event| match event {
                QueuedApplicationEvent::Host(event) => event,
                QueuedApplicationEvent::RemoteControl(_) => {
                    unreachable!("queue preserves rejected event type")
                }
            })
    }

    // Rejection returns the original owned event without another allocation.
    #[allow(clippy::result_large_err)]
    pub fn publish_remote_control(
        &self,
        event: NativeRemoteControlEvent,
    ) -> Result<(), NativeRemoteControlEvent> {
        self.publish_queued(QueuedApplicationEvent::RemoteControl(event))
            .map_err(|event| match event {
                QueuedApplicationEvent::RemoteControl(event) => event,
                QueuedApplicationEvent::Host(_) => {
                    unreachable!("queue preserves rejected event type")
                }
            })
    }

    #[allow(clippy::result_large_err)]
    fn publish_queued(&self, event: QueuedApplicationEvent) -> Result<(), QueuedApplicationEvent> {
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
            NativeResourceStream {
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

    pub fn transition_running(&self) {
        let mut queue = lock(&self.shared.queue);
        if matches!(queue.lifecycle().state, LifecycleState::Starting) {
            let _ = queue.transition(LifecycleState::Running);
        }
        drop(queue);
        self.shared.notify_all();
    }

    pub fn request_stop(&self) {
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

    pub fn finish_stop(&self) {
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

    pub fn fail(&self, detail: String) {
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

impl NativeEventSink for NativeSessionEvents {
    fn publish_remote_control(&self, event: NativeRemoteControlEvent) -> bool {
        self.publish_remote_control(event).is_ok()
    }

    fn running(&self) {
        self.transition_running();
    }

    fn publish_application(&self, event: ApplicationEvent) -> bool {
        NativeSessionEvents::publish_application(self, event).is_ok()
    }

    fn publish_resource(&self, event: ResourceAvailable, body: Vec<u8>) -> bool {
        NativeSessionEvents::publish_resource(self, event, body).is_ok()
    }

    fn publish_diagnostic(&self, event: DiagnosticEvent) {
        NativeSessionEvents::publish_diagnostic(self, event);
    }

    fn stopped(&self) {
        self.finish_stop();
    }

    fn failed(&self, detail: String) {
        self.fail(detail);
    }
}

impl NativeSessionEvents {
    pub fn new(limits: PrnsLimits, running: bool) -> Self {
        let mut queue = BoundedHostQueue::new(limits);
        if running {
            let _ = queue.transition(LifecycleState::Running);
        }
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(queue),
                resources: Mutex::new(BTreeMap::new()),
                ready: Condvar::new(),
                application_readiness: Arc::new(Readiness::new()),
                diagnostic_readiness: Arc::new(Readiness::new()),
                stop_requested: AtomicBool::new(false),
                pending_diagnostics_gap: Mutex::new(0),
            }),
        }
    }

    pub fn lifecycle(&self) -> LifecycleSnapshot {
        lock(&self.shared.queue).lifecycle()
    }

    /// Observe node exit without taking ownership of either event lane. This signals
    /// native runtime termination; the owning session still joins before releasing storage.
    pub async fn wait_terminal(&self) -> LifecycleSnapshot {
        let mut changed = self.shared.application_readiness.subscribe();
        loop {
            let snapshot = self.lifecycle();
            if snapshot.state.is_terminal() {
                return snapshot;
            }
            let _ = changed.changed().await;
        }
    }

    pub fn claim_stream(
        &self,
        lane: ConsumerLane,
    ) -> Result<NativeEventStream, ConsumerUnavailable> {
        lock(&self.shared.queue).claim_consumer(lane)?;
        Ok(NativeEventStream {
            shared: Arc::clone(&self.shared),
            lane,
            interrupted: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }
}

pub enum NativeEventValue {
    Application(ApplicationEvent),
    RemoteControl(NativeRemoteControlEvent),
    Diagnostic(DiagnosticEvent),
    DiagnosticsDropped(u128),
}

pub struct NativeEvent {
    pub value: NativeEventValue,
    pub resource: Option<NativeResourceStream>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStreamError {
    WouldBlock,
    TimedOut,
    Interrupted,
    Stopped,
}

pub struct NativeEventStream {
    shared: Arc<Shared>,
    lane: ConsumerLane,
    interrupted: AtomicBool,
    closed: AtomicBool,
}

impl Drop for NativeEventStream {
    fn drop(&mut self) {
        self.close();
    }
}

impl NativeEventStream {
    /// Invalidates this lease and quiesces its bounded synchronous drain before reclaim.
    /// A retained old future/Arc cannot clear or release a successor's registration.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.readiness().clear();
        lock(&self.shared.queue).release_consumer(self.lane);
        self.shared.notify_all();
    }

    pub fn readiness(&self) -> &Arc<Readiness> {
        self.shared.readiness(self.lane)
    }

    pub fn interrupt_wait(&self) {
        self.interrupted.store(true, Ordering::Release);
        self.shared.notify_lane(self.lane);
    }

    /// Wait without dequeuing. Cancelling this future leaves queued ownership intact.
    /// `try_next` performs the subsequent bounded, nonblocking ownership transfer.
    pub async fn ready(&self) -> Result<(), NativeStreamError> {
        let mut changed = self.readiness().subscribe();
        loop {
            if self.closed.load(Ordering::Acquire) {
                return Err(NativeStreamError::Stopped);
            }
            if self.interrupted.swap(false, Ordering::AcqRel) {
                return Err(NativeStreamError::Interrupted);
            }
            {
                let queue = lock(&self.shared.queue);
                let depths = queue.depths();
                let available = match self.lane {
                    ConsumerLane::ApplicationEvents => depths.application_events > 0,
                    ConsumerLane::Diagnostics => {
                        depths.diagnostics > 0
                            || depths.dropped_diagnostics > 0
                            || *lock(&self.shared.pending_diagnostics_gap) > 0
                    }
                };
                if available {
                    return Ok(());
                }
                if queue.lifecycle().state.is_terminal() {
                    return Err(NativeStreamError::Stopped);
                }
            }
            if changed.changed().await.is_err() {
                return Err(NativeStreamError::Stopped);
            }
        }
    }

    pub fn try_next(&self) -> Result<NativeEvent, NativeStreamError> {
        self.next(Some(Duration::ZERO))
    }

    pub fn next(&self, timeout: Option<Duration>) -> Result<NativeEvent, NativeStreamError> {
        let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
        let mut queue = lock(&self.shared.queue);
        loop {
            if self.closed.load(Ordering::Acquire) {
                return Err(NativeStreamError::Stopped);
            }
            if self.interrupted.swap(false, Ordering::AcqRel) {
                return Err(NativeStreamError::Interrupted);
            }
            if let Some(event) = self.pop_event(&mut queue) {
                return Ok(event);
            }
            if queue.lifecycle().state.is_terminal() {
                return Err(NativeStreamError::Stopped);
            }
            if timeout == Some(Duration::ZERO) {
                return Err(NativeStreamError::WouldBlock);
            }
            match deadline {
                None => {
                    queue = self
                        .shared
                        .ready
                        .wait(queue)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(NativeStreamError::TimedOut);
                    }
                    let waited = self
                        .shared
                        .ready
                        .wait_timeout(queue, remaining)
                        .unwrap_or_else(PoisonError::into_inner);
                    if waited.1.timed_out() {
                        return Err(NativeStreamError::TimedOut);
                    }
                    queue = waited.0;
                }
            }
        }
    }

    fn pop_event(
        &self,
        queue: &mut BoundedHostQueue<(), QueuedApplicationEvent>,
    ) -> Option<NativeEvent> {
        let mut pending_gap = lock(&self.shared.pending_diagnostics_gap);
        if self.lane == ConsumerLane::Diagnostics && *pending_gap > 0 {
            let dropped = std::mem::take(&mut *pending_gap);
            return Some(NativeEvent {
                value: NativeEventValue::DiagnosticsDropped(dropped),
                resource: None,
            });
        }
        match self.lane {
            ConsumerLane::ApplicationEvents => queue.pop_application_event().map(|event| {
                let resource = match &event {
                    QueuedApplicationEvent::Host(ApplicationEvent::ResourceAvailable(value)) => {
                        lock(&self.shared.resources).remove(&value.stream_id.get())
                    }
                    _ => None,
                };
                NativeEvent {
                    value: match event {
                        QueuedApplicationEvent::Host(event) => NativeEventValue::Application(event),
                        QueuedApplicationEvent::RemoteControl(event) => {
                            NativeEventValue::RemoteControl(event)
                        }
                    },
                    resource,
                }
            }),
            ConsumerLane::Diagnostics => {
                let mut batch = queue.drain_diagnostics(1);
                if let Some(event) = batch.events.pop() {
                    *pending_gap = batch.dropped_newest;
                    Some(NativeEvent {
                        value: NativeEventValue::Diagnostic(event),
                        resource: None,
                    })
                } else if batch.dropped_newest > 0 {
                    Some(NativeEvent {
                        value: NativeEventValue::DiagnosticsDropped(batch.dropped_newest),
                        resource: None,
                    })
                } else {
                    None
                }
            }
        }
    }
}

pub struct NativeResourceStream {
    state: Mutex<ResourceState>,
}

struct ResourceState {
    chunks: VecDeque<Vec<u8>>,
    active: Option<Vec<u8>>,
    offset: usize,
}

impl NativeResourceStream {
    /// The borrowed view remains valid until the next read or stream drop. A foreign
    /// adapter can retain that view under its own serialized-access contract.
    pub fn with_next_chunk<T>(
        &self,
        maximum_bytes: usize,
        view: impl FnOnce(Option<&[u8]>) -> T,
    ) -> T {
        let mut state = lock(&self.state);
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
                return view(None);
            };
            if active.is_empty() {
                state.active = None;
                continue;
            }
            let start = state.offset;
            let end = start.saturating_add(maximum_bytes).min(active.len());
            state.offset = end;
            let active = state.active.as_deref().unwrap_or(&[]);
            return view(Some(&active[start..end]));
        }
    }

    pub fn read_chunk(&self, maximum_bytes: usize) -> Option<Vec<u8>> {
        self.with_next_chunk(maximum_bytes, |chunk| chunk.map(<[u8]>::to_vec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_host::{
        DestinationHash, InterfaceId, LinkId, ResourceHash, ResourceStreamId, SingleDelivery,
    };
    use std::future::Future;
    use std::sync::atomic::AtomicUsize;
    use std::task::{Context, Poll, Waker};

    fn event() -> ApplicationEvent {
        ApplicationEvent::SingleDelivery(SingleDelivery {
            destination: DestinationHash::new([1; 16]),
            source_interface: InterfaceId::new([2; 8]),
            plaintext: vec![3, 4],
        })
    }

    #[test]
    fn cancelled_readiness_never_consumes_an_event() -> Result<(), String> {
        let session = NativeSessionEvents::new(PrnsLimits::balanced(), true);
        let stream = session
            .claim_stream(ConsumerLane::ApplicationEvents)
            .map_err(|e| format!("{e:?}"))?;
        let mut readiness = Box::pin(stream.ready());
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            readiness.as_mut().poll(&mut context),
            Poll::Pending
        ));
        session
            .publish_application(event())
            .map_err(|_| "publish failed")?;
        assert!(matches!(
            readiness.as_mut().poll(&mut context),
            Poll::Ready(Ok(()))
        ));
        drop(readiness);
        assert!(matches!(
            stream.try_next(),
            Ok(NativeEvent {
                value: NativeEventValue::Application(_),
                ..
            })
        ));
        assert!(matches!(
            stream.try_next(),
            Err(NativeStreamError::WouldBlock)
        ));
        Ok(())
    }

    #[test]
    fn closing_retained_stream_reclaims_without_old_drop_releasing_successor() -> Result<(), String>
    {
        let session = NativeSessionEvents::new(PrnsLimits::balanced(), true);
        let old = session
            .claim_stream(ConsumerLane::ApplicationEvents)
            .map_err(|e| format!("{e:?}"))?;
        let mut pending = Box::pin(old.ready());
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(pending.as_mut().poll(&mut context), Poll::Pending));
        old.close();
        let successor = session
            .claim_stream(ConsumerLane::ApplicationEvents)
            .map_err(|e| format!("{e:?}"))?;
        let signals = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&signals);
        let registration = successor
            .readiness()
            .register(Arc::new(move || {
                captured.fetch_add(1, Ordering::AcqRel);
            }))
            .map_err(|e| format!("{e:?}"))?;
        assert!(matches!(
            pending.as_mut().poll(&mut context),
            Poll::Ready(Err(NativeStreamError::Stopped))
        ));
        drop(pending);
        drop(old);
        assert!(session
            .claim_stream(ConsumerLane::ApplicationEvents)
            .is_err());
        session
            .publish_application(event())
            .map_err(|_| "publish failed")?;
        assert_eq!(signals.load(Ordering::Acquire), 1);
        assert!(successor.try_next().is_ok());
        successor.readiness().unregister(&registration);
        Ok(())
    }

    #[test]
    fn retained_resource_outlives_its_event_lane_and_session() -> Result<(), String> {
        let session = NativeSessionEvents::new(PrnsLimits::balanced(), true);
        let stream = session
            .claim_stream(ConsumerLane::ApplicationEvents)
            .map_err(|e| format!("{e:?}"))?;
        session
            .publish_resource(
                ResourceAvailable {
                    stream_id: ResourceStreamId::new(8),
                    link_id: LinkId::new([4; 16]),
                    hash: ResourceHash::new([5; 32]),
                    metadata: None,
                    total_bytes: 3,
                },
                vec![1, 2, 3],
            )
            .map_err(|_| "publish failed")?;
        let resource = stream
            .try_next()
            .map_err(|e| format!("{e:?}"))?
            .resource
            .ok_or("resource missing")?;
        drop(stream);
        drop(session);
        assert_eq!(resource.read_chunk(2), Some(vec![1, 2]));
        assert_eq!(resource.read_chunk(2), Some(vec![3]));
        assert_eq!(resource.read_chunk(2), None);
        Ok(())
    }

    #[test]
    fn diagnostic_gap_survives_consumer_replacement_without_leaking_to_application_lane(
    ) -> Result<(), String> {
        let limits = PrnsLimits::try_new(1, 1, 128, 1).map_err(|e| format!("{e:?}"))?;
        let session = NativeSessionEvents::new(limits, true);
        for _ in 0..3 {
            session.publish_diagnostic(DiagnosticEvent::BackendDiagnostic {
                kind: "probe".into(),
                detail: "probe".into(),
            });
        }
        let first = session
            .claim_stream(ConsumerLane::Diagnostics)
            .map_err(|e| format!("{e:?}"))?;
        assert!(matches!(
            first.try_next(),
            Ok(NativeEvent {
                value: NativeEventValue::Diagnostic(_),
                ..
            })
        ));
        drop(first);
        let application = session
            .claim_stream(ConsumerLane::ApplicationEvents)
            .map_err(|e| format!("{e:?}"))?;
        assert!(matches!(
            application.try_next(),
            Err(NativeStreamError::WouldBlock)
        ));
        let second = session
            .claim_stream(ConsumerLane::Diagnostics)
            .map_err(|e| format!("{e:?}"))?;
        assert!(matches!(
            second.try_next(),
            Ok(NativeEvent {
                value: NativeEventValue::DiagnosticsDropped(2),
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn application_pressure_fails_session_without_replacing_retained_event() -> Result<(), String> {
        let limits = PrnsLimits::try_new(1, 1, 128, 1).map_err(|e| format!("{e:?}"))?;
        let session = NativeSessionEvents::new(limits, true);
        session
            .publish_application(event())
            .map_err(|_| "first publish failed")?;
        assert!(session.publish_application(event()).is_err());
        assert!(matches!(
            session.lifecycle().state,
            LifecycleState::Failed(_)
        ));
        let stream = session
            .claim_stream(ConsumerLane::ApplicationEvents)
            .map_err(|e| format!("{e:?}"))?;
        assert!(stream.try_next().is_ok());
        assert!(matches!(stream.try_next(), Err(NativeStreamError::Stopped)));
        Ok(())
    }
}
