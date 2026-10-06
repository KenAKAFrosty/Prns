use super::RemoteControlNodeIdentitySecrets;

/// Retains already-derived test keys while each simulated node gets its own owned secrets.
/// Mutable engine, authorization, and persistence state are never shared by the fixture.
pub struct RemoteControlIdentityFixture(RemoteControlNodeIdentitySecrets);

impl RemoteControlIdentityFixture {
    pub fn new(identities: RemoteControlNodeIdentitySecrets) -> Self {
        Self(identities)
    }

    pub fn fresh_secrets(&self) -> RemoteControlNodeIdentitySecrets {
        self.0.duplicate_for_fixture()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::crypto::{ed25519_sign, ed25519_verify, x25519_public_key};
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::IDENTITY_SECRET_KEY_LEN;
    use crate::remote_control::{
        RemoteControlControllerIdentitySecret, RemoteControlTargetIdentitySecret,
    };

    #[test]
    fn fresh_secrets_keep_key_pairs_and_survive_dropping_another_copy() {
        let original = RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
                [0x71; IDENTITY_SECRET_KEY_LEN],
            )),
            RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
                [0x72; IDENTITY_SECRET_KEY_LEN],
            )),
        )
        .unwrap();
        let identities = original.identities();
        let sealing_key = original.target_sealing_key(b"fixture-test");
        let fixture = RemoteControlIdentityFixture::new(original);
        let first = fixture.fresh_secrets();
        let second = fixture.fresh_secrets();
        drop(first);
        drop(fixture);
        assert_eq!(second.identities(), identities);
        assert_eq!(
            second.target_sealing_key(b"fixture-test").as_bytes(),
            sealing_key.as_bytes()
        );
        let (controller, target) = second.into_parts();
        for parts in [controller, target] {
            assert_eq!(
                x25519_public_key(&parts.encryption_secret),
                *parts.encryption_public.as_x25519()
            );
            let message = b"independently owned fixture keys";
            let signature = ed25519_sign(&parts.signing_secret, message);
            ed25519_verify(parts.signing_public.as_ed25519(), message, &signature).unwrap();
        }
    }
}
