use crate::{
    files, reference_name, unwrap, wrap, Error, IdentitySecretKey, IdentityStore, OsEntropy, Result,
};
use prns_core::crypto::sha256;
use prns_core::entropy::RuntimeEntropy;
use prns_core::identity::{PrivateIdentityMaterial, Zeroizing};
use prns_identities_core::{
    Action, Command, Coordinator, EffectTicket, KeyReference, KeySource, OperationResult,
    OperationState, OperationToken, PublicIdentityHash, VerifiedKey,
};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

const KEY_MAGIC: &[u8; 8] = b"PNIKEY01";
const KEY_BYTES: usize = 8 + 32 + 16 + 64 + 32;
const KEEP_MAGIC: &[u8; 8] = b"PNIKEEP1";

impl<const I: usize, const O: usize, const S: usize> IdentityStore<I, O, S> {
    /// Complete the current publication action. A complete staged key is reused
    /// after uncertain writes; an incomplete/corrupt stage fails closed. Only a
    /// genuinely absent key for a current Generate action draws fresh entropy.
    pub fn publish_key(
        &mut self,
        authority: &Coordinator<'_, I, O, S>,
        ticket: EffectTicket,
        import: Option<&IdentitySecretKey>,
    ) -> Result<VerifiedKey> {
        self.validate_effect(authority, ticket)?;
        let token = ticket.operation.ok_or(Error::WrongAction)?;
        let (reference, source) = match authority.action(token).map_err(Error::Authority)? {
            Some(Action::PublishKey {
                ticket: current,
                key_reference,
                source,
            }) if current == ticket => (key_reference, source),
            _ => return Err(Error::WrongAction),
        };
        let path = self
            .root
            .join("keys")
            .join(format!("{}.key", reference_name(reference)));
        let secret = publish_key(&path, reference, source, import)?;
        Ok(VerifiedKey {
            key_reference: reference,
            public_hash: hash(&secret),
        })
    }

    /// Read existing key material only for the current activation/restoration.
    /// Missing keys, even with a leftover stage, never trigger generation.
    pub fn activate_key(
        &self,
        authority: &Coordinator<'_, I, O, S>,
        ticket: EffectTicket,
    ) -> Result<IdentitySecretKey> {
        self.validate_effect(authority, ticket)?;
        let action = match ticket.operation {
            Some(token) => authority.action(token),
            None => authority.recovery_action(ticket.identity),
        }
        .map_err(Error::Authority)?;
        let (reference, expected) = match action {
            Some(Action::Activate {
                ticket: current,
                identity,
                key_reference,
                ..
            }) if current == ticket => (key_reference, identity.public_hash),
            _ => return Err(Error::WrongAction),
        };
        let path = self
            .root
            .join("keys")
            .join(format!("{}.key", reference_name(reference)));
        read_key(&path, reference, Some(expected))?.ok_or(Error::MissingKey)
    }

    /// Load the existing key only for the current explicit service-initialization
    /// action. Ordinary history/activation never gains initialization authority.
    pub fn initialization_key(
        &self,
        authority: &Coordinator<'_, I, O, S>,
        ticket: EffectTicket,
    ) -> Result<IdentitySecretKey> {
        self.validate_effect(authority, ticket)?;
        let token = ticket.operation.ok_or(Error::WrongAction)?;
        let Some(Action::InitializeService {
            ticket: current,
            identity,
            key_reference,
            ..
        }) = authority.action(token).map_err(Error::Authority)?
        else {
            return Err(Error::WrongAction);
        };
        if current != ticket {
            return Err(Error::WrongAction);
        }
        let path = self
            .root
            .join("keys")
            .join(format!("{}.key", reference_name(key_reference)));
        read_key(&path, key_reference, Some(identity.public_hash))?.ok_or(Error::MissingKey)
    }

    /// Open the pinned key for local history/contact signing without registering
    /// destinations. Network and publication remain separately admitted effects.
    pub fn local_key(
        &self,
        authority: &Coordinator<'_, I, O, S>,
        guard: prns_identities_core::AdmissionGuard,
    ) -> Result<IdentitySecretKey> {
        self.validate_owner(authority)?;
        authority
            .validate_admission(guard)
            .map_err(Error::Authority)?;
        if !matches!(
            guard.class,
            prns_identities_core::AdmissionClass::History
                | prns_identities_core::AdmissionClass::OfflineSigning
        ) {
            return Err(Error::WrongAction);
        }
        let record = authority
            .identity(guard.identity.id)
            .map_err(Error::Authority)?;
        let path = self
            .root
            .join("keys")
            .join(format!("{}.key", reference_name(record.key_reference)));
        read_key(&path, record.key_reference, Some(record.public_hash))?.ok_or(Error::MissingKey)
    }

    /// Durably index an extra imported key before acknowledging core result
    /// retirement. No key is erased. Retry is safe before/after the core ack.
    pub fn account_retained_key(
        &mut self,
        authority: &Coordinator<'_, I, O, S>,
        token: OperationToken,
        reference: KeyReference,
    ) -> Result<()> {
        self.validate_owner(authority)?;
        let operation = authority.operation(token).map_err(Error::Authority)?;
        if !matches!(
            operation.state,
            OperationState::Completed(OperationResult::Existing { .. })
        ) {
            return Err(Error::WrongAction);
        }
        let expected = match operation.command {
            Command::Create(request) => match request.source {
                KeySource::Import { expected_hash } => expected_hash,
                _ => return Err(Error::WrongAction),
            },
            _ => return Err(Error::WrongAction),
        };
        if operation
            .unpublished_key
            .is_some_and(|value| value != reference)
        {
            return Err(Error::KeyMismatch);
        }
        let path = self
            .root
            .join("retained")
            .join(format!("{}.keep", reference_name(reference)));
        let mut body = Vec::with_capacity(104);
        body.extend_from_slice(&reference.0);
        body.extend_from_slice(&expected.0);
        body.extend_from_slice(&token.incarnation);
        body.extend_from_slice(&token.sequence.to_be_bytes());
        body.extend_from_slice(&token.digest.0);
        if let Some(bytes) = files::read_bounded(&path, 152)? {
            if unwrap(&bytes, KEEP_MAGIC, 104)? != body {
                return Err(Error::KeyMismatch);
            }
            files::sync_published(&path)?;
            return Ok(());
        }
        if operation.unpublished_key != Some(reference) {
            return Err(Error::WrongAction);
        }
        let key = self
            .root
            .join("keys")
            .join(format!("{}.key", reference_name(reference)));
        read_key(&key, reference, Some(expected))?.ok_or(Error::MissingKey)?;
        let bytes = wrap(KEEP_MAGIC, &body)?;
        files::replace(&path, &path.with_extension("next"), &bytes, "retained")?;
        let stored = files::read_bounded(&path, 152)?.ok_or(Error::Corrupt)?;
        if unwrap(&stored, KEEP_MAGIC, 104)? != body {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
}

/// Publish immutable key material while the caller holds its directory lock.
/// A valid staged key is reused; a partial or contradictory key fails closed.
pub(crate) fn publish_key(
    path: &Path,
    reference: KeyReference,
    source: KeySource,
    import: Option<&IdentitySecretKey>,
) -> Result<IdentitySecretKey> {
    let expected = match source {
        KeySource::Generate => {
            if import.is_some() {
                return Err(Error::UnexpectedImport);
            }
            None
        }
        KeySource::Import { expected_hash } => {
            if let Some(secret) = import {
                if hash(secret) != expected_hash {
                    return Err(Error::KeyMismatch);
                }
            }
            Some(expected_hash)
        }
    };
    let pending = path.with_extension("pending");
    if let Some(secret) = read_key(path, reference, expected)? {
        if import.is_some_and(|value| value.as_ref() != secret.as_ref()) {
            return Err(Error::KeyMismatch);
        }
        // A contradictory retained stage is never silently discarded.
        if let Some(stage) = read_key(&pending, reference, expected)? {
            if stage.as_ref() != secret.as_ref() {
                return Err(Error::KeyMismatch);
            }
        }
        files::sync_published(path)?;
        return Ok(secret);
    }
    if let Some(secret) = read_key(&pending, reference, expected)? {
        if import.is_some_and(|value| value.as_ref() != secret.as_ref()) {
            return Err(Error::KeyMismatch);
        }
        files::sync_published(&pending)?;
    } else {
        files::crash("key", "before_stage");
        let secret = match source {
            KeySource::Import { .. } => import.cloned().ok_or(Error::ImportRequired)?,
            KeySource::Generate => {
                let mut entropy = RuntimeEntropy::try_new(OsEntropy).map_err(Error::Entropy)?;
                let mut secret = IdentitySecretKey::new([0; 64]);
                entropy.fill_random(secret.as_mut());
                secret
            }
        };
        let bytes = encode_key(reference, &secret);
        let mut file = files::new_file(&pending)?;
        file.write_all(bytes.as_ref())?;
        files::crash("key", "before_file_sync");
        file.sync_all()?;
        files::crash("key", "after_file_sync");
        // Retain the stage name durably before attempting publication.
        files::sync_directory(pending.parent().ok_or(Error::UnsafePath)?)?;
    }
    // Immutable publication: the exclusive owner must not overwrite a key.
    if files::check_file(path)? {
        return Err(Error::KeyMismatch);
    }
    fs::rename(&pending, path)?;
    files::crash("key", "after_rename");
    files::sync_directory(path.parent().ok_or(Error::UnsafePath)?)?;
    files::crash("key", "after_directory_sync");
    let secret = read_key(path, reference, expected)?.ok_or(Error::MissingKey)?;
    Ok(secret)
}

/// Resume a first publication without ever generating missing key material.
/// Existing bytes establish the expected identity only at this explicit initial
/// creation boundary; ordinary restoration must supply its pinned expectation.
pub(crate) fn recover_key(path: &Path, reference: KeyReference) -> Result<IdentitySecretKey> {
    let secret = match read_key(path, reference, None)? {
        Some(secret) => secret,
        None => {
            read_key(&path.with_extension("pending"), reference, None)?.ok_or(Error::MissingKey)?
        }
    };
    // Reuse immutable publication, including contradictory-stage checks and
    // file/directory sync. Import-without-input cannot generate a missing key.
    publish_key(
        path,
        reference,
        KeySource::Import {
            expected_hash: hash(&secret),
        },
        None,
    )
}

fn hash(secret: &IdentitySecretKey) -> PublicIdentityHash {
    let material = PrivateIdentityMaterial::from(secret.clone());
    PublicIdentityHash(*material.identity_hash().as_bytes())
}

fn encode_key(reference: KeyReference, secret: &IdentitySecretKey) -> Zeroizing<[u8; KEY_BYTES]> {
    let mut bytes = Zeroizing::new([0; KEY_BYTES]);
    bytes[..8].copy_from_slice(KEY_MAGIC);
    bytes[8..40].copy_from_slice(&reference.0);
    bytes[40..56].copy_from_slice(&hash(secret).0);
    bytes[56..120].copy_from_slice(secret.as_ref());
    let checksum = sha256(&bytes[..120]);
    bytes[120..].copy_from_slice(&checksum);
    bytes
}

pub(crate) fn read_key(
    path: &Path,
    reference: KeyReference,
    expected: Option<PublicIdentityHash>,
) -> Result<Option<IdentitySecretKey>> {
    if !files::check_file(path)? {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    if file.metadata()?.len() != KEY_BYTES as u64 {
        return Err(Error::Corrupt);
    }
    let mut bytes = Zeroizing::new([0; KEY_BYTES]);
    file.read_exact(bytes.as_mut())?;
    let mut trailing = [0; 1];
    if file.read(&mut trailing)? != 0
        || &bytes[..8] != KEY_MAGIC
        || sha256(&bytes[..120]).as_slice() != &bytes[120..]
    {
        return Err(Error::Corrupt);
    }
    if bytes[8..40] != reference.0 {
        return Err(Error::KeyMismatch);
    }
    let mut secret = IdentitySecretKey::new([0; 64]);
    secret.copy_from_slice(&bytes[56..120]);
    let derived = hash(&secret);
    if bytes[40..56] != derived.0 || expected.is_some_and(|value| value != derived) {
        return Err(Error::KeyMismatch);
    }
    Ok(Some(secret))
}
