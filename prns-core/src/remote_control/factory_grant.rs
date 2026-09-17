use crate::identity::vault::{IdentityLabel, IdentityVault, MAX_IDENTITY_LABEL_LEN};
use crate::identity::{IdentityPublicKeys, IDENTITY_PUBLIC_KEY_LEN, IDENTITY_SECRET_KEY_LEN};

use super::{
    parse_controller_public_keys, RemoteControlControllerAuthority, RemoteControlControllerGrant,
    RemoteControlRequestSet,
};

const TARGET_IDENTITY_LABEL: &str = "target";

/// Flash-time owner grant stored beside the target Remote Control identity.
pub const FACTORY_CONTROLLER_GRANT_LABEL: &str = "factory-grant";
pub const FACTORY_CONTROLLER_GRANT_MAGIC: &[u8; 8] = b"RCFG1\0\0\0";
pub const FACTORY_CONTROLLER_GRANT_BLOB_LEN: usize = 8 + IDENTITY_PUBLIC_KEY_LEN;
pub const REMOTE_CONTROL_IDENTITY_VAULT_PAGE_LEN: usize = 4096;

const SLOT_LEN: usize = 256;
const COMMIT_LEN: usize = 4;
const LABEL_LEN_OFFSET: usize = COMMIT_LEN;
const LABEL_OFFSET: usize = LABEL_LEN_OFFSET + 1;
const SECRET_OFFSET: usize = LABEL_OFFSET + MAX_IDENTITY_LABEL_LEN;
const SECRET_INVERSE_OFFSET: usize = SECRET_OFFSET + IDENTITY_SECRET_KEY_LEN;
const STATE_EMPTY: u8 = 0xFF;
const STATE_OCCUPIED: u8 = 0xA5;
const STATE_BLOB: u8 = 0x3C;
const BLOB_LEN_PREFIX_LEN: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactoryControllerGrantError {
    InvalidLabel,
    BlobTooLong,
}

fn write_bytes(dst: &mut [u8], at: usize, src: &[u8]) -> bool {
    let Some(end) = at.checked_add(src.len()) else {
        return false;
    };
    let Some(slot) = dst.get_mut(at..end) else {
        return false;
    };
    slot.copy_from_slice(src);
    true
}

/// Encodes the Controller allow-list key the target should trust after flash.
#[must_use]
pub fn encode_factory_controller_grant_blob(
    public_keys: &IdentityPublicKeys,
) -> [u8; FACTORY_CONTROLLER_GRANT_BLOB_LEN] {
    let mut blob = [0u8; FACTORY_CONTROLLER_GRANT_BLOB_LEN];
    let magic_ok = write_bytes(&mut blob, 0, FACTORY_CONTROLLER_GRANT_MAGIC);
    let keys_ok = write_bytes(
        &mut blob,
        FACTORY_CONTROLLER_GRANT_MAGIC.len(),
        &public_keys.public_key_bytes(),
    );
    debug_assert!(magic_ok && keys_ok);
    blob
}

#[must_use]
pub fn decode_factory_controller_grant_blob(bytes: &[u8]) -> Option<RemoteControlControllerGrant> {
    if bytes.len() != FACTORY_CONTROLLER_GRANT_BLOB_LEN
        || bytes.get(..FACTORY_CONTROLLER_GRANT_MAGIC.len())? != FACTORY_CONTROLLER_GRANT_MAGIC
    {
        return None;
    }
    let keys = bytes.get(FACTORY_CONTROLLER_GRANT_MAGIC.len()..)?;
    let identity = parse_controller_public_keys(keys)?;
    RemoteControlControllerGrant::new(
        identity,
        RemoteControlControllerAuthority::Administrator,
        RemoteControlRequestSet::all(),
    )
    .ok()
}

pub fn load_factory_controller_grant<V: IdentityVault>(
    vault: &V,
) -> Result<Option<RemoteControlControllerGrant>, V::Error> {
    let Ok(label) = IdentityLabel::new(FACTORY_CONTROLLER_GRANT_LABEL) else {
        return Ok(None);
    };
    let mut buffer = [0u8; FACTORY_CONTROLLER_GRANT_BLOB_LEN];
    let Some(bytes) = vault.load_blob(&label, &mut buffer)? else {
        return Ok(None);
    };
    Ok(decode_factory_controller_grant_blob(bytes))
}

/// One erase page: target identity in slot 0, factory grant blob in slot 1, vacant slot 2.
pub fn encode_remote_control_vault_page(
    target_secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    controller_public: &IdentityPublicKeys,
) -> Result<[u8; REMOTE_CONTROL_IDENTITY_VAULT_PAGE_LEN], FactoryControllerGrantError> {
    let target_label = IdentityLabel::new(TARGET_IDENTITY_LABEL)
        .map_err(|_| FactoryControllerGrantError::InvalidLabel)?;
    let grant_label = IdentityLabel::new(FACTORY_CONTROLLER_GRANT_LABEL)
        .map_err(|_| FactoryControllerGrantError::InvalidLabel)?;
    let blob = encode_factory_controller_grant_blob(controller_public);
    let mut page = [STATE_EMPTY; REMOTE_CONTROL_IDENTITY_VAULT_PAGE_LEN];
    if !write_bytes(
        &mut page,
        0,
        &encode_identity_slot(&target_label, target_secret),
    ) {
        return Err(FactoryControllerGrantError::BlobTooLong);
    }
    if !write_bytes(&mut page, SLOT_LEN, &encode_blob_slot(&grant_label, &blob)?) {
        return Err(FactoryControllerGrantError::BlobTooLong);
    }
    Ok(page)
}

fn encode_identity_slot(
    label: &IdentityLabel,
    secret: &[u8; IDENTITY_SECRET_KEY_LEN],
) -> [u8; SLOT_LEN] {
    let mut buffer = [STATE_EMPTY; SLOT_LEN];
    write_label(&mut buffer, label);
    let _ = write_bytes(&mut buffer, SECRET_OFFSET, secret);
    let inverse_end = SECRET_INVERSE_OFFSET.saturating_add(IDENTITY_SECRET_KEY_LEN);
    if let Some(inverse) = buffer.get_mut(SECRET_INVERSE_OFFSET..inverse_end) {
        for (dst, byte) in inverse.iter_mut().zip(secret.iter()) {
            *dst = !*byte;
        }
    }
    if let Some(commit) = buffer.first_mut() {
        *commit = STATE_OCCUPIED;
    }
    buffer
}

fn encode_blob_slot(
    label: &IdentityLabel,
    blob: &[u8],
) -> Result<[u8; SLOT_LEN], FactoryControllerGrantError> {
    let cap = SLOT_LEN
        .saturating_sub(SECRET_OFFSET)
        .saturating_sub(BLOB_LEN_PREFIX_LEN);
    if blob.len() > cap {
        return Err(FactoryControllerGrantError::BlobTooLong);
    }
    let mut buffer = [STATE_EMPTY; SLOT_LEN];
    write_label(&mut buffer, label);
    let len = u16::try_from(blob.len()).map_err(|_| FactoryControllerGrantError::BlobTooLong)?;
    if !write_bytes(&mut buffer, SECRET_OFFSET, &len.to_le_bytes()) {
        return Err(FactoryControllerGrantError::BlobTooLong);
    }
    let body_at = SECRET_OFFSET.saturating_add(BLOB_LEN_PREFIX_LEN);
    if !write_bytes(&mut buffer, body_at, blob) {
        return Err(FactoryControllerGrantError::BlobTooLong);
    }
    if let Some(commit) = buffer.first_mut() {
        *commit = STATE_BLOB;
    }
    Ok(buffer)
}

fn write_label(buffer: &mut [u8; SLOT_LEN], label: &IdentityLabel) {
    let bytes = label.as_str().as_bytes();
    if let Some(len_slot) = buffer.get_mut(LABEL_LEN_OFFSET) {
        *len_slot = bytes.len() as u8;
    }
    let _ = write_bytes(buffer, LABEL_OFFSET, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::{IdentitySigner, IDENTITY_SECRET_KEY_LEN};

    fn public_keys(fill: u8) -> IdentityPublicKeys {
        let mut secret = [0u8; IDENTITY_SECRET_KEY_LEN];
        secret.fill(fill);
        secret[0] = fill.saturating_add(1);
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret);
        IdentityPublicKeys {
            encryption: identity.encryption_public_key(),
            signing: identity.signing_public_key(),
        }
    }

    #[test]
    fn factory_grant_blob_round_trips_the_controller_public_keys() {
        let keys = public_keys(0x42);
        let blob = encode_factory_controller_grant_blob(&keys);
        let grant = decode_factory_controller_grant_blob(&blob)
            .expect("a well-formed factory grant blob decodes");
        assert_eq!(grant.controller().public_keys(), &keys);
        assert_eq!(grant.permitted_requests(), &RemoteControlRequestSet::all());
    }

    #[test]
    fn factory_grant_blob_rejects_a_truncated_or_foreign_payload() {
        assert_eq!(decode_factory_controller_grant_blob(&[0x00; 8]), None);
        let mut blob = encode_factory_controller_grant_blob(&public_keys(0x11));
        blob[0] = b'X';
        assert_eq!(decode_factory_controller_grant_blob(&blob), None);
    }

    #[test]
    fn vault_page_holds_the_target_identity_and_factory_grant_slots() {
        let mut target_secret = [0u8; IDENTITY_SECRET_KEY_LEN];
        target_secret[0] = 0xA1;
        target_secret[32] = 0xB2;
        let keys = public_keys(0x33);
        let page = encode_remote_control_vault_page(&target_secret, &keys)
            .expect("a factory vault page encodes");
        assert_eq!(page.first().copied(), Some(STATE_OCCUPIED));
        assert_eq!(page.get(SLOT_LEN).copied(), Some(STATE_BLOB));
        assert!(page
            .get(SLOT_LEN.saturating_mul(2)..)
            .is_some_and(|tail| tail.iter().all(|byte| *byte == STATE_EMPTY)));
        let stored_secret = page
            .get(SECRET_OFFSET..SECRET_OFFSET.saturating_add(IDENTITY_SECRET_KEY_LEN))
            .expect("target secret span fits");
        assert_eq!(stored_secret, &target_secret);
        let grant_body = SECRET_OFFSET.saturating_add(BLOB_LEN_PREFIX_LEN);
        let start = SLOT_LEN.saturating_add(grant_body);
        let end = start.saturating_add(FACTORY_CONTROLLER_GRANT_BLOB_LEN);
        let stored = page.get(start..end).expect("grant blob span fits");
        assert_eq!(
            decode_factory_controller_grant_blob(stored)
                .expect("the stored blob decodes")
                .controller()
                .public_keys(),
            &keys
        );
    }
}
