use std::sync::{Arc, Mutex};

use prns_core::entropy::{EntropySource, RuntimeEntropy};

use super::OsEntropySource;

pub(crate) type TokioEntropy = TokioHandleEntropy;

/// One node's shared handle/interface stream and independent fallible path-ID source.
/// Clones share ownership; they never copy generator or provider state.
#[derive(Clone)]
pub struct TokioHandleEntropy {
    owner: Arc<dyn HandleEntropyOwner>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PathEntropyError {
    SourceUnavailable,
    OwnerPoisoned,
}

trait HandleEntropyOwner: Send + Sync {
    fn fill(&self, output: &mut [u8]);
    fn try_fill_path_id(&self, output: &mut [u8]) -> Result<(), PathEntropyError>;
}

struct EntropyState<S, P> {
    stream: RuntimeEntropy<S>,
    path_source: P,
}

impl<S: EntropySource + Send, P: EntropySource + Send> HandleEntropyOwner
    for Mutex<EntropyState<S, P>>
{
    #[expect(
        clippy::expect_used,
        reason = "a poisoned entropy owner must not emit runtime randomness"
    )]
    fn fill(&self, output: &mut [u8]) {
        self.lock()
            .expect("runtime entropy owner must not be poisoned")
            .stream
            .fill_random(output);
    }

    fn try_fill_path_id(&self, output: &mut [u8]) -> Result<(), PathEntropyError> {
        self.lock()
            .map_err(|_| PathEntropyError::OwnerPoisoned)?
            .path_source
            .try_fill_entropy(output)
            .map_err(|_| PathEntropyError::SourceUnavailable)
    }
}

impl TokioHandleEntropy {
    /// Initializes the shared stream from the OS and retains OS reads for path IDs.
    pub fn try_os() -> Result<Self, super::OsEntropyError> {
        RuntimeEntropy::try_new(OsEntropySource)
            .map(|stream| Self::from_sources(stream, OsEntropySource))
    }

    /// Consumes a branded runtime stream and a separate fallible path-ID provider.
    /// Production providers must satisfy the core cryptographic entropy contract;
    /// scripted providers are appropriate only in isolated validation.
    #[must_use]
    pub fn from_sources<S, P>(stream: RuntimeEntropy<S>, path_source: P) -> Self
    where
        S: EntropySource + Send + 'static,
        P: EntropySource + Send + 'static,
    {
        Self {
            owner: Arc::new(Mutex::new(EntropyState {
                stream,
                path_source,
            })),
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "runtime entropy requires a functioning OS seed source"
    )]
    pub(crate) fn new() -> Self {
        Self::try_os().expect("OS CSPRNG must provide the initial runtime seed")
    }

    pub(crate) fn fill(&self, output: &mut [u8]) {
        if !output.is_empty() {
            self.owner.fill(output);
        }
    }

    pub(crate) fn try_fill_path_id(&self, output: &mut [u8]) -> Result<(), PathEntropyError> {
        self.owner.try_fill_path_id(output)
    }

    #[cfg(test)]
    pub(crate) fn from_test_seed(byte: u8) -> Self {
        let stream = RuntimeEntropy::try_new(move |output: &mut [u8]| {
            output.fill(byte);
            Ok::<(), core::convert::Infallible>(())
        })
        .unwrap_or_else(|never| match never {});
        Self::from_sources(stream, OsEntropySource)
    }
}

#[cfg(test)]
mod tests;
