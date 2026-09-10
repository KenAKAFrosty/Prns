use core::cell::{Cell, RefCell};
use core::future::poll_fn;
use core::task::Poll;

use embedded_hal::digital::{
    Error as DigitalError, ErrorKind as DigitalErrorKind, ErrorType as DigitalErrorType, OutputPin,
};
use embedded_hal::spi::{Error as SpiError, ErrorKind as SpiErrorKind};
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital::Wait;

use crate::transcript::{EventKind, Transcript};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockError {
    InvalidReadBuffer,
    TransactionTooLong,
    Transcript,
    UnsupportedOperation,
}

impl SpiError for MockError {
    fn kind(&self) -> SpiErrorKind {
        SpiErrorKind::Other
    }
}

impl DigitalError for MockError {
    fn kind(&self) -> DigitalErrorKind {
        DigitalErrorKind::Other
    }
}

#[derive(Clone, Copy)]
pub struct Trace<'a> {
    transcript: &'a RefCell<Transcript>,
}

impl<'a> Trace<'a> {
    pub const fn new(transcript: &'a RefCell<Transcript>) -> Self {
        Self { transcript }
    }

    pub fn record(self, kind: EventKind, payload: &[u8]) -> Result<(), MockError> {
        self.transcript
            .borrow_mut()
            .record(kind, payload)
            .map_err(|_| MockError::Transcript)
    }

    pub fn record_deferred(self, kind: EventKind, payload: &[u8]) {
        let _ = self.transcript.borrow_mut().record(kind, payload);
    }
}

pub struct ReadyWait<'a> {
    trace: Trace<'a>,
    identity: u8,
}

impl<'a> ReadyWait<'a> {
    pub const fn new(trace: Trace<'a>, identity: u8) -> Self {
        Self { trace, identity }
    }

    async fn ready(&mut self, operation: u8) -> Result<(), MockError> {
        self.trace
            .record(EventKind::Wait, &[self.identity, operation, 1])
    }
}

impl DigitalErrorType for ReadyWait<'_> {
    type Error = MockError;
}

impl Wait for ReadyWait<'_> {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        self.ready(1).await
    }

    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        self.ready(2).await
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        self.ready(3).await
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        self.ready(4).await
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        self.ready(5).await
    }
}

pub struct ControlledWait<'a> {
    trace: Trace<'a>,
    high_ready: &'a Cell<bool>,
}

impl<'a> ControlledWait<'a> {
    pub const fn new(trace: Trace<'a>, high_ready: &'a Cell<bool>) -> Self {
        Self { trace, high_ready }
    }

    async fn wait_high(&mut self, operation: u8) -> Result<(), MockError> {
        poll_fn(|_| {
            let ready = self.high_ready.get();
            self.trace
                .record(EventKind::Wait, &[2, operation, u8::from(ready)])?;
            if ready {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

impl DigitalErrorType for ControlledWait<'_> {
    type Error = MockError;
}

impl Wait for ControlledWait<'_> {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        self.wait_high(1).await
    }

    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        self.trace.record(EventKind::Wait, &[2, 2, 1])
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        self.wait_high(3).await
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        self.trace.record(EventKind::Wait, &[2, 4, 1])
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        self.wait_high(5).await
    }
}

pub struct MockOutput<'a> {
    trace: Trace<'a>,
}

impl<'a> MockOutput<'a> {
    pub const fn new(trace: Trace<'a>) -> Self {
        Self { trace }
    }
}

impl DigitalErrorType for MockOutput<'_> {
    type Error = MockError;
}

impl OutputPin for MockOutput<'_> {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        self.trace.record(EventKind::Reset, &[0])
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.trace.record(EventKind::Reset, &[1])
    }
}

pub struct MockDelay<'a> {
    trace: Trace<'a>,
}

impl<'a> MockDelay<'a> {
    pub const fn new(trace: Trace<'a>) -> Self {
        Self { trace }
    }
}

impl DelayNs for MockDelay<'_> {
    async fn delay_ns(&mut self, nanoseconds: u32) {
        self.trace
            .record_deferred(EventKind::Delay, &nanoseconds.to_be_bytes());
    }
}
