#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::crypto::{Ed25519PublicKey, X25519PublicKey};
use prns_core::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey,
};
use prns_core::persistence::{
    read_remote_control_access_snapshot, remote_control_access_snapshot_capacity,
    write_remote_control_access_snapshot,
};
use prns_core::remote_control::{
    RemoteControlControllerGrant, RemoteControlControllerIdentity, RemoteControlRequestKind,
    RemoteControlRequestSet,
};

fuzz_target!(|data: &[u8]| {
    let mut grants = Vec::new();
    for row in data.chunks(2).take(8) {
        let Some(fill) = row.first().copied() else {
            continue;
        };
        let selector = row.get(1).copied().unwrap_or_default();
        let mut permitted_requests = RemoteControlRequestSet::empty();
        if selector & 0x01 != 0 {
            let _inserted = permitted_requests.insert(RemoteControlRequestKind::Describe);
        }
        if selector & 0x02 != 0 {
            let _inserted = permitted_requests.insert(RemoteControlRequestKind::AnnounceSelf);
        }
        if permitted_requests.is_empty() {
            let _inserted = permitted_requests.insert(RemoteControlRequestKind::Describe);
        }
        let controller = RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill.wrapping_add(1); 32])),
        });
        grants.push(
            RemoteControlControllerGrant::new(controller, permitted_requests)
                .expect("the generated request set is nonempty"),
        );
    }

    let mut encoded = vec![0u8; remote_control_access_snapshot_capacity(grants.len())];
    let written = write_remote_control_access_snapshot(grants.iter().copied(), &mut encoded)
        .expect("the exact snapshot capacity must fit every generated grant");
    let parsed = read_remote_control_access_snapshot(&encoded[..written])
        .expect("a freshly written access snapshot must parse");
    assert_eq!(parsed.collect::<Vec<_>>(), grants);

    let mut mutated = encoded[..written].to_vec();
    for (byte, mutation) in mutated.iter_mut().zip(data.iter().rev()) {
        *byte ^= mutation;
    }
    if let Ok(mut parsed) = read_remote_control_access_snapshot(&mutated) {
        while parsed.next().is_some() {}
        assert_eq!(parsed.len(), 0);
    }
});
