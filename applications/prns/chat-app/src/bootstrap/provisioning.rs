//! Initial phone/controller identity selection. Reviewed secrets remain native;
//! the platform passes only an explicit process-bound selection through its
//! startup admission. Ordinary Create always generates a fresh controller.

use super::{AppBootstrapError, Registry, Result, State, PROCESS};
use prns_core::identity::{IdentityHash, PrivateIdentityMaterial, IDENTITY_SECRET_KEY_LEN};
use prns_host_runtime::HostEmbedding;
use prns_service_runtime_uniffi::{identity::IdentitySecretKey, ServiceRuntime};
use std::{fs::File, io::Read, path::Path, sync::Arc};

#[derive(Debug, uniffi::Error)]
pub enum PhoneIdentityImportError {
    InvalidPath,
    InvalidKeyFile,
    Busy,
    StaleReview,
    EntropyUnavailable,
    Io { code: Option<i32> },
}
impl std::fmt::Display for PhoneIdentityImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "phone identity import: {self:?}")
    }
}
impl std::error::Error for PhoneIdentityImportError {}
impl From<std::io::Error> for PhoneIdentityImportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            code: error.raw_os_error(),
        }
    }
}

#[derive(uniffi::Object)]
pub struct PhoneIdentityImport {
    secret: IdentitySecretKey,
}
#[uniffi::export]
impl PhoneIdentityImport {
    pub fn identity_hash(&self) -> Vec<u8> {
        self.hash().as_bytes().to_vec()
    }
}
impl PhoneIdentityImport {
    fn hash(&self) -> IdentityHash {
        PrivateIdentityMaterial::from(self.secret.clone()).identity_hash()
    }
    fn create_controller(&self, path: &Path) -> Result<super::KeyOwner> {
        super::KeyOwner::create_imported(path, &self.secret, self.hash())
            .map_err(super::controller_error)
    }
}

/// Read an absolute, platform-selected local file containing exactly the raw
/// 64-byte Reticulum private identity. URI access/copying belongs to the native
/// picker. Applying the captured review never rereads this path.
#[uniffi::export]
pub fn app_review_phone_identity_file(
    path: String,
) -> std::result::Result<Arc<PhoneIdentityImport>, PhoneIdentityImportError> {
    let path = Path::new(&path);
    if !path.is_absolute() || !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(PhoneIdentityImportError::InvalidPath);
    }
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != IDENTITY_SECRET_KEY_LEN as u64 {
        return Err(PhoneIdentityImportError::InvalidKeyFile);
    }
    let mut secret = IdentitySecretKey::new([0; IDENTITY_SECRET_KEY_LEN]);
    file.read_exact(secret.as_mut())?;
    if file.read(&mut [0_u8; 1])? != 0 {
        return Err(PhoneIdentityImportError::InvalidKeyFile);
    }
    Ok(Arc::new(PhoneIdentityImport { secret }))
}

/// Public selection only. Its process-local ID is 64 lowercase hexadecimal
/// characters; neither a private key nor a durable restoration credential.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PhoneIdentityImportSelection {
    pub id: String,
    pub identity_hash: Vec<u8>,
}
pub(super) struct RetainedPhoneIdentityImport {
    selection: PhoneIdentityImportSelection,
    imported: Arc<PhoneIdentityImport>,
    admitted: Option<Arc<ServiceRuntime>>,
}

pub(super) enum Provisioning<'a> {
    Create,
    Import(&'a PhoneIdentityImport),
    RetainedImport {
        selection: PhoneIdentityImportSelection,
        imported: Arc<PhoneIdentityImport>,
    },
    Open,
}
impl Provisioning<'_> {
    pub(super) fn creates(&self) -> bool {
        !matches!(self, Self::Open)
    }
    pub(super) fn imports(&self) -> bool {
        matches!(self, Self::Import(_) | Self::RetainedImport { .. })
    }
    pub(super) fn validate_import(&self, state: &State) -> Result<()> {
        if let Self::RetainedImport {
            selection,
            imported,
        } = self
        {
            if !state.pending_import.as_ref().is_some_and(|pending| {
                pending.selection == *selection && Arc::ptr_eq(&pending.imported, imported)
            }) {
                return Err(AppBootstrapError::StaleImportReview);
            }
        }
        Ok(())
    }
    pub(super) fn finish_import(
        &self,
        state: &mut State,
        owner: Arc<ServiceRuntime>,
    ) -> Option<RetainedPhoneIdentityImport> {
        if matches!(self, Self::RetainedImport { .. }) {
            // The construction claim retains the validated slot. Local opening
            // still runs asynchronously; only confirmed readiness releases it.
            if let Some(pending) = &mut state.pending_import {
                pending.admitted = Some(owner);
            }
            None
        } else if self.creates() {
            state.pending_import.take()
        } else {
            None
        }
    }
    pub(super) fn create_controller(&self, path: &Path) -> Result<super::KeyOwner> {
        match self {
            Self::Create => super::KeyOwner::create_new(path).map_err(super::controller_error),
            Self::Import(imported) => imported.create_controller(path),
            Self::RetainedImport { imported, .. } => imported.create_controller(path),
            Self::Open => Err(AppBootstrapError::InvalidInstallation),
        }
    }
}

impl Registry {
    pub fn retain_phone_identity_import(
        &self,
        imported: Arc<PhoneIdentityImport>,
    ) -> std::result::Result<PhoneIdentityImportSelection, PhoneIdentityImportError> {
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| PhoneIdentityImportError::EntropyUnavailable)?;
        let id = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let selection = PhoneIdentityImportSelection {
            id,
            identity_hash: imported.identity_hash(),
        };
        let retired = {
            let mut state = self.lock();
            if state.construction.is_some()
                || state
                    .pending_import
                    .as_ref()
                    .is_some_and(|pending| pending.admitted.is_some())
            {
                return Err(PhoneIdentityImportError::Busy);
            }
            state.pending_import.replace(RetainedPhoneIdentityImport {
                selection: selection.clone(),
                imported,
                admitted: None,
            })
        };
        drop(retired);
        Ok(selection)
    }
    pub fn pending_phone_identity_import(&self) -> Option<PhoneIdentityImportSelection> {
        self.lock()
            .pending_import
            .as_ref()
            .map(|pending| pending.selection.clone())
    }
    /// Cancel an unaccepted review, or release it after its exact installation
    /// becomes locally Ready. This never cancels an admitted installation. A
    /// failed/unfinished local open keeps its review until explicit reset.
    pub fn cancel_phone_identity_import(
        &self,
        id: &str,
    ) -> std::result::Result<(), PhoneIdentityImportError> {
        let retired = {
            let mut state = self.lock();
            if state.construction.is_some() {
                return Err(PhoneIdentityImportError::Busy);
            }
            let pending = state
                .pending_import
                .as_ref()
                .filter(|pending| pending.selection.id == id)
                .ok_or(PhoneIdentityImportError::StaleReview)?;
            if let Some(owner) = &pending.admitted {
                if !state
                    .installation
                    .as_ref()
                    .is_some_and(|entry| Arc::ptr_eq(&entry.owner, owner))
                {
                    return Err(PhoneIdentityImportError::StaleReview);
                }
                if owner.status().phase != super::Phase::Ready {
                    return Err(PhoneIdentityImportError::Busy);
                }
            }
            state.pending_import.take()
        };
        drop(retired);
        Ok(())
    }
    fn retained_import(&self, id: &str) -> Result<Provisioning<'static>> {
        let state = self.lock();
        let pending = state
            .pending_import
            .as_ref()
            .filter(|pending| pending.selection.id == id)
            .ok_or(AppBootstrapError::StaleImportReview)?;
        Ok(Provisioning::RetainedImport {
            selection: pending.selection.clone(),
            imported: pending.imported.clone(),
        })
    }
    /// Native-only convenience for embeddings which already retain the review.
    pub fn start_imported(
        &self,
        root: impl AsRef<Path>,
        imported: &PhoneIdentityImport,
        embedding: HostEmbedding,
    ) -> Result<Arc<ServiceRuntime>> {
        self.start(
            root.as_ref(),
            Provisioning::Import(imported),
            Some(embedding),
        )
    }
    pub fn prepare_imported(
        &self,
        root: impl AsRef<Path>,
        imported: &PhoneIdentityImport,
    ) -> Result<Arc<ServiceRuntime>> {
        self.start(root.as_ref(), Provisioning::Import(imported), None)
    }
    pub fn prepare_retained_phone_identity_import(
        &self,
        root: impl AsRef<Path>,
        id: &str,
    ) -> Result<Arc<ServiceRuntime>> {
        self.start(root.as_ref(), self.retained_import(id)?, None)
    }
    pub fn start_retained_phone_identity_import(
        &self,
        root: impl AsRef<Path>,
        id: &str,
        embedding: HostEmbedding,
    ) -> Result<Arc<ServiceRuntime>> {
        self.start(root.as_ref(), self.retained_import(id)?, Some(embedding))
    }
}

#[uniffi::export]
pub fn app_retain_phone_identity_import(
    imported: Arc<PhoneIdentityImport>,
) -> std::result::Result<PhoneIdentityImportSelection, PhoneIdentityImportError> {
    PROCESS.retain_phone_identity_import(imported)
}
#[uniffi::export]
pub fn app_pending_phone_identity_import() -> Option<PhoneIdentityImportSelection> {
    PROCESS.pending_phone_identity_import()
}
#[uniffi::export]
pub fn app_cancel_phone_identity_import(
    id: String,
) -> std::result::Result<(), PhoneIdentityImportError> {
    PROCESS.cancel_phone_identity_import(&id)
}

/// The platform calls this before network permission/admission. It constructs
/// only local state; networking is subsequently admitted through ordinary Open.
#[uniffi::export]
pub fn app_prepare_retained_phone_identity_import(
    storage_root: String,
    id: String,
) -> std::result::Result<Arc<ServiceRuntime>, AppBootstrapError> {
    PROCESS.prepare_retained_phone_identity_import(storage_root, &id)
}
/// Platform construction hook for an already admitted network start. Product JS
/// must use its path-free platform bridge, just as for ordinary Create/Open.
#[uniffi::export]
pub fn app_start_retained_phone_identity_import(
    storage_root: String,
    id: String,
) -> std::result::Result<Arc<ServiceRuntime>, AppBootstrapError> {
    PROCESS.start_retained_phone_identity_import(storage_root, &id, super::platform_embedding())
}

#[cfg(test)]
#[path = "provisioning_tests.rs"]
mod tests;
