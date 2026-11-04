//! Runtime attachment ownership on the host's existing command and cleanup lanes.
//!
//! A scope is minted by an `OwnedSession`, never by a borrowed foreign client.
//! Accepted operations belong to the host. Dropping a waiter does not discard an
//! attachment or its cleanup claim. This is process ownership, not persistence.

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use personal_rns::config::PlannedMedium;
use prns_host::{
    CommandFailure, CommandOutcome, HostCommand, InterfaceConfig, InterfaceId, InterfaceKind,
    InterfaceRoutingPolicy,
};

use crate::owner::{SessionError, SessionStopped};
use crate::{
    lock, Attachment, AttachmentResource, CommandWait, HostRuntime, HostSubmitError,
    PrnsNodeHandle, RuntimeCommand, RuntimeControl,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentError {
    Closed,
    Conflict,
    NotOwned,
    Session(SessionError),
    Command(CommandFailure),
    /// This failed attach still owns a partial plan whose exact retirement must be retried.
    RetirementPending {
        interface: InterfaceId,
        failure: CommandFailure,
    },
}

pub(crate) type RetirementEvidence = Arc<Mutex<Option<(InterfaceId, CommandFailure)>>>;

/// One native owner's attachment authority. This owns no authority to stop the host.
/// Its cleanup lane remains usable after ordinary client admission is fenced.
pub struct AttachmentScope {
    host: Arc<HostRuntime>,
    claim: Arc<Claim>,
    stopped: SessionStopped,
}

pub(crate) struct Claim {
    state: Mutex<ClaimState>,
}

#[derive(Default)]
struct ClaimState {
    closed: bool,
    cleanup: Option<Arc<Completion>>,
}

impl AttachmentScope {
    pub(crate) fn new(host: Arc<HostRuntime>, stopped: SessionStopped) -> Self {
        Self {
            host,
            claim: Arc::new(Claim {
                state: Mutex::new(ClaimState::default()),
            }),
            stopped,
        }
    }

    pub async fn attach(
        &self,
        config: InterfaceConfig,
        routing: Option<InterfaceRoutingPolicy>,
    ) -> Result<InterfaceId, AttachmentError> {
        let retirement: RetirementEvidence = Arc::new(Mutex::new(None));
        let pending = {
            let state = lock(&self.claim.state);
            if state.closed {
                return Err(AttachmentError::Closed);
            }
            self.host
                .submit_native(
                    RuntimeCommand::Contract {
                        command: HostCommand::AttachInterface { config, routing },
                        claim: Some(Arc::clone(&self.claim)),
                        retirement: Some(Arc::clone(&retirement)),
                    },
                    None,
                )
                .map_err(submit_error)?
        };
        let result = pending.wait_async().await;
        let unfinished = lock(&retirement).take();
        if let Some((interface, failure)) = unfinished {
            return Err(AttachmentError::RetirementPending { interface, failure });
        }
        match result {
            CommandWait::Completed(Ok(CommandOutcome::InterfaceAttached { interface })) => {
                Ok(interface)
            }
            CommandWait::Completed(Err(CommandFailure::Busy)) => Err(AttachmentError::Conflict),
            CommandWait::Completed(Err(CommandFailure::NodeStopped))
                if lock(&self.claim.state).closed =>
            {
                Err(AttachmentError::Closed)
            }
            CommandWait::Completed(Err(error)) => Err(AttachmentError::Command(error)),
            _ => Err(AttachmentError::Session(SessionError::Stopped)),
        }
    }

    /// Cleanup is permitted after the scope is fenced, but only for its own ID.
    pub async fn detach(&self, interface: InterfaceId) -> Result<(), AttachmentError> {
        let pending = self
            .host
            .submit_native(
                RuntimeCommand::Contract {
                    command: HostCommand::DetachInterface { interface },
                    claim: Some(Arc::clone(&self.claim)),
                    retirement: None,
                },
                None,
            )
            .map_err(submit_error)?;
        match pending.wait_async().await {
            CommandWait::Completed(Ok(CommandOutcome::InterfaceDetached { .. })) => Ok(()),
            CommandWait::Completed(Err(CommandFailure::PermissionDenied { .. })) => {
                Err(AttachmentError::NotOwned)
            }
            CommandWait::Completed(Err(error)) => Err(AttachmentError::Command(error)),
            _ => Err(AttachmentError::Session(SessionError::Stopped)),
        }
    }

    /// Deny new attachment admission without retiring existing interfaces.
    /// Domain owners may drain network-dependent work before requesting cleanup.
    pub fn fence(&self) {
        lock(&self.claim.state).closed = true;
    }

    /// Fence synchronously and retain cleanup independently of an observer. The
    /// control lane cannot be rejected merely because ordinary admission is full.
    pub fn request_close(&self) {
        drop(self.begin_close());
    }

    /// Fence before returning the future. Concurrent callers join one attempt;
    /// a later explicit call may retry a completed teardown failure.
    pub fn close(&self) -> impl Future<Output = Result<(), AttachmentError>> + Send + 'static {
        let completion = self.begin_close();
        let stopped = self.stopped.clone();
        async move {
            match (Wait {
                completion,
                registration: None,
            })
            .await
            {
                Err(AttachmentError::Session(SessionError::Stopped)) => {
                    stopped.wait().await.map_err(AttachmentError::Session)
                }
                result => result,
            }
        }
    }

    /// Observe actual joined host shutdown without requesting it.
    pub fn stopped(&self) -> impl Future<Output = Result<(), SessionError>> + Send + 'static {
        let stopped = self.stopped.clone();
        async move { stopped.wait().await }
    }

    fn begin_close(&self) -> Arc<Completion> {
        let completion = {
            let mut state = lock(&self.claim.state);
            state.closed = true;
            if let Some(completion) = &state.cleanup {
                if !matches!(completion.result(), Some(Err(_))) {
                    return Arc::clone(completion);
                }
            }
            let completion = Arc::new(Completion::default());
            state.cleanup = Some(Arc::clone(&completion));
            completion
        };
        {
            // Sending/dropping the cleanup job can publish completion. Keep it
            // outside admission locks: an executor's waker is arbitrary code.
            let _ = self
                .host
                .controls
                .send(RuntimeControl::CloseAttachments(Cleanup {
                    claim: Arc::clone(&self.claim),
                    completion: Arc::clone(&completion),
                }));
        }
        completion
    }
}

impl Drop for AttachmentScope {
    fn drop(&mut self) {
        self.request_close();
    }
}

fn submit_error(error: HostSubmitError) -> AttachmentError {
    AttachmentError::Session(match error {
        HostSubmitError::Busy => SessionError::Busy,
        HostSubmitError::Stopped => SessionError::Stopped,
    })
}

pub(crate) struct Cleanup {
    claim: Arc<Claim>,
    completion: Arc<Completion>,
}

impl Cleanup {
    pub(crate) async fn execute(
        self,
        handle: &PrnsNodeHandle,
        attachments: &mut BTreeMap<InterfaceId, Attachment>,
    ) {
        let owned = attachments
            .iter()
            .filter_map(|(id, entry)| {
                entry
                    .claim
                    .as_ref()
                    .filter(|claim| Arc::ptr_eq(claim, &self.claim))
                    .map(|_| *id)
            })
            .collect::<Vec<_>>();
        let mut failure = None;
        for id in owned {
            let Some(entry) = attachments.get_mut(&id) else {
                failure.get_or_insert(AttachmentError::Command(CommandFailure::BackendFailed {
                    detail: "owned attachment disappeared before retirement".into(),
                }));
                continue;
            };
            match entry.teardown(handle).await {
                Ok(()) => {
                    attachments.remove(&id);
                }
                Err(error) => {
                    failure.get_or_insert(AttachmentError::Command(error));
                }
            }
        }
        self.completion.finish(failure.map_or(Ok(()), Err));
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.completion
            .finish(Err(AttachmentError::Session(SessionError::Stopped)));
    }
}

/// Canonical planner values normalize default settings. Routing/bitrate do not
/// make the same physical resource safe to attach again under another owner.
pub(crate) struct Signature {
    kind: InterfaceKind,
    media: Vec<PlannedMedium>,
    device: Option<String>,
}

fn signature(
    config: &InterfaceConfig,
    routing: Option<InterfaceRoutingPolicy>,
) -> Result<Signature, CommandFailure> {
    config
        .validate()
        .map_err(|error| crate::invalid_configuration(format!("{error:?}")))?;
    let plan = crate::typed_interface_plan(config, routing)?;
    let device = match config {
        InterfaceConfig::Serial { port, .. }
        | InterfaceConfig::Kiss { port, .. }
        | InterfaceConfig::Ax25Kiss { port, .. }
        | InterfaceConfig::RNode { port, .. }
        | InterfaceConfig::MultiRNode { port, .. }
        | InterfaceConfig::Weave { port } => Some(port.clone()),
        _ => None,
    };
    Ok(Signature {
        kind: config.kind(),
        media: plan
            .interfaces
            .into_iter()
            .map(|interface| interface.medium)
            .collect(),
        device,
    })
}

impl Signature {
    fn overlaps(&self, other: &Self) -> bool {
        if self.device.is_some() && self.device == other.device {
            return true;
        }
        if self.kind == other.kind
            && matches!(
                self.kind,
                InterfaceKind::AutoLan
                    | InterfaceKind::AutomaticBluetoothLe
                    | InterfaceKind::AutomaticUsb
                    | InterfaceKind::I2p
            )
        {
            return true;
        }
        self.media
            .iter()
            .any(|left| other.media.iter().any(|right| medium_overlaps(left, right)))
    }
}

/// Lexical endpoint identity; no DNS lookup or transport effect. Shared with
/// persisted profile validation so a raw spelling cannot bypass its claim.
pub(crate) fn canonical_network_host(host: &str) -> Option<String> {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return None;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty()
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn same_network_host(left: &str, right: &str) -> bool {
    match (canonical_network_host(left), canonical_network_host(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

fn medium_overlaps(left: &PlannedMedium, right: &PlannedMedium) -> bool {
    use PlannedMedium as M;
    match (left, right) {
        (
            M::TcpClient { connection: a, .. } | M::BackboneClient { connection: a },
            M::TcpClient { connection: b, .. } | M::BackboneClient { connection: b },
        ) => same_network_host(&a.host, &b.host) && a.port == b.port,
        (
            M::TcpServer { listener: a, .. }
            | M::Backbone { listener: a }
            | M::PrnsWebSocketServer { listener: a, .. },
            M::TcpServer { listener: b, .. }
            | M::Backbone { listener: b }
            | M::PrnsWebSocketServer { listener: b, .. },
        ) => {
            use personal_rns::config::TcpListenHost as H;
            a.port == b.port
                && match (&a.host, &b.host) {
                    (H::Address(a), H::Address(b)) => same_network_host(a, b),
                    _ => a.host == b.host,
                }
        }
        (M::Udp { flow: a }, M::Udp { flow: b }) => {
            use personal_rns::config::UdpFlowPlan as F;
            match (a, b) {
                (
                    F::ReceiveOnly { listen: a } | F::Bidirectional { listen: a, .. },
                    F::ReceiveOnly { listen: b } | F::Bidirectional { listen: b, .. },
                ) => a == b,
                _ => a == b,
            }
        }
        (M::Pipe { command: a, .. }, M::Pipe { command: b, .. }) => a == b,
        (M::PrnsWebSocketClient { target: a, .. }, M::PrnsWebSocketClient { target: b, .. }) => {
            a == b
        }
        _ => left == right,
    }
}

pub(crate) fn preflight(
    attachments: &BTreeMap<InterfaceId, Attachment>,
    config: &InterfaceConfig,
    routing: Option<InterfaceRoutingPolicy>,
    claim: Option<&Arc<Claim>>,
) -> Result<Signature, CommandFailure> {
    if claim.is_some_and(|claim| lock(&claim.state).closed) {
        return Err(CommandFailure::NodeStopped);
    }
    let signature = signature(config, routing)?;
    for entry in attachments.values() {
        if claim.is_none() && entry.claim.is_none() {
            continue;
        }
        let conflicts = match &entry.signature {
            Some(existing) => signature.overlaps(existing),
            // Prepared native resources have no canonical config to compare. Do
            // not adopt or overwrite them merely because their ID is observable.
            None => signature.kind == entry.kind(),
        };
        if conflicts {
            return Err(CommandFailure::Busy);
        }
    }
    Ok(signature)
}

pub(crate) async fn complete_attach(
    handle: &PrnsNodeHandle,
    attachments: &mut BTreeMap<InterfaceId, Attachment>,
    resource: AttachmentResource,
    signature: Signature,
    claim: Option<&Arc<Claim>>,
    retirement: Option<&RetirementEvidence>,
    failure: Option<CommandFailure>,
) -> Result<CommandOutcome, CommandFailure> {
    let interface = resource
        .interfaces()
        .first()
        .copied()
        .map(crate::host_interface)
        .ok_or_else(|| CommandFailure::BackendFailed {
            detail: "interface attachment has no runtime interface".into(),
        })?;
    attachments.insert(interface, tracked(resource, signature, claim));
    if let Some(failure) = failure {
        // Partial attachment is still an owned resource. Publish its exact ID if
        // cleanup fails; neither the caller nor its result observer owns the map.
        if let Err(error) = attachments
            .get_mut(&interface)
            .ok_or_else(|| CommandFailure::BackendFailed {
                detail: "partial attachment disappeared before retirement".into(),
            })?
            .teardown(handle)
            .await
        {
            if let Some(retirement) = retirement {
                *lock(retirement) = Some((interface, error.clone()));
            }
            return Err(error);
        }
        attachments.remove(&interface);
        return Err(failure);
    }
    Ok(CommandOutcome::InterfaceAttached { interface })
}

pub(crate) fn tracked(
    resource: AttachmentResource,
    signature: Signature,
    claim: Option<&Arc<Claim>>,
) -> Attachment {
    Attachment {
        resource,
        signature: Some(signature),
        claim: claim.cloned(),
    }
}

pub(crate) fn require_owner(
    attachment: &Attachment,
    claim: Option<&Arc<Claim>>,
) -> Result<(), CommandFailure> {
    match (&attachment.claim, claim) {
        (None, None) => Ok(()),
        (Some(owned), Some(requested)) if Arc::ptr_eq(owned, requested) => Ok(()),
        _ => Err(CommandFailure::PermissionDenied {
            detail: "interface belongs to a different attachment owner".into(),
        }),
    }
}

#[derive(Default)]
struct Completion {
    state: Mutex<CompletionState>,
}
#[derive(Default)]
struct CompletionState {
    result: Option<Result<(), AttachmentError>>,
    waiters: Vec<(Arc<()>, Waker)>,
}
impl Completion {
    fn result(&self) -> Option<Result<(), AttachmentError>> {
        lock(&self.state).result.clone()
    }
    fn finish(&self, result: Result<(), AttachmentError>) {
        let waiters = {
            let mut state = lock(&self.state);
            if state.result.is_some() {
                return;
            }
            state.result = Some(result);
            std::mem::take(&mut state.waiters)
        };
        for (_, waker) in waiters {
            let _ = catch_unwind(AssertUnwindSafe(|| waker.wake()));
        }
    }
}
struct Wait {
    completion: Arc<Completion>,
    registration: Option<Arc<()>>,
}
impl Future for Wait {
    type Output = Result<(), AttachmentError>;
    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let waker = context.waker().clone();
        let mut state = lock(&this.completion.state);
        if let Some(result) = &state.result {
            return Poll::Ready(result.clone());
        }
        let previous = if let Some(registration) = &this.registration {
            if let Some((_, stored)) = state
                .waiters
                .iter_mut()
                .find(|(id, _)| Arc::ptr_eq(id, registration))
            {
                Some(std::mem::replace(stored, waker))
            } else {
                state.waiters.push((Arc::clone(registration), waker));
                None
            }
        } else {
            let id = Arc::new(());
            state.waiters.push((Arc::clone(&id), waker));
            this.registration = Some(id);
            None
        };
        drop(state);
        drop(previous);
        Poll::Pending
    }
}
impl Drop for Wait {
    fn drop(&mut self) {
        if let Some(registration) = &self.registration {
            let mut state = lock(&self.completion.state);
            let removed = state
                .waiters
                .iter()
                .position(|(id, _)| Arc::ptr_eq(id, registration))
                .map(|index| state.waiters.swap_remove(index));
            drop(state);
            drop(removed);
        }
    }
}

#[cfg(test)]
mod tests;
