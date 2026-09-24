use core::cell::RefCell;
use core::future::Future;
use core::pin::{pin, Pin};
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::mutex::{Mutex, MutexGuard};
use embassy_sync::signal::Signal;
use personal_rns::interfaces::{DiscoveryGroupSet, InterfaceId};
use personal_rns::remote_control::{RemoteControlWifiCredentialRevision, RemoteControlWifiStation};
use personal_rns::runtime::{
    restored_discovery_groups_now, store_discovery_group_configuration,
    DiscoveryGroupConfigurationChange, RemoteControlHostCommand, RemoteControlHostCommandError,
    RemoteControlHostControls, RemoteControlHostResponse,
};

pub struct PreparedDiscoveryGroupReplacement {
    interface_id: InterfaceId,
    previous: DiscoveryGroupSet,
}

impl PreparedDiscoveryGroupReplacement {
    #[must_use]
    pub fn runtime_rollback_groups(&self) -> DiscoveryGroupSet {
        self.previous
    }
}

pub async fn persist_discovery_group_replacement(
    interface_id: InterfaceId,
    groups: &DiscoveryGroupSet,
) -> Result<PreparedDiscoveryGroupReplacement, RemoteControlHostCommandError> {
    let previous = restored_discovery_groups_now(interface_id)
        .ok_or(RemoteControlHostCommandError::PersistenceFailed)?
        .unwrap_or_else(DiscoveryGroupSet::reticulum);
    let prepared = PreparedDiscoveryGroupReplacement {
        interface_id,
        previous,
    };
    store_discovery_group_configuration(DiscoveryGroupConfigurationChange::upsert(
        interface_id,
        *groups,
    ))
    .await
    .map_err(|_| RemoteControlHostCommandError::PersistenceFailed)?;
    Ok(prepared)
}

pub async fn rollback_discovery_group_replacement(
    prepared: &PreparedDiscoveryGroupReplacement,
) -> Result<(), RemoteControlHostCommandError> {
    let change =
        DiscoveryGroupConfigurationChange::upsert(prepared.interface_id, prepared.previous);
    store_discovery_group_configuration(change)
        .await
        .map_err(|_| RemoteControlHostCommandError::RollbackFailed)
}

pub struct HopspotWifiCredentialUpdate {
    pub revision: Option<RemoteControlWifiCredentialRevision>,
    pub station: RemoteControlWifiStation,
}

pub enum HopspotWifiCredentialCommand {
    Replace(HopspotWifiCredentialUpdate),
    Clear,
}

pub struct HopspotWifiCredentialMailbox {
    update: Signal<CriticalSectionRawMutex, HopspotWifiCredentialCommand>,
    pending: Signal<CriticalSectionRawMutex, ()>,
}

impl HopspotWifiCredentialMailbox {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            update: Signal::new(),
            pending: Signal::new(),
        }
    }

    pub fn replace(&self, update: HopspotWifiCredentialUpdate) {
        self.update
            .signal(HopspotWifiCredentialCommand::Replace(update));
        self.pending.signal(());
    }

    pub fn clear(&self) {
        self.update.signal(HopspotWifiCredentialCommand::Clear);
        self.pending.signal(());
    }

    pub async fn receive(&self) -> HopspotWifiCredentialCommand {
        self.update.wait().await
    }

    pub fn try_receive(&self) -> Option<HopspotWifiCredentialCommand> {
        self.update.try_take()
    }

    /// Wake the single station consumer without taking its pending credentials.
    ///
    /// Cancellation leaves the latest Replace/Clear command queued. A separate notification
    /// avoids consuming and requeueing a command over a newer update. Notifications left by an
    /// earlier receive are drained here, so they cannot repeatedly bypass station backoff.
    pub async fn wait_until_pending(&self) {
        loop {
            if self.update.signaled() {
                return;
            }
            self.pending.wait().await;
        }
    }
}

impl Default for HopspotWifiCredentialMailbox {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PendingHopspotCommand {
    sequence: u32,
    command: RemoteControlHostCommand,
}

pub struct HopspotCommandToken(u32);

impl PendingHopspotCommand {
    #[must_use]
    pub fn into_parts(self) -> (HopspotCommandToken, RemoteControlHostCommand) {
        (HopspotCommandToken(self.sequence), self.command)
    }
}

struct HopspotCommandCompletion {
    sequence: u32,
    result: Result<RemoteControlHostResponse, RemoteControlHostCommandError>,
}

enum HopspotCommandExchange {
    Empty,
    Queued {
        pending: PendingHopspotCommand,
        abandoned: bool,
    },
    Executing {
        sequence: u32,
        abandoned: bool,
    },
    Completed(HopspotCommandCompletion),
}

pub struct HopspotCommandMailbox<const DEPTH: usize> {
    exchange: BlockingMutex<CriticalSectionRawMutex, RefCell<HopspotCommandExchange>>,
    command_ready: Signal<CriticalSectionRawMutex, ()>,
    completion_ready: Signal<CriticalSectionRawMutex, ()>,
    live_caller: Mutex<CriticalSectionRawMutex, ()>,
    next_sequence: AtomicU32,
}

impl<const DEPTH: usize> HopspotCommandMailbox<DEPTH> {
    #[must_use]
    pub const fn new() -> Self {
        assert!(
            DEPTH == 1,
            "the Hopspot command executor has exactly one bounded slot"
        );
        Self {
            exchange: BlockingMutex::new(RefCell::new(HopspotCommandExchange::Empty)),
            command_ready: Signal::new(),
            completion_ready: Signal::new(),
            live_caller: Mutex::new(()),
            next_sequence: AtomicU32::new(1),
        }
    }

    #[must_use]
    pub const fn handle(&'static self) -> HopspotCommandHandle<DEPTH> {
        HopspotCommandHandle { mailbox: self }
    }

    fn execute(&'static self, command: RemoteControlHostCommand) -> HopspotCommandFuture<DEPTH> {
        HopspotCommandFuture::new(self, command)
    }

    pub async fn receive(&'static self) -> PendingHopspotCommand {
        loop {
            let pending = self.exchange.lock(|exchange| {
                let mut exchange = exchange.borrow_mut();
                match core::mem::replace(&mut *exchange, HopspotCommandExchange::Empty) {
                    HopspotCommandExchange::Queued { pending, abandoned } => {
                        *exchange = HopspotCommandExchange::Executing {
                            sequence: pending.sequence,
                            abandoned,
                        };
                        Some(pending)
                    }
                    current => {
                        *exchange = current;
                        None
                    }
                }
            });
            if let Some(pending) = pending {
                return pending;
            }
            self.command_ready.wait().await;
        }
    }

    pub fn complete(
        &'static self,
        token: HopspotCommandToken,
        result: Result<RemoteControlHostResponse, RemoteControlHostCommandError>,
    ) {
        let completed = self.exchange.lock(|exchange| {
            let mut exchange = exchange.borrow_mut();
            match &*exchange {
                HopspotCommandExchange::Executing {
                    sequence,
                    abandoned,
                } if *sequence == token.0 => {
                    if *abandoned {
                        *exchange = HopspotCommandExchange::Empty;
                        false
                    } else {
                        *exchange = HopspotCommandExchange::Completed(HopspotCommandCompletion {
                            sequence: token.0,
                            result,
                        });
                        true
                    }
                }
                _ => false,
            }
        });
        if completed {
            self.completion_ready.signal(());
        }
    }
}

impl<const DEPTH: usize> Default for HopspotCommandMailbox<DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
pub struct HopspotCommandHandle<const DEPTH: usize> {
    mailbox: &'static HopspotCommandMailbox<DEPTH>,
}

impl<const DEPTH: usize> HopspotCommandHandle<DEPTH> {
    pub fn execute(
        self,
        command: RemoteControlHostCommand,
    ) -> impl Future<Output = Result<RemoteControlHostResponse, RemoteControlHostCommandError>>
    {
        self.mailbox.execute(command)
    }
}

struct HopspotCommandFuture<const DEPTH: usize> {
    mailbox: &'static HopspotCommandMailbox<DEPTH>,
    caller: Option<MutexGuard<'static, CriticalSectionRawMutex, ()>>,
    sequence: u32,
    immediate_failure: Option<RemoteControlHostCommandError>,
    completed: bool,
}

impl<const DEPTH: usize> HopspotCommandFuture<DEPTH> {
    fn new(
        mailbox: &'static HopspotCommandMailbox<DEPTH>,
        command: RemoteControlHostCommand,
    ) -> Self {
        let Ok(caller) = mailbox.live_caller.try_lock() else {
            return Self {
                mailbox,
                caller: None,
                sequence: 0,
                immediate_failure: Some(RemoteControlHostCommandError::Busy),
                completed: false,
            };
        };
        let sequence = mailbox.next_sequence.fetch_add(1, Ordering::Relaxed);
        let queued = mailbox.exchange.lock(|exchange| {
            let mut exchange = exchange.borrow_mut();
            if matches!(*exchange, HopspotCommandExchange::Empty) {
                *exchange = HopspotCommandExchange::Queued {
                    pending: PendingHopspotCommand { sequence, command },
                    abandoned: false,
                };
                true
            } else {
                false
            }
        });
        if queued {
            mailbox.command_ready.signal(());
        }
        let immediate_failure = (!queued).then_some(RemoteControlHostCommandError::Busy);
        Self {
            mailbox,
            caller: immediate_failure.is_none().then_some(caller),
            sequence,
            immediate_failure,
            completed: false,
        }
    }
}

impl<const DEPTH: usize> Future for HopspotCommandFuture<DEPTH> {
    type Output = Result<RemoteControlHostResponse, RemoteControlHostCommandError>;

    #[inline(never)]
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(error) = self.immediate_failure.take() {
            self.completed = true;
            return Poll::Ready(Err(error));
        }
        if self.completed {
            return Poll::Pending;
        }
        loop {
            let completion = self.mailbox.exchange.lock(|exchange| {
                let mut exchange = exchange.borrow_mut();
                let matches = matches!(
                    &*exchange,
                    HopspotCommandExchange::Completed(completion)
                        if completion.sequence == self.sequence
                );
                if !matches {
                    return None;
                }
                match core::mem::replace(&mut *exchange, HopspotCommandExchange::Empty) {
                    HopspotCommandExchange::Completed(completion) => Some(completion.result),
                    _ => None,
                }
            });
            if let Some(result) = completion {
                self.caller = None;
                self.completed = true;
                return Poll::Ready(result);
            }
            let mut ready = pin!(self.mailbox.completion_ready.wait());
            match ready.as_mut().poll(context) {
                Poll::Ready(()) => {}
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<const DEPTH: usize> Drop for HopspotCommandFuture<DEPTH> {
    fn drop(&mut self) {
        if self.completed || self.sequence == 0 {
            return;
        }
        self.mailbox.exchange.lock(|exchange| {
            let mut exchange = exchange.borrow_mut();
            match &mut *exchange {
                HopspotCommandExchange::Queued { pending, abandoned }
                    if pending.sequence == self.sequence =>
                {
                    *abandoned = true
                }
                HopspotCommandExchange::Executing {
                    sequence,
                    abandoned,
                } if *sequence == self.sequence => *abandoned = true,
                HopspotCommandExchange::Completed(completion)
                    if completion.sequence == self.sequence =>
                {
                    *exchange = HopspotCommandExchange::Empty;
                }
                _ => {}
            }
        });
    }
}

impl<const DEPTH: usize> RemoteControlHostControls for HopspotCommandHandle<DEPTH> {
    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        self.execute(command).await
    }
}

/// Owns the mutable hardware state behind the shared Hopspot command queue.
#[allow(async_fn_in_trait)]
pub trait HopspotCommandExecutor {
    async fn execute(
        &mut self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError>;
}

pub async fn run_hopspot_command_executor<const DEPTH: usize>(
    mailbox: &'static HopspotCommandMailbox<DEPTH>,
    executor: &mut impl HopspotCommandExecutor,
) -> ! {
    loop {
        let queued = mailbox.receive().await;
        let (token, command) = queued.into_parts();
        let result = executor.execute(command).await;
        mailbox.complete(token, result);
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};
    use std::boxed::Box;

    use personal_rns::remote_control::RemoteControlBuildVersion;

    use super::*;

    fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        future.poll(&mut context)
    }

    fn wifi_update(ssid: &str, revision: u32) -> HopspotWifiCredentialUpdate {
        HopspotWifiCredentialUpdate {
            revision: Some(
                RemoteControlWifiCredentialRevision::new(revision).expect("nonzero revision"),
            ),
            station: RemoteControlWifiStation::parse(ssid, "test-password")
                .expect("valid credentials"),
        }
    }

    #[test]
    fn wifi_pending_wait_wakes_without_consuming_credentials() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::task::Wake;

        #[derive(Default)]
        struct WakeCount(AtomicUsize);
        impl Wake for WakeCount {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let mailbox = HopspotWifiCredentialMailbox::new();
        let wakes = Arc::new(WakeCount::default());
        let waker = Waker::from(Arc::clone(&wakes));
        let mut context = Context::from_waker(&waker);
        let mut waiting = pin!(mailbox.wait_until_pending());
        assert!(waiting.as_mut().poll(&mut context).is_pending());
        mailbox.replace(wifi_update("restored-network", 2));
        assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
        assert!(waiting.as_mut().poll(&mut context).is_ready());
        let Some(HopspotWifiCredentialCommand::Replace(update)) = mailbox.try_receive() else {
            panic!("waking must leave the credentials queued");
        };
        assert_eq!(update.revision.map(|revision| revision.get()), Some(2));
        assert_eq!(update.station.ssid(), "restored-network");
        assert_eq!(update.station.password(), "test-password");
        assert!(mailbox.try_receive().is_none());
    }

    #[test]
    fn wifi_pending_wait_preserves_latest_replacement_and_clear() {
        let mailbox = HopspotWifiCredentialMailbox::new();
        mailbox.replace(wifi_update("old-network", 1));
        let mut waiting = pin!(mailbox.wait_until_pending());
        assert!(poll(waiting.as_mut()).is_ready());
        mailbox.replace(wifi_update("new-network", 2));
        let Some(HopspotWifiCredentialCommand::Replace(update)) = mailbox.try_receive() else {
            panic!("latest credentials remain queued after a wake");
        };
        assert_eq!(update.station.ssid(), "new-network");
        assert_eq!(update.revision.map(|revision| revision.get()), Some(2));

        let mut waiting = pin!(mailbox.wait_until_pending());
        assert!(poll(waiting.as_mut()).is_pending());
        mailbox.replace(wifi_update("discarded-network", 3));
        mailbox.clear();
        assert!(poll(waiting.as_mut()).is_ready());
        assert!(matches!(
            mailbox.try_receive(),
            Some(HopspotWifiCredentialCommand::Clear)
        ));
        assert!(mailbox.try_receive().is_none());
    }

    #[test]
    fn wifi_pending_wait_is_cancel_safe_and_ignores_stale_notifications() {
        let mailbox = HopspotWifiCredentialMailbox::new();
        mailbox.replace(wifi_update("consumed-network", 1));
        assert!(mailbox.try_receive().is_some());
        {
            let mut waiting = pin!(mailbox.wait_until_pending());
            assert!(
                poll(waiting.as_mut()).is_pending(),
                "an old notification is not a new command"
            );
        }
        mailbox.clear();
        {
            let mut waiting = pin!(mailbox.wait_until_pending());
            assert!(
                poll(waiting.as_mut()).is_ready(),
                "a command queued before waiting is immediately visible"
            );
        }
        let mut receive = pin!(mailbox.receive());
        assert!(matches!(
            poll(receive.as_mut()),
            Poll::Ready(HopspotWifiCredentialCommand::Clear)
        ));
        let mut waiting = pin!(mailbox.wait_until_pending());
        assert!(
            poll(waiting.as_mut()).is_pending(),
            "both receive paths leave no spurious retry wake"
        );
    }

    #[test]
    fn one_live_caller_gets_exact_completion_and_a_competitor_gets_busy() {
        let mailbox = Box::leak(Box::new(HopspotCommandMailbox::<1>::new()));
        let handle = mailbox.handle();
        let mut first = pin!(handle.execute(RemoteControlHostCommand::DescribeBuild));
        assert!(poll(first.as_mut()).is_pending());

        let mut competing = pin!(handle.execute(RemoteControlHostCommand::DescribePower));
        assert_eq!(
            poll(competing.as_mut()),
            Poll::Ready(Err(RemoteControlHostCommandError::Busy))
        );

        let mut receive = pin!(mailbox.receive());
        let Poll::Ready(pending) = poll(receive.as_mut()) else {
            panic!("the first command was enqueued");
        };
        let (token, command) = pending.into_parts();
        assert!(matches!(command, RemoteControlHostCommand::DescribeBuild));
        let expected = RemoteControlHostResponse::DescribeBuild(RemoteControlBuildVersion::empty());
        mailbox.complete(token, Ok(expected.clone()));
        assert_eq!(poll(first.as_mut()), Poll::Ready(Ok(expected)));
    }

    #[test]
    fn an_unreceived_depth_one_command_holds_bounded_backpressure() {
        let mailbox = Box::leak(Box::new(HopspotCommandMailbox::<1>::new()));
        let handle = mailbox.handle();
        let mut first = pin!(handle.execute(RemoteControlHostCommand::DescribeBuild));
        assert!(poll(first.as_mut()).is_pending());

        drop(first);
        let mut second = pin!(handle.execute(RemoteControlHostCommand::DescribePower));
        assert_eq!(
            poll(second.as_mut()),
            Poll::Ready(Err(RemoteControlHostCommandError::Busy))
        );
    }

    #[test]
    fn an_abandoned_command_releases_the_slot_after_exact_executor_completion() {
        let mailbox = Box::leak(Box::new(HopspotCommandMailbox::<1>::new()));
        let handle = mailbox.handle();
        let mut abandoned = Box::pin(handle.execute(RemoteControlHostCommand::DescribeBuild));
        assert!(poll(abandoned.as_mut()).is_pending());
        drop(abandoned);

        let mut blocked = pin!(handle.execute(RemoteControlHostCommand::DescribePower));
        assert_eq!(
            poll(blocked.as_mut()),
            Poll::Ready(Err(RemoteControlHostCommandError::Busy))
        );

        let mut receive = pin!(mailbox.receive());
        let Poll::Ready(pending) = poll(receive.as_mut()) else {
            panic!("the abandoned command remains executor-owned");
        };
        let (token, command) = pending.into_parts();
        assert!(matches!(command, RemoteControlHostCommand::DescribeBuild));
        mailbox.complete(
            token,
            Ok(RemoteControlHostResponse::DescribeBuild(
                RemoteControlBuildVersion::empty(),
            )),
        );

        let mut recovered = pin!(handle.execute(RemoteControlHostCommand::DescribePower));
        assert!(poll(recovered.as_mut()).is_pending());
    }
}
