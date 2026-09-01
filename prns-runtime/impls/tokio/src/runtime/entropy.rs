use prns_core::entropy::{EntropySource, ReseedHealth, RuntimeEntropy};
use prns_core::remote_control::{
    RemoteControlNodeIdentitySecrets, RemoteControlNodeIdentitySecretsError,
};

pub(crate) struct OsEntropySource;

impl EntropySource for OsEntropySource {
    type Error = OsEntropyError;

    fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::getrandom(output).map_err(OsEntropyError)
    }
}

pub struct OsRuntimeEntropy {
    inner: RuntimeEntropy<OsEntropySource>,
}

impl OsRuntimeEntropy {
    pub fn try_new() -> Result<Self, OsEntropyError> {
        RuntimeEntropy::try_new(OsEntropySource).map(|inner| Self { inner })
    }

    pub fn fill_random(&mut self, output: &mut [u8]) {
        self.inner.fill_random(output);
    }

    #[must_use]
    pub fn reseed_health(&self) -> ReseedHealth {
        self.inner.reseed_health()
    }

    pub fn generate_remote_control_identity_secrets(
        &mut self,
    ) -> Result<RemoteControlNodeIdentitySecrets, RemoteControlNodeIdentitySecretsError> {
        RemoteControlNodeIdentitySecrets::generate_with_runtime_entropy(&mut self.inner)
    }

    pub(crate) fn inner_mut(&mut self) -> &mut RuntimeEntropy<OsEntropySource> {
        &mut self.inner
    }
}

#[derive(Debug)]
pub struct OsEntropyError(getrandom::Error);

impl core::fmt::Display for OsEntropyError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "OS CSPRNG failed: {}", self.0)
    }
}

impl std::error::Error for OsEntropyError {}
