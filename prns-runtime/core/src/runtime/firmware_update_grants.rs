use core::sync::atomic::{AtomicU8, Ordering};

use crate::identity::IdentityHash;
use crate::remote_control::{RemoteControlControllerGrantTable, RemoteControlRequestKind};

const CAP: usize = 8;

static COUNT: AtomicU8 = AtomicU8::new(0);
static BYTES: [[AtomicU8; 16]; CAP] = [const { [const { AtomicU8::new(0) }; 16] }; CAP];

pub fn note_firmware_update_grants(table: &impl RemoteControlControllerGrantTable) {
    let mut count = 0u8;
    for grant in table.grants_in_identity_hash_order() {
        if count as usize == CAP {
            break;
        }
        if grant
            .effective_requests()
            .supports(RemoteControlRequestKind::FirmwareUpdate)
        {
            let identity = grant.controller().identity_hash();
            let bytes = identity.as_bytes();
            for (slot, byte) in BYTES[count as usize].iter().zip(bytes) {
                slot.store(*byte, Ordering::Relaxed);
            }
            count = count.saturating_add(1);
        }
    }
    COUNT.store(count, Ordering::Release);
}

#[must_use]
pub fn firmware_update_permitted(identity: &IdentityHash) -> bool {
    firmware_update_noted_grant_index(identity).is_some()
}

#[must_use]
pub fn firmware_update_noted_grant_count() -> u8 {
    COUNT.load(Ordering::Acquire)
}

#[must_use]
pub fn firmware_update_noted_grant(index: u8) -> Option<IdentityHash> {
    if index >= firmware_update_noted_grant_count() {
        return None;
    }
    let mut found = [0u8; 16];
    for (slot, byte) in found.iter_mut().zip(&BYTES[index as usize]) {
        *slot = byte.load(Ordering::Relaxed);
    }
    Some(IdentityHash::new(found))
}

fn firmware_update_noted_grant_index(identity: &IdentityHash) -> Option<u8> {
    let wanted = identity.as_bytes();
    let mut index = 0u8;
    while index < firmware_update_noted_grant_count() {
        let mut found = [0u8; 16];
        for (slot, byte) in found.iter_mut().zip(&BYTES[index as usize]) {
            *slot = byte.load(Ordering::Relaxed);
        }
        if &found == wanted {
            return Some(index);
        }
        index = index.saturating_add(1);
    }
    None
}
