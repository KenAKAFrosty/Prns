use super::*;
use prns_core::entropy::{EntropySource, RuntimeEntropy};

const RESEED_WINDOW_BYTES: usize = 64 * 1024;
const MAX_SOURCE_READS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BootId {
    pub node: usize,
    pub generation: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadOutcome {
    Bytes(u8),
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Read {
    pub boot: BootId,
    pub attempt: u8,
    pub outcome: ReadOutcome,
}

impl Read {
    pub fn initial(boot: BootId, seed: u8) -> Self {
        Self {
            boot,
            attempt: 1,
            outcome: ReadOutcome::Bytes(seed),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Reseed {
    NotReached,
    Succeed(u8),
    Fail,
}

#[derive(Debug)]
pub(super) struct SourceUnavailable;

// Isolated validation input, never a production source or provisioning identity.
pub(super) struct ScriptedSource {
    boot: BootId,
    seed: u8,
    reseed: Reseed,
    attempts: u8,
    calls: Rc<RefCell<Vec<Read>>>,
}

impl EntropySource for ScriptedSource {
    type Error = SourceUnavailable;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        assert_eq!(output.len(), 32);
        self.attempts += 1;
        let outcome = match self.attempts {
            1 => ReadOutcome::Bytes(self.seed),
            2 => match self.reseed {
                Reseed::NotReached => unreachable!("scenario must not reseed"),
                Reseed::Succeed(seed) => ReadOutcome::Bytes(seed),
                Reseed::Fail => ReadOutcome::Unavailable,
            },
            _ => unreachable!("bounded source script exhausted"),
        };
        let mut calls = self.calls.borrow_mut();
        assert!(calls.len() < MAX_SOURCE_READS);
        calls.push(Read {
            boot: self.boot,
            attempt: self.attempts,
            outcome,
        });
        match outcome {
            ReadOutcome::Bytes(seed) => {
                output.fill(seed);
                Ok(())
            }
            ReadOutcome::Unavailable => Err(SourceUnavailable),
        }
    }
}

pub(super) fn stream(
    boot: BootId,
    seed: u8,
    reseed: Reseed,
    calls: Rc<RefCell<Vec<Read>>>,
) -> RuntimeEntropy<ScriptedSource> {
    let before = calls.borrow().len();
    let mut entropy = RuntimeEntropy::try_new(ScriptedSource {
        boot,
        seed,
        reseed,
        attempts: 0,
        calls: Rc::clone(&calls),
    })
    .unwrap_or_else(|error| unreachable!("initial script seed: {error:?}"));
    match reseed {
        Reseed::NotReached => {}
        Reseed::Succeed(_) | Reseed::Fail => {
            // Prime the real core stream; the node's first nonempty draw triggers the reseed.
            entropy.fill_random(&mut vec![0; RESEED_WINDOW_BYTES]);
        }
    }
    assert_eq!(&calls.borrow()[before..], &[Read::initial(boot, seed)]);
    entropy
}
