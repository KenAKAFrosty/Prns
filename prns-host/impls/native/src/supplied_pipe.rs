use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::future::Future;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{ready, Context, Poll};
use std::time::{Duration, Instant};

use personal_rns::interfaces::pipe as pipe_contract;
use personal_rns::interfaces::{BitrateBps, ConfiguredInterfacePolicy};
use personal_rns::pipe::{PipeInterface, PipeRespawnDelay};
use personal_rns::PrnsNodeHandle;
use prns_host::{Bitrate, CommandFailure, CommandOutcome, InterfaceId};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};

use crate::{
    engine_bitrate, host_interface, Attachment, CommandHandle, CommandReadiness, NativeCommand,
    NativeControl, NativeHost, NativeSubmitError, ReadinessSignal,
};

const CHANNEL_NAMESPACE: &[u8] = b"host-supplied-pipe:";

type OpenFuture = Pin<Box<dyn Future<Output = io::Result<FdStream>> + Send>>;

pub struct SuppliedPipeConfig {
    pub name: String,
    pub respawn_delay: Duration,
    pub bitrate: Bitrate,
}

enum AttachState {
    Pending,
    Attached,
}

pub(crate) struct SuppliedPipeAttach {
    pub config: SuppliedPipeConfig,
    pub broker: Arc<SuppliedPipeBroker>,
    state: AttachState,
}

impl SuppliedPipeAttach {
    pub(crate) fn new(config: SuppliedPipeConfig, broker: Arc<SuppliedPipeBroker>) -> Self {
        Self {
            config,
            broker,
            state: AttachState::Pending,
        }
    }

    pub(crate) fn mark_attached(&mut self) {
        self.state = AttachState::Attached;
    }
}

impl Drop for SuppliedPipeAttach {
    fn drop(&mut self) {
        match &self.state {
            AttachState::Pending => {
                self.broker.close();
            }
            AttachState::Attached => {}
        }
    }
}

enum OpenResponse {
    Descriptor(OwnedFd),
    Declined,
}

struct OpenRequest {
    reply: Mutex<Option<oneshot::Sender<OpenResponse>>>,
}

impl OpenRequest {
    fn respond(&self, response: OpenResponse) -> bool {
        let Some(reply) = lock(&self.reply).take() else {
            return false;
        };
        reply.send(response).is_ok()
    }

    fn is_pending(&self) -> bool {
        lock(&self.reply).is_some()
    }
}

pub struct SuppliedPipeOpenRequest {
    inner: Arc<OpenRequest>,
}

impl SuppliedPipeOpenRequest {
    #[must_use]
    pub fn provide(&self, descriptor: OwnedFd) -> bool {
        self.inner.respond(OpenResponse::Descriptor(descriptor))
    }

    #[must_use]
    pub fn decline(&self) -> bool {
        self.inner.respond(OpenResponse::Declined)
    }
}

impl Drop for SuppliedPipeOpenRequest {
    fn drop(&mut self) {
        let _ = self.decline();
    }
}

struct BrokerState {
    closed: bool,
    interrupted: bool,
    available: VecDeque<Arc<OpenRequest>>,
    current: Option<Weak<OpenRequest>>,
}

pub(crate) struct SuppliedPipeBroker {
    state: Mutex<BrokerState>,
    ready: Condvar,
    readiness: Option<ReadinessSignal>,
}

pub enum SuppliedPipeRequestWait {
    Request(SuppliedPipeOpenRequest),
    WouldBlock,
    TimedOut,
    Interrupted,
    Stopped,
}

pub struct NativeSuppliedPipe {
    broker: Arc<SuppliedPipeBroker>,
    attachment: Mutex<Option<CommandHandle>>,
    controls: mpsc::UnboundedSender<NativeControl>,
    closed: AtomicBool,
}

impl NativeSuppliedPipe {
    pub fn claim_attachment(&self) -> Option<CommandHandle> {
        lock(&self.attachment).take()
    }

    #[must_use]
    pub fn next_request(&self, timeout: Option<Duration>) -> SuppliedPipeRequestWait {
        self.broker.next_request(timeout)
    }

    pub fn interrupt_wait(&self) {
        self.broker.interrupt_wait();
    }

    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.broker.close();
        let _ = self
            .controls
            .send(NativeControl::DetachSuppliedPipe(Arc::clone(&self.broker)));
    }
}

impl Drop for NativeSuppliedPipe {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) struct SuppliedPipeLifetime {
    broker: Arc<SuppliedPipeBroker>,
}

impl SuppliedPipeLifetime {
    fn new(broker: Arc<SuppliedPipeBroker>) -> Self {
        Self { broker }
    }

    fn belongs_to(&self, broker: &Arc<SuppliedPipeBroker>) -> bool {
        Arc::ptr_eq(&self.broker, broker)
    }
}

impl Drop for SuppliedPipeLifetime {
    fn drop(&mut self) {
        self.broker.close();
    }
}

impl NativeHost {
    pub fn begin_supplied_pipe(
        &self,
        config: SuppliedPipeConfig,
        attachment_readiness: Option<CommandReadiness>,
        request_readiness: Option<ReadinessSignal>,
    ) -> Result<NativeSuppliedPipe, NativeSubmitError> {
        let broker = Arc::new(SuppliedPipeBroker::new(request_readiness));
        let attachment = self.submit_native(
            NativeCommand::AttachSuppliedPipe(SuppliedPipeAttach::new(config, Arc::clone(&broker))),
            attachment_readiness,
        );
        match attachment {
            Ok(attachment) => Ok(NativeSuppliedPipe {
                broker,
                attachment: Mutex::new(Some(attachment)),
                controls: self.controls.clone(),
                closed: AtomicBool::new(false),
            }),
            Err(error) => {
                broker.close();
                Err(error)
            }
        }
    }
}

impl SuppliedPipeBroker {
    pub(crate) fn new(readiness: Option<ReadinessSignal>) -> Self {
        Self {
            state: Mutex::new(BrokerState {
                closed: false,
                interrupted: false,
                available: VecDeque::new(),
                current: None,
            }),
            ready: Condvar::new(),
            readiness,
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        lock(&self.state).closed
    }

    pub(crate) fn close(&self) -> bool {
        let current = {
            let mut state = lock(&self.state);
            if state.closed {
                return false;
            }
            state.closed = true;
            state.available.clear();
            state.current.take().and_then(|request| request.upgrade())
        };
        if let Some(current) = current {
            current.respond(OpenResponse::Declined);
        }
        self.notify();
        true
    }

    pub(crate) fn interrupt_wait(&self) {
        lock(&self.state).interrupted = true;
        self.notify();
    }

    pub(crate) fn next_request(&self, timeout: Option<Duration>) -> SuppliedPipeRequestWait {
        let started = Instant::now();
        let mut state = lock(&self.state);
        loop {
            while state
                .available
                .front()
                .is_some_and(|request| !request.is_pending())
            {
                state.available.pop_front();
            }
            if let Some(request) = state.available.pop_front() {
                return SuppliedPipeRequestWait::Request(SuppliedPipeOpenRequest {
                    inner: request,
                });
            }
            if state.closed {
                return SuppliedPipeRequestWait::Stopped;
            }
            if state.interrupted {
                return SuppliedPipeRequestWait::Interrupted;
            }
            match timeout {
                None => {
                    state = self
                        .ready
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(timeout) if timeout.is_zero() => {
                    return SuppliedPipeRequestWait::WouldBlock;
                }
                Some(timeout) => {
                    let remaining = timeout.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        return SuppliedPipeRequestWait::TimedOut;
                    }
                    let waited = self
                        .ready
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(PoisonError::into_inner);
                    state = waited.0;
                    if waited.1.timed_out() {
                        return SuppliedPipeRequestWait::TimedOut;
                    }
                }
            }
        }
    }

    async fn open(self: &Arc<Self>) -> io::Result<FdStream> {
        let (reply, response) = oneshot::channel();
        let request = Arc::new(OpenRequest {
            reply: Mutex::new(Some(reply)),
        });
        {
            let mut state = lock(&self.state);
            if state.closed {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "the supplied pipe controller is closed",
                ));
            }
            state.current = Some(Arc::downgrade(&request));
            state.available.push_back(Arc::clone(&request));
        }
        self.notify();
        let pending = PendingOpen {
            broker: Arc::clone(self),
            request: Arc::clone(&request),
        };
        let response = response.await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "the supplied pipe request was cancelled",
            )
        })?;
        drop(pending);
        match response {
            OpenResponse::Descriptor(descriptor) => FdStream::supplied(descriptor),
            OpenResponse::Declined => Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "the application declined to supply a pipe",
            )),
        }
    }

    fn finish(&self, request: &Arc<OpenRequest>) {
        let mut state = lock(&self.state);
        if state
            .current
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|current| Arc::ptr_eq(&current, request))
        {
            state.current = None;
        }
        state
            .available
            .retain(|available| !Arc::ptr_eq(available, request));
    }

    fn notify(&self) {
        self.ready.notify_all();
        if let Some(readiness) = &self.readiness {
            readiness();
        }
    }
}

struct PendingOpen {
    broker: Arc<SuppliedPipeBroker>,
    request: Arc<OpenRequest>,
}

impl Drop for PendingOpen {
    fn drop(&mut self) {
        self.request.respond(OpenResponse::Declined);
        self.broker.finish(&self.request);
    }
}

pub(super) fn attach(
    handle: &PrnsNodeHandle,
    attachments: &mut BTreeMap<InterfaceId, Attachment>,
    attach: &mut SuppliedPipeAttach,
) -> Result<CommandOutcome, CommandFailure> {
    let result = (|| {
        if attach.broker.is_closed() {
            return Err(CommandFailure::NodeStopped);
        }
        if attach.config.name.is_empty() {
            return Err(CommandFailure::InvalidConfiguration {
                detail: "a supplied pipe needs a non-empty name".to_string(),
            });
        }
        let pipe = supplied_pipe_interface(attach, engine_bitrate(attach.config.bitrate)?);
        let interface = host_interface(pipe.id());
        if attachments.contains_key(&interface) {
            return Err(CommandFailure::InvalidConfiguration {
                detail: format!(
                    "a supplied pipe named {:?} is already attached",
                    attach.config.name
                ),
            });
        }
        if attach.broker.is_closed() {
            return Err(CommandFailure::NodeStopped);
        }
        let attached = handle.add_interface(pipe);
        attachments.insert(
            interface,
            Attachment::SuppliedPipe {
                attachment: attached,
                lifetime: SuppliedPipeLifetime::new(Arc::clone(&attach.broker)),
            },
        );
        attach.mark_attached();
        Ok(CommandOutcome::InterfaceAttached { interface })
    })();
    if result.is_err() {
        attach.broker.close();
    }
    result
}

pub(super) async fn detach(
    handle: &PrnsNodeHandle,
    attachments: &mut BTreeMap<InterfaceId, Attachment>,
    broker: Arc<SuppliedPipeBroker>,
) {
    let interface = attachments
        .iter()
        .find_map(|(interface, attachment)| match attachment {
            Attachment::SuppliedPipe { lifetime, .. } if lifetime.belongs_to(&broker) => {
                Some(*interface)
            }
            _ => None,
        });
    if let Some(interface) = interface {
        if let Some(attachment) = attachments.remove(&interface) {
            attachment.teardown(handle).await;
        }
    }
}

fn supplied_pipe_interface(
    attach: &SuppliedPipeAttach,
    bitrate: BitrateBps,
) -> PipeInterface<impl FnMut() -> OpenFuture> {
    let mut channel_tag = CHANNEL_NAMESPACE.to_vec();
    channel_tag.extend_from_slice(attach.config.name.as_bytes());
    PipeInterface::with_policy(
        opener(Arc::clone(&attach.broker)),
        PipeRespawnDelay::new(attach.config.respawn_delay),
        pipe_contract::configured_policy(ConfiguredInterfacePolicy {
            bitrate: Some(bitrate),
            ..ConfiguredInterfacePolicy::default()
        }),
        &channel_tag,
    )
}

fn opener(broker: Arc<SuppliedPipeBroker>) -> impl FnMut() -> OpenFuture {
    move || {
        let broker = Arc::clone(&broker);
        Box::pin(async move { broker.open().await }) as OpenFuture
    }
}

pub(crate) struct FdStream {
    fd: AsyncFd<File>,
}

impl FdStream {
    fn supplied(fd: OwnedFd) -> io::Result<Self> {
        Ok(Self {
            fd: AsyncFd::new(File::from(fd))?,
        })
    }
}

impl AsyncRead for FdStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let stream = self.get_mut();
        loop {
            let mut guard = ready!(stream.fd.poll_read_ready(cx))?;
            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|inner| {
                let mut file = inner.get_ref();
                file.read(unfilled)
            }) {
                Ok(Ok(read)) => {
                    buf.advance(read);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_would_block) => {}
            }
        }
    }
}

impl AsyncWrite for FdStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let stream = self.get_mut();
        loop {
            let mut guard = ready!(stream.fd.poll_write_ready(cx))?;
            match guard.try_io(|inner| {
                let mut file = inner.get_ref();
                file.write(buf)
            }) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => {}
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Writes go straight to the unbuffered descriptor, so there is nothing to flush here.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Generic Unix descriptors have no universal half-close; dropping FdStream closes it.
        Poll::Ready(Ok(()))
    }
}

// The broker protects only simple coordination state. Recovering a poisoned guard keeps request settlement and resource teardown available instead of cascading another panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn request_handles_settle_only_once() -> Result<(), String> {
        let broker = Arc::new(SuppliedPipeBroker::new(None));
        let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
        let opening = {
            let broker = Arc::clone(&broker);
            runtime.spawn(async move { broker.open().await })
        };
        let request = match broker.next_request(Some(Duration::from_secs(1))) {
            SuppliedPipeRequestWait::Request(request) => request,
            _ => return Err("the broker did not publish its request".to_string()),
        };
        assert!(request.decline());
        assert!(!request.decline());
        assert!(runtime
            .block_on(opening)
            .map_err(|error| error.to_string())?
            .is_err());
        Ok(())
    }

    #[test]
    fn abandoned_attachment_stops_its_request_broker() {
        let broker = Arc::new(SuppliedPipeBroker::new(None));
        drop(SuppliedPipeAttach::new(
            SuppliedPipeConfig {
                name: "abandoned".to_string(),
                respawn_delay: Duration::from_millis(10),
                bitrate: Bitrate::Auto,
            },
            Arc::clone(&broker),
        ));
        assert!(matches!(
            broker.next_request(Some(Duration::ZERO)),
            SuppliedPipeRequestWait::Stopped
        ));
    }

    #[test]
    fn closing_rejects_a_descriptor_without_leaking_it() -> Result<(), String> {
        use std::os::fd::OwnedFd;

        let broker = Arc::new(SuppliedPipeBroker::new(None));
        let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
        let opening = {
            let broker = Arc::clone(&broker);
            runtime.spawn(async move { broker.open().await })
        };
        let request = match broker.next_request(Some(Duration::from_secs(1))) {
            SuppliedPipeRequestWait::Request(request) => request,
            _ => return Err("the broker did not publish its request".to_string()),
        };
        assert!(broker.close());
        let (wire, _peer) = UnixStream::pair().map_err(|error| error.to_string())?;
        assert!(!request.provide(OwnedFd::from(wire)));
        assert!(runtime
            .block_on(opening)
            .map_err(|error| error.to_string())?
            .is_err());
        Ok(())
    }

    #[test]
    fn provide_and_close_have_exactly_one_winner() -> Result<(), String> {
        use std::os::fd::OwnedFd;
        use std::sync::Barrier;

        let broker = Arc::new(SuppliedPipeBroker::new(None));
        let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
        let opening = {
            let broker = Arc::clone(&broker);
            runtime.spawn(async move { broker.open().await })
        };
        let request = match broker.next_request(Some(Duration::from_secs(1))) {
            SuppliedPipeRequestWait::Request(request) => Arc::new(request),
            _ => return Err("the broker did not publish its request".to_string()),
        };
        let (wire, _peer) = UnixStream::pair().map_err(|error| error.to_string())?;
        wire.set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let start = Arc::new(Barrier::new(3));
        let closing = {
            let broker = Arc::clone(&broker);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                broker.close()
            })
        };
        let providing = {
            let request = Arc::clone(&request);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                request.provide(OwnedFd::from(wire))
            })
        };
        start.wait();
        let _closed = closing.join().map_err(|_| "close panicked".to_string())?;
        let accepted = providing
            .join()
            .map_err(|_| "provide panicked".to_string())?;
        let opened = runtime
            .block_on(opening)
            .map_err(|error| error.to_string())?;
        assert_eq!(accepted, opened.is_ok());
        assert!(!request.decline());
        Ok(())
    }
}
