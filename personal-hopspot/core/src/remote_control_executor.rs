use core::future::Future;
use core::pin::{pin, Pin};
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::{Mutex, MutexGuard};
use embassy_sync::signal::Signal;
use personal_rns::remote_control::{RemoteControlWifiCredentialRevision, RemoteControlWifiStation};
use personal_rns::runtime::{
    RemoteControlHostCommand, RemoteControlHostCommandError, RemoteControlHostControls,
    RemoteControlHostResponse,
};

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
}

impl HopspotWifiCredentialMailbox {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            update: Signal::new(),
        }
    }

    pub fn replace(&self, update: HopspotWifiCredentialUpdate) {
        self.update
            .signal(HopspotWifiCredentialCommand::Replace(update));
    }

    pub fn clear(&self) {
        self.update.signal(HopspotWifiCredentialCommand::Clear);
    }

    pub async fn receive(&self) -> HopspotWifiCredentialCommand {
        self.update.wait().await
    }

    pub fn try_receive(&self) -> Option<HopspotWifiCredentialCommand> {
        self.update.try_take()
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

pub struct HopspotCommandMailbox<const DEPTH: usize> {
    commands: Channel<CriticalSectionRawMutex, PendingHopspotCommand, DEPTH>,
    completion: Signal<CriticalSectionRawMutex, HopspotCommandCompletion>,
    live_caller: Mutex<CriticalSectionRawMutex, ()>,
    next_sequence: AtomicU32,
}

impl<const DEPTH: usize> HopspotCommandMailbox<DEPTH> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            commands: Channel::new(),
            completion: Signal::new(),
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
        self.commands.receive().await
    }

    pub fn complete(
        &'static self,
        token: HopspotCommandToken,
        result: Result<RemoteControlHostResponse, RemoteControlHostCommandError>,
    ) {
        self.completion.signal(HopspotCommandCompletion {
            sequence: token.0,
            result,
        });
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
        let _ = mailbox.completion.try_take();
        let sequence = mailbox.next_sequence.fetch_add(1, Ordering::Relaxed);
        let immediate_failure = mailbox
            .commands
            .try_send(PendingHopspotCommand { sequence, command })
            .err()
            .map(|_| RemoteControlHostCommandError::Busy);
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
            let mut completion = pin!(self.mailbox.completion.wait());
            match completion.as_mut().poll(context) {
                Poll::Ready(completion) if completion.sequence == self.sequence => {
                    self.caller = None;
                    self.completed = true;
                    return Poll::Ready(completion.result);
                }
                Poll::Ready(_) => {}
                Poll::Pending => return Poll::Pending,
            }
        }
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
}
