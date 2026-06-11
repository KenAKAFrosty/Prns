use crate::crypto::{hkdf_sha256, hkdf_sha256_into, sha256, sha256_chunks};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::wire::BROADCAST_MTU;

/// RNS 1.3.1 `Reticulum.IFAC_SALT`.
pub const IFAC_SALT: [u8; 32] = [
    0xad, 0xf5, 0x4d, 0x88, 0x2c, 0x9a, 0x9b, 0x80, 0x77, 0x1e, 0xb4, 0x99, 0x5d, 0x70, 0x2d, 0x4a,
    0x3e, 0x73, 0x33, 0x91, 0xb2, 0xa0, 0xf5, 0x3f, 0x41, 0x6d, 0x9f, 0x90, 0x7e, 0x55, 0xcf, 0xf8,
];

pub const DEFAULT_IFAC_SIZE: usize = 8;

pub const IFAC_MAX_SIZE: usize = 64;

const IFAC_FLAG: u8 = 0x80;
const SIGNATURE_LEN: usize = 64;
const MAX_MASK_LEN: usize = BROADCAST_MTU + IFAC_MAX_SIZE;

pub struct InterfaceIfac {
    pub id: crate::interfaces::InterfaceId,
    pub context: IfacContext,
}

pub struct IfacContext {
    key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    identity: InMemoryNodeIdentity,
    size: usize,
}

impl IfacContext {
    pub fn derive(netname: Option<&str>, netkey: Option<&str>, size: usize) -> Option<Self> {
        if netname.is_none() && netkey.is_none() {
            return None;
        }
        let name_hash = netname.map(|name| sha256(name.as_bytes()));
        let key_hash = netkey.map(|key| sha256(key.as_bytes()));
        let origin_hash = match (&name_hash, &key_hash) {
            (Some(name), Some(key)) => sha256_chunks(&[name, key]),
            (Some(only), None) | (None, Some(only)) => sha256(only),
            (None, None) => unreachable!(),
        };
        let key = Zeroizing::new(hkdf_sha256::<IDENTITY_SECRET_KEY_LEN>(
            &origin_hash,
            &IFAC_SALT,
            &[],
        ));
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&key);
        Some(Self {
            key,
            identity,
            size: size.clamp(1, SIGNATURE_LEN),
        })
    }

    #[must_use]
    pub fn ifac_size(&self) -> usize {
        self.size
    }

    pub fn mask_outbound(&self, clean: &[u8], out: &mut [u8]) -> Option<usize> {
        let total = clean.len().checked_add(self.size)?;
        if clean.len() < 2 || clean.len() > BROADCAST_MTU || out.len() < total {
            return None;
        }
        let signature = self.identity.sign(clean);
        let ifac = &signature.0[SIGNATURE_LEN - self.size..];

        let mut mask = [0u8; MAX_MASK_LEN];
        hkdf_sha256_into(ifac, &*self.key, &[], &mut mask[..total]);

        out[0] = (clean[0] ^ mask[0]) | IFAC_FLAG;
        out[1] = clean[1] ^ mask[1];
        out[2..2 + self.size].copy_from_slice(ifac);
        for i in 2 + self.size..total {
            out[i] = clean[i - self.size] ^ mask[i];
        }
        Some(total)
    }

    pub fn unmask_inbound(&self, wire: &[u8], out: &mut [u8]) -> Option<usize> {
        if wire.len() <= 2 + self.size || wire.len() > MAX_MASK_LEN {
            return None;
        }
        if wire[0] & IFAC_FLAG == 0 {
            return None;
        }
        let clean_len = wire.len() - self.size;
        if out.len() < clean_len {
            return None;
        }
        let ifac = &wire[2..2 + self.size];

        let mut mask = [0u8; MAX_MASK_LEN];
        hkdf_sha256_into(ifac, &*self.key, &[], &mut mask[..wire.len()]);

        out[0] = (wire[0] ^ mask[0]) & !IFAC_FLAG;
        out[1] = wire[1] ^ mask[1];
        for i in 2 + self.size..wire.len() {
            out[i - self.size] = wire[i] ^ mask[i];
        }

        let expected = self.identity.sign(&out[..clean_len]);
        ct_eq(ifac, &expected.0[SIGNATURE_LEN - self.size..]).then_some(clean_len)
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{hx, RAW_ANNOUNCE};

    const REFERENCE_KEY: &str = "d6154017dde7498492067c746115fca3863d7fc12604733d0f814594f10e79fe\
         f641be626fdca080fe47907a6bcd6771744e5eabffc970f486202e02cfcb425b";

    const REFERENCE_MASKED: &str =
        "cf6c710f0d7e21c29b0d8f5ba6536e70bbbba6a15b618fbaa77a2e957e9d1fe12d7e6a800cd44d6e9b61b1c9\
         ac448ec481b53ed130c2aab39329b212ba92e0a8a5680924f3055f80f470b71fe42a0ab9f73098ed143d3ada\
         2ce442f22f722f0168456d53d76c1a33e6392493ba4f84d268f6e1b78c583cd1e0cdcf5a24755027d9665248\
         43fb3b088f21305d740a067c0878ad6b1b5625700824ac429e48d45c8c9b3b9a014243ef2461a2523a9b76b4\
         618f0839b02d8dfe21ba9d2af8";

    fn testnet() -> IfacContext {
        IfacContext::derive(Some("testnet"), Some("s3cret"), 8).unwrap()
    }

    #[test]
    fn derivation_matches_the_reference_key() {
        assert_eq!(testnet().key.as_slice(), hx(REFERENCE_KEY).as_slice());
        assert_eq!(
            IfacContext::derive(Some("testnet"), None, 8).unwrap().key[..16],
            hx("bedfc668194da1f48eeff6901693069d")[..],
            "a name alone also derives, per the reference's optional fields",
        );
        assert!(IfacContext::derive(None, None, 8).is_none());
    }

    #[test]
    fn masking_reproduces_the_reference_wire() {
        let clean = hx(RAW_ANNOUNCE);
        let mut out = [0u8; MAX_MASK_LEN];
        let written = testnet().mask_outbound(&clean, &mut out).unwrap();
        assert_eq!(out[..written], hx(REFERENCE_MASKED)[..]);
    }

    #[test]
    fn unmasking_recovers_and_verifies_the_reference_wire() {
        let wire = hx(REFERENCE_MASKED);
        let mut out = [0u8; MAX_MASK_LEN];
        let clean_len = testnet().unmask_inbound(&wire, &mut out).unwrap();
        assert_eq!(out[..clean_len], hx(RAW_ANNOUNCE)[..]);
    }

    #[test]
    fn a_tampered_packet_or_tag_fails_the_access_check() {
        let ctx = testnet();
        let mut out = [0u8; MAX_MASK_LEN];

        let mut tampered_payload = hx(REFERENCE_MASKED);
        tampered_payload[40] ^= 0x01;
        assert!(ctx.unmask_inbound(&tampered_payload, &mut out).is_none());

        let mut tampered_tag = hx(REFERENCE_MASKED);
        tampered_tag[3] ^= 0x01;
        assert!(ctx.unmask_inbound(&tampered_tag, &mut out).is_none());
    }

    #[test]
    fn the_wrong_network_code_opens_nothing() {
        let stranger = IfacContext::derive(Some("testnet"), Some("wrong"), 8).unwrap();
        let mut out = [0u8; MAX_MASK_LEN];
        assert!(stranger
            .unmask_inbound(&hx(REFERENCE_MASKED), &mut out)
            .is_none());
    }

    #[test]
    fn unflagged_or_truncated_wire_is_refused() {
        let ctx = testnet();
        let mut out = [0u8; MAX_MASK_LEN];

        let mut unflagged = hx(REFERENCE_MASKED);
        unflagged[0] &= 0x7f;
        assert!(ctx.unmask_inbound(&unflagged, &mut out).is_none());

        assert!(ctx
            .unmask_inbound(&hx(REFERENCE_MASKED)[..10], &mut out)
            .is_none());
    }

    #[test]
    fn a_wider_tag_round_trips_on_its_own() {
        let ctx = IfacContext::derive(None, Some("only-a-passphrase"), 16).unwrap();
        let clean = hx(RAW_ANNOUNCE);
        let mut wire = [0u8; MAX_MASK_LEN];
        let written = ctx.mask_outbound(&clean, &mut wire).unwrap();
        assert_eq!(written, clean.len() + 16);

        let mut back = [0u8; MAX_MASK_LEN];
        let clean_len = ctx.unmask_inbound(&wire[..written], &mut back).unwrap();
        assert_eq!(back[..clean_len], clean[..]);
    }
}
