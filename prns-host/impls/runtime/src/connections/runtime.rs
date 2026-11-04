use super::{
    binding::{AttachmentState, BindingSnapshot, CloseFuture, Control, LiveProfile},
    reply,
    store::Store,
    Error, Guard, Job, Profile, ProfileId, Snapshot,
};
use crate::{attachment_scope::AttachmentError, owner::SessionError};
use prns_host::{InterfaceConfig, InterfaceId};
use prns_storage::OwnerProgress;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{atomic::Ordering, Arc},
    task::{Context, Poll, Waker},
};

type EffectFuture = Pin<Box<dyn Future<Output = Result<Effect, AttachmentError>> + Send>>;
type StopFuture = Pin<Box<dyn Future<Output = Result<(), SessionError>> + Send>>;
enum Effect {
    Attached(InterfaceId),
    Detached,
}
struct Pending {
    id: ProfileId,
    future: EffectFuture,
}
struct Entry {
    live: LiveProfile,
    attached_revision: u64,
    retire_attempt: Option<u64>,
}
struct Bound {
    control: Arc<Control>,
    entries: BTreeMap<ProfileId, Entry>,
    retry: BTreeSet<ProfileId>,
    pending: Option<Pending>,
    closing: Option<(u64, CloseFuture)>,
    stopped: Option<StopFuture>,
    applied: Option<u64>,
}
impl Bound {
    fn new(control: Arc<Control>) -> Self {
        let scope = control.scope.clone();
        Self {
            control,
            entries: BTreeMap::new(),
            retry: BTreeSet::new(),
            pending: None,
            closing: None,
            stopped: Some(Box::pin(async move { scope.stopped().await })),
            applied: None,
        }
    }
    fn snapshot(&self, saved: Guard) -> BindingSnapshot {
        BindingSnapshot {
            generation: self.control.generation,
            saved,
            applied_revision: if self.control.fenced.load(Ordering::Acquire) {
                None
            } else {
                self.applied
            },
            phase: self.control.status().phase,
            attachments: self.entries.values().map(|e| e.live.clone()).collect(),
        }
    }
    fn poll(&mut self, saved: Option<&Snapshot>, context: &mut Context<'_>) -> bool {
        // A real joined host stop also retires every claimed attachment. Merely
        // losing a command reply or seeing Stopped is insufficient evidence.
        if let Some(stopped) = &mut self.stopped {
            if let Poll::Ready(result) = stopped.as_mut().poll(context) {
                self.stopped = None;
                match result {
                    Ok(()) => {
                        self.control.finish();
                        return true;
                    }
                    Err(error) => {
                        self.control.request_close();
                        self.control.failed(
                            self.control.close_attempt.load(Ordering::Acquire),
                            Error::Host(error),
                        );
                    }
                }
            }
        }
        if self.control.fenced.load(Ordering::Acquire) {
            self.applied = None;
        }
        if self.control.closing.load(Ordering::Acquire) {
            if let Some(pending) = self.control.take_close() {
                self.closing = Some(pending);
            }
            if let Some((attempt, future)) = &mut self.closing {
                if let Poll::Ready(result) = future.as_mut().poll(context) {
                    let attempt = *attempt;
                    self.closing = None;
                    match result {
                        Ok(()) => {
                            self.control.finish();
                            return true;
                        }
                        Err(error) => self.control.failed(attempt, Error::Attachment(error)),
                    }
                }
            }
        }
        if let Some(pending) = &mut self.pending {
            if let Poll::Ready(result) = pending.future.as_mut().poll(context) {
                let id = pending.id;
                self.pending = None;
                match result {
                    Ok(Effect::Detached) => {
                        self.entries.remove(&id);
                    }
                    Ok(Effect::Attached(interface)) => {
                        if let Some(entry) = self.entries.get_mut(&id) {
                            entry.live.interface = Some(interface);
                            entry.live.state = AttachmentState::Attached;
                            entry.live.failure = None;
                        }
                    }
                    Err(error) => {
                        if let Some(entry) = self.entries.get_mut(&id) {
                            if let AttachmentError::RetirementPending { interface, .. } = &error {
                                // The host retained a partially attached plan after
                                // failed rollback. Its exact claim must be retired
                                // before this profile can disappear or be replaced.
                                entry.live.interface = Some(*interface);
                                entry.live.state = AttachmentState::Retiring;
                                entry.retire_attempt = Some(entry.attached_revision);
                            } else if entry.live.interface.is_none() {
                                entry.live.state = AttachmentState::Failed;
                            }
                            entry.live.failure = Some(Error::Attachment(error));
                        }
                    }
                }
                self.control.progress.wake_by_ref();
            }
        }
        if self.control.fenced.load(Ordering::Acquire) || self.pending.is_some() {
            return false;
        }
        let Some(saved) = saved else {
            return false;
        };
        self.schedule(saved);
        if self.pending.is_some() {
            self.control.progress.wake_by_ref();
        }
        false
    }
    fn schedule(&mut self, saved: &Snapshot) {
        let desired: BTreeMap<_, _> = saved
            .profiles
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.id, p))
            .collect();
        let obsolete = self.entries.iter().find_map(|(id, entry)| {
            let changed = desired
                .get(id)
                .is_none_or(|p| !same_config(&entry.live.profile, p));
            ((changed || entry.live.state == AttachmentState::Retiring || self.retry.contains(id))
                && (entry.retire_attempt != Some(saved.guard.revision) || self.retry.contains(id)))
            .then_some(*id)
        });
        if let Some(id) = obsolete {
            self.retry.remove(&id);
            let Some(entry) = self.entries.get_mut(&id) else {
                return;
            };
            entry.live.state = AttachmentState::Retiring;
            entry.retire_attempt = Some(saved.guard.revision);
            entry.live.failure = None;
            self.applied = None;
            if let Some(interface) = entry.live.interface {
                let scope = self.control.scope.clone();
                self.pending = Some(Pending {
                    id,
                    future: Box::pin(async move {
                        scope.detach(interface).await.map(|()| Effect::Detached)
                    }),
                });
            } else {
                self.entries.remove(&id);
                self.control.progress.wake_by_ref();
            }
            return;
        }
        for (id, profile) in &desired {
            if let Some(entry) = self.entries.get_mut(id) {
                if entry.live.interface.is_some() || entry.live.state == AttachmentState::Retiring {
                    if entry.live.state == AttachmentState::Attached
                        && same_config(&entry.live.profile, profile)
                    {
                        entry.live.profile = (*profile).clone();
                    }
                    continue;
                }
                if entry.attached_revision == saved.guard.revision && !self.retry.contains(id) {
                    continue;
                }
            }
            // Profile IDs cannot bypass the previous physical medium's ownership.
            if self.entries.iter().any(|(other, e)| {
                other != id && same_medium(&e.live.profile.config, &profile.config)
            }) {
                continue;
            }
            self.retry.remove(id);
            self.applied = None;
            self.entries.insert(
                *id,
                Entry {
                    live: LiveProfile {
                        profile: (*profile).clone(),
                        interface: None,
                        state: AttachmentState::Attaching,
                        failure: None,
                    },
                    attached_revision: saved.guard.revision,
                    retire_attempt: None,
                },
            );
            let scope = self.control.scope.clone();
            let config = profile.config.clone();
            let routing = profile.routing;
            self.pending = Some(Pending {
                id: *id,
                future: Box::pin(async move {
                    scope.attach(config, routing).await.map(Effect::Attached)
                }),
            });
            return;
        }
        self.applied = (self.entries.len() == desired.len()
            && desired.iter().all(|(id, p)| {
                self.entries.get(id).is_some_and(|e| {
                    e.live.state == AttachmentState::Attached && same_config(&e.live.profile, p)
                })
            }))
        .then_some(saved.guard.revision);
    }
}
fn same_config(a: &Profile, b: &Profile) -> bool {
    a.config == b.config && a.routing == b.routing
}
fn same_medium(a: &InterfaceConfig, b: &InterfaceConfig) -> bool {
    match (a, b) {
        (
            InterfaceConfig::TcpClient { target: a, .. },
            InterfaceConfig::TcpClient { target: b, .. },
        ) => a == b,
        (InterfaceConfig::AutoLan { .. }, InterfaceConfig::AutoLan { .. }) => true,
        _ => false,
    }
}

pub(super) struct State {
    store: Store,
    saved: Result<Snapshot, Error>,
    binding: Option<Bound>,
    closing: bool,
    progress: Waker,
}
impl State {
    pub fn new(store: Store, saved: Snapshot, progress: Waker) -> Self {
        Self {
            store,
            saved: Ok(saved),
            binding: None,
            closing: false,
            progress,
        }
    }
    fn observe(&mut self, saved: Result<Snapshot, Error>) {
        if self.saved.as_ref().ok().map(|s| s.guard) != saved.as_ref().ok().map(|s| s.guard) {
            if let Some(binding) = &mut self.binding {
                binding.applied = None;
            }
        }
        self.saved = saved;
    }
    pub fn job(&mut self, job: Job) {
        match job {
            Job::Snapshot(send) => {
                let result = self.store.read();
                self.observe(result.clone());
                reply(send, result);
            }
            Job::Replace {
                expected,
                profiles,
                reply: send,
            } => {
                let result = self.store.replace(expected, profiles);
                // Commit errors may follow durable success. Re-read exact owned
                // storage before another network action or claiming old intent.
                let observed = match &result {
                    Ok(saved) => Ok(saved.clone()),
                    Err(_) => self.store.read(),
                };
                self.observe(observed);
                reply(send, result);
            }
            Job::Bind {
                control,
                reply: send,
            } => {
                if self.binding.is_some() || self.closing {
                    reply(
                        send,
                        Err(if self.closing {
                            Error::Closed
                        } else {
                            Error::AlreadyBound
                        }),
                    );
                } else {
                    self.binding = Some(Bound::new(control));
                    reply(send, Ok(()));
                }
            }
            Job::BindingSnapshot {
                generation,
                reply: send,
            } => {
                let result = self.saved.clone().and_then(|saved| {
                    self.binding
                        .as_ref()
                        .filter(|b| b.control.generation == generation)
                        .map(|b| b.snapshot(saved.guard))
                        .ok_or(Error::BindingChanged)
                });
                reply(send, result);
            }
            Job::Retry {
                generation,
                expected,
                profile,
                reply: send,
            } => {
                let result = (|| {
                    let saved = self.saved.as_ref().map_err(Clone::clone)?;
                    if saved.guard != expected {
                        return Err(Error::Conflict(saved.clone()));
                    }
                    let binding = self
                        .binding
                        .as_mut()
                        .filter(|b| b.control.generation == generation)
                        .ok_or(Error::BindingChanged)?;
                    if binding.control.fenced.load(Ordering::Acquire) {
                        return Err(Error::Closed);
                    }
                    if !saved.profiles.iter().any(|p| p.id == profile && p.enabled) {
                        return Err(Error::NotEnabled);
                    }
                    binding.retry.insert(profile);
                    binding.applied = None;
                    Ok(())
                })();
                reply(send, result);
            }
            #[cfg(test)]
            Job::Fault(fault, send) => {
                self.store.inject(fault);
                reply(send, ());
            }
            #[cfg(test)]
            Job::Barrier(entered, release) => {
                let _ = entered.send(());
                let _ = release.recv();
            }
        }
        self.progress.wake_by_ref();
    }
    pub fn close(&mut self) {
        self.closing = true;
        if let Some(bound) = &self.binding {
            if !bound.control.closing.load(Ordering::Acquire) {
                bound.control.request_close();
            }
        }
    }
    pub fn poll(&mut self, context: &mut Context<'_>) -> OwnerProgress {
        if let Some(bound) = &mut self.binding {
            if bound.poll(self.saved.as_ref().ok(), context) {
                self.binding = None;
            }
        }
        if self.closing && self.binding.is_none() {
            OwnerProgress::Drained
        } else {
            OwnerProgress::Pending
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(bound) = &self.binding {
            bound.control.request_close();
            bound.control.failed(
                bound.control.close_attempt.load(Ordering::Acquire),
                Error::WorkerFailed,
            );
        }
    }
}
