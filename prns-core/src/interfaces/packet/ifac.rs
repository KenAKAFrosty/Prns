use crate::crypto::{hkdf_sha256, hmac_sha256, hmac_sha256_chunks, sha256, sha256_chunks};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};

/// RNS 1.4.2 `Reticulum.IFAC_SALT`.
const IFAC_SALT: [u8; 32] = [
    0xad, 0xf5, 0x4d, 0x88, 0x2c, 0x9a, 0x9b, 0x80, 0x77, 0x1e, 0xb4, 0x99, 0x5d, 0x70, 0x2d, 0x4a,
    0x3e, 0x73, 0x33, 0x91, 0xb2, 0xa0, 0xf5, 0x3f, 0x41, 0x6d, 0x9f, 0x90, 0x7e, 0x55, 0xcf, 0xf8,
];

pub const IFAC_MAX_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfacSize(u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfacSizeError {
    TooSmall,
    TooLarge,
}

#[derive(Debug, PartialEq, Eq)]
pub enum IfacMaskError {
    PacketTooShort,
    LengthOverflow,
    OutputTooSmall { required: usize, available: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfacUnmaskError {
    PacketTooShort,
    MissingFlag,
    InvalidSignature,
    OutputTooSmall { required: usize, available: usize },
}

impl IfacSize {
    pub const MIN: Self = Self(1);
    pub const NARROW: Self = Self(8);
    pub const WIDE: Self = Self(16);
    pub const MAX: Self = Self(IFAC_MAX_SIZE as u8);

    pub const fn new(bytes: usize) -> Result<Self, IfacSizeError> {
        if bytes == 0 {
            Err(IfacSizeError::TooSmall)
        } else if bytes > IFAC_MAX_SIZE {
            Err(IfacSizeError::TooLarge)
        } else {
            Ok(Self(bytes as u8))
        }
    }

    #[must_use]
    pub const fn bytes(self) -> usize {
        self.0 as usize
    }
}

impl TryFrom<usize> for IfacSize {
    type Error = IfacSizeError;

    fn try_from(bytes: usize) -> Result<Self, Self::Error> {
        Self::new(bytes)
    }
}

pub const DEFAULT_IFAC_SIZE: IfacSize = IfacSize::NARROW;

const IFAC_FLAG: u8 = 0x80;
const HEADER_FLAGS_INDEX: usize = 0;
const HEADER_HOPS_INDEX: usize = 1;
const IFAC_START: usize = HEADER_HOPS_INDEX + 1;
const HMAC_SHA256_OUTPUT_LEN: usize = 32;
const SIGNATURE_BYTE_LEN: usize = 64;

fn masked_packet_len(clean_len: usize, ifac_size: IfacSize) -> Result<usize, IfacMaskError> {
    clean_len
        .checked_add(ifac_size.bytes())
        .ok_or(IfacMaskError::LengthOverflow)
}

pub struct InterfaceIfac {
    pub id: crate::interfaces::InterfaceId,
    pub context: IfacContext,
}

pub struct IfacContext {
    key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    identity: InMemoryNodeIdentity,
    size: IfacSize,
}

impl Clone for IfacContext {
    fn clone(&self) -> Self {
        let key = self.key.clone();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&key);
        Self {
            key,
            identity,
            size: self.size,
        }
    }
}

impl IfacContext {
    pub fn derive(netname: Option<&str>, netkey: Option<&str>, size: IfacSize) -> Option<Self> {
        let netname = netname.filter(|value| !value.is_empty());
        let netkey = netkey.filter(|value| !value.is_empty());
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
            size,
        })
    }

    #[must_use]
    pub fn ifac_size(&self) -> IfacSize {
        self.size
    }

    #[must_use]
    pub fn ifac_signature(&self) -> [u8; SIGNATURE_BYTE_LEN] {
        self.identity.sign(&sha256(&self.key[..])).0
    }

    pub fn mask_outbound(&self, clean: &[u8], out: &mut [u8]) -> Option<usize> {
        self.try_mask_outbound(clean, out).ok()
    }

    pub fn try_mask_outbound(&self, clean: &[u8], out: &mut [u8]) -> Result<usize, IfacMaskError> {
        let size = self.size.bytes();
        let total = masked_packet_len(clean.len(), self.size)?;
        if clean.len() <= IFAC_START {
            return Err(IfacMaskError::PacketTooShort);
        }
        if out.len() < total {
            return Err(IfacMaskError::OutputTooSmall {
                required: total,
                available: out.len(),
            });
        }
        let signature = self.identity.sign(clean);
        let ifac = &signature.0[SIGNATURE_BYTE_LEN - size..];
        let ifac_end = IFAC_START + size;

        out[HEADER_FLAGS_INDEX] = clean[HEADER_FLAGS_INDEX] | IFAC_FLAG;
        out[HEADER_HOPS_INDEX] = clean[HEADER_HOPS_INDEX];
        out[IFAC_START..ifac_end].copy_from_slice(ifac);
        out[ifac_end..total].copy_from_slice(&clean[IFAC_START..]);
        apply_rns_mask(ifac, &self.key[..], &mut out[..total], IFAC_START..ifac_end);
        out[HEADER_FLAGS_INDEX] |= IFAC_FLAG;
        Ok(total)
    }

    pub fn unmask_inbound(&self, wire: &[u8], out: &mut [u8]) -> Option<usize> {
        self.try_unmask_inbound(wire, out).ok()
    }

    pub fn try_unmask_inbound(
        &self,
        wire: &[u8],
        out: &mut [u8],
    ) -> Result<usize, IfacUnmaskError> {
        if out.len() < wire.len() {
            return Err(IfacUnmaskError::OutputTooSmall {
                required: wire.len(),
                available: out.len(),
            });
        }
        out[..wire.len()].copy_from_slice(wire);
        self.try_unmask_inbound_in_place(&mut out[..wire.len()])
    }

    pub fn unmask_inbound_in_place(&self, wire: &mut [u8]) -> Option<usize> {
        self.try_unmask_inbound_in_place(wire).ok()
    }

    pub fn try_unmask_inbound_in_place(&self, wire: &mut [u8]) -> Result<usize, IfacUnmaskError> {
        let size = self.size.bytes();
        let ifac_end = IFAC_START + size;
        if wire.len() <= ifac_end {
            return Err(IfacUnmaskError::PacketTooShort);
        }
        if wire[HEADER_FLAGS_INDEX] & IFAC_FLAG == 0 {
            return Err(IfacUnmaskError::MissingFlag);
        }
        let wire_bytes = wire.len();
        let clean_len = wire_bytes - size;
        let mut ifac = [0u8; IFAC_MAX_SIZE];
        ifac[..size].copy_from_slice(&wire[IFAC_START..ifac_end]);
        apply_rns_mask(&ifac[..size], &self.key[..], wire, IFAC_START..ifac_end);
        wire[HEADER_FLAGS_INDEX] &= !IFAC_FLAG;
        wire.copy_within(ifac_end..wire_bytes, IFAC_START);

        let expected = self.identity.sign(&wire[..clean_len]);
        if ct_eq(&ifac[..size], &expected.0[SIGNATURE_BYTE_LEN - size..]) {
            Ok(clean_len)
        } else {
            Err(IfacUnmaskError::InvalidSignature)
        }
    }
}

fn apply_rns_mask(
    derive_from: &[u8],
    salt: &[u8],
    bytes: &mut [u8],
    unmasked_ifac: core::ops::Range<usize>,
) {
    let pseudorandom_key = hmac_sha256(salt, derive_from);
    let mut previous = [0u8; HMAC_SHA256_OUTPUT_LEN];
    for (block_index, chunk) in bytes.chunks_mut(HMAC_SHA256_OUTPUT_LEN).enumerate() {
        // RNS 1.4.2 HKDF.py uses `bytes([(i + 1) % 256])`, so long IFAC masks wrap the counter to zero.
        let counter = [(block_index + 1) as u8];
        let block = if block_index == 0 {
            hmac_sha256_chunks(&pseudorandom_key, &[&counter])
        } else {
            hmac_sha256_chunks(&pseudorandom_key, &[&previous, &counter])
        };
        let offset = block_index * HMAC_SHA256_OUTPUT_LEN;
        for (index, byte) in chunk.iter_mut().enumerate() {
            let position = offset + index;
            if !unmasked_ifac.contains(&position) {
                *byte ^= block[index];
            }
        }
        previous = block;
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
    use crate::engine::test_support::{bytes_from_hex, RNS_1_4_2_ANNOUNCE};
    use crate::wire::BROADCAST_MTU;
    use proptest::prelude::*;

    const TEST_MASK_LEN: usize = BROADCAST_MTU + IFAC_MAX_SIZE;

    const REFERENCE_KEY: &str = "d6154017dde7498492067c746115fca3863d7fc12604733d0f814594f10e79fe\
         f641be626fdca080fe47907a6bcd6771744e5eabffc970f486202e02cfcb425b";

    const REFERENCE_MASKED: &str =
        "cf6c710f0d7e21c29b0d8f5ba6536e70bbbba6a15b618fbaa77a2e957e9d1fe12d7e6a800cd44d6e9b61b1c9\
         ac448ec481b53ed130c2aab39329b212ba92e0a8a5680924f3055f80f470b71fe42a0ab9f73098ed143d3ada\
         2ce442f22f722f0168456d53d76c1a33e6392493ba4f84d268f6e1b78c583cd1e0cdcf5a24755027d9665248\
         43fb3b088f21305d740a067c0878ad6b1b5625700824ac429e48d45c8c9b3b9a014243ef2461a2523a9b76b4\
         618f0839b02d8dfe21ba9d2af8";

    fn testnet() -> IfacContext {
        IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::NARROW).unwrap()
    }

    #[test]
    fn derivation_matches_the_reference_key() {
        assert_eq!(
            testnet().key.as_slice(),
            bytes_from_hex(REFERENCE_KEY).as_slice()
        );
        assert_eq!(
            IfacContext::derive(Some("testnet"), None, IfacSize::NARROW)
                .unwrap()
                .key[..16],
            bytes_from_hex("bedfc668194da1f48eeff6901693069d")[..],
            "a name alone also derives, per the reference's optional fields",
        );
        assert_eq!(
            testnet().ifac_signature().as_slice(),
            bytes_from_hex(
                "63d666ff07c9d1bfa595893b3e5c74eedd582a696fd6f3a0863ac1b77b2f6673\
                 e433e2902ad24bb9a806ad112d7d6096c142218f641ae5190218351141360e0f"
            )
            .as_slice(),
        );
        assert!(IfacContext::derive(None, None, IfacSize::NARROW).is_none());
        assert!(IfacContext::derive(Some(""), Some(""), IfacSize::NARROW).is_none());
    }

    #[test]
    fn constant_time_equality_does_not_cancel_repeated_differences() {
        assert!(ct_eq(&[0x10, 0x20], &[0x10, 0x20]));
        assert!(!ct_eq(&[0x10, 0x10], &[0x20, 0x20]));
        assert!(!ct_eq(&[0x10], &[0x10, 0x20]));
    }

    #[test]
    fn masking_reproduces_the_reference_wire() {
        let clean = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let mut out = [0u8; TEST_MASK_LEN];
        let written = testnet().mask_outbound(&clean, &mut out).unwrap();
        assert_eq!(out[..written], bytes_from_hex(REFERENCE_MASKED)[..]);
    }

    #[test]
    fn unmasking_recovers_and_verifies_the_reference_wire() {
        let wire = bytes_from_hex(REFERENCE_MASKED);
        let mut out = std::vec![0u8; wire.len()];
        let clean_len = testnet().unmask_inbound(&wire, &mut out).unwrap();
        assert_eq!(out[..clean_len], bytes_from_hex(RNS_1_4_2_ANNOUNCE)[..]);
    }

    #[test]
    fn a_tampered_packet_or_tag_fails_the_access_check() {
        let ctx = testnet();
        let mut out = [0u8; TEST_MASK_LEN];

        let mut tampered_payload = bytes_from_hex(REFERENCE_MASKED);
        tampered_payload[40] ^= 0x01;
        assert!(ctx.unmask_inbound(&tampered_payload, &mut out).is_none());

        let mut tampered_tag = bytes_from_hex(REFERENCE_MASKED);
        tampered_tag[3] ^= 0x01;
        assert!(ctx.unmask_inbound(&tampered_tag, &mut out).is_none());
    }

    #[test]
    fn the_wrong_network_code_opens_nothing() {
        let stranger =
            IfacContext::derive(Some("testnet"), Some("wrong"), IfacSize::NARROW).unwrap();
        let mut out = [0u8; TEST_MASK_LEN];
        assert!(stranger
            .unmask_inbound(&bytes_from_hex(REFERENCE_MASKED), &mut out)
            .is_none());
    }

    #[test]
    fn unflagged_or_truncated_wire_is_refused() {
        let ctx = testnet();
        let mut out = [0u8; TEST_MASK_LEN];
        assert!(ctx
            .mask_outbound(&[0u8; IFAC_START - 1], &mut out)
            .is_none());
        assert!(ctx.mask_outbound(&[0u8; IFAC_START], &mut out).is_none());
        let clean = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let mut short = std::vec![0u8; clean.len() + IfacSize::NARROW.bytes() - 1];
        assert!(ctx.mask_outbound(&clean, &mut short).is_none());
        let masked = bytes_from_hex(REFERENCE_MASKED);
        let mut short = std::vec![0u8; masked.len() - 1];
        assert!(ctx.unmask_inbound(&masked, &mut short).is_none());

        let mut unflagged = bytes_from_hex(REFERENCE_MASKED);
        unflagged[0] &= 0x7f;
        assert!(ctx.unmask_inbound(&unflagged, &mut out).is_none());

        assert!(ctx
            .unmask_inbound(&bytes_from_hex(REFERENCE_MASKED)[..10], &mut out)
            .is_none());
    }

    #[test]
    fn checked_unmasking_distinguishes_protocol_shape_from_access_refusal() {
        let ctx = testnet();
        let masked = bytes_from_hex(REFERENCE_MASKED);
        let mut out = [0u8; TEST_MASK_LEN];

        assert_eq!(
            ctx.try_unmask_inbound(&masked[..10], &mut out),
            Err(IfacUnmaskError::PacketTooShort),
        );

        let mut unflagged = masked.clone();
        unflagged[0] &= !IFAC_FLAG;
        assert_eq!(
            ctx.try_unmask_inbound(&unflagged, &mut out),
            Err(IfacUnmaskError::MissingFlag),
        );

        let mut tampered = masked.clone();
        tampered[3] ^= 1;
        assert_eq!(
            ctx.try_unmask_inbound(&tampered, &mut out),
            Err(IfacUnmaskError::InvalidSignature),
        );

        let available = masked.len() - 1;
        assert_eq!(
            ctx.try_unmask_inbound(&masked, &mut out[..available]),
            Err(IfacUnmaskError::OutputTooSmall {
                required: masked.len(),
                available,
            }),
        );
    }

    #[test]
    fn checked_masking_distinguishes_invalid_input_from_insufficient_output() {
        let ctx = testnet();
        let mut out = [0u8; TEST_MASK_LEN];
        assert_eq!(
            ctx.try_mask_outbound(&[0u8; IFAC_START], &mut out),
            Err(IfacMaskError::PacketTooShort)
        );

        let clean = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let available = clean.len() + IfacSize::NARROW.bytes() - 1;
        assert_eq!(
            ctx.try_mask_outbound(&clean, &mut out[..available]),
            Err(IfacMaskError::OutputTooSmall {
                required: available + 1,
                available,
            })
        );
        assert_eq!(
            masked_packet_len(usize::MAX, IfacSize::MIN),
            Err(IfacMaskError::LengthOverflow)
        );
    }

    #[test]
    fn a_wider_tag_round_trips_on_its_own() {
        let ctx = IfacContext::derive(None, Some("only-a-passphrase"), IfacSize::WIDE).unwrap();
        let clean = bytes_from_hex(RNS_1_4_2_ANNOUNCE);
        let mut wire = [0u8; TEST_MASK_LEN];
        let written = ctx.mask_outbound(&clean, &mut wire).unwrap();
        assert_eq!(written, clean.len() + 16);

        let mut back = [0u8; TEST_MASK_LEN];
        let clean_len = ctx.unmask_inbound(&wire[..written], &mut back).unwrap();
        assert_eq!(back[..clean_len], clean[..]);
    }

    #[test]
    fn ifac_sizes_refuse_values_outside_the_signature() {
        assert_eq!(IfacSize::new(0), Err(IfacSizeError::TooSmall));
        assert_eq!(IfacSize::new(1), Ok(IfacSize::MIN));
        assert_eq!(IfacSize::new(64), Ok(IfacSize::MAX));
        assert_eq!(IfacSize::new(65), Err(IfacSizeError::TooLarge));
    }

    #[test]
    fn high_mtu_masks_match_rns_across_the_counter_wrap() {
        let context = IfacContext::derive(Some("testnet"), Some("s3cret"), IfacSize::WIDE).unwrap();
        let cases = [
            (
                1_024,
                "7ae79e8b2beed7930f47336b96345517b185589af21b1511d956873d11b22775",
            ),
            (
                8_193,
                "b9b5d7deeb22739ed7fe17d963ec728f31692210e471fd5cff0a7f8b5820ced6",
            ),
            (
                crate::routing::links::MAX_LINK_MTU,
                "739f6d9643a03c46150f625ce9d89cfc4082c2204642046d826bf0039fd1b76b",
            ),
        ];

        for (clean_len, expected_hash) in cases {
            let mut clean = std::vec![0u8; clean_len];
            clean[0] = 1;
            for (index, byte) in clean[2..].iter_mut().enumerate() {
                *byte = (index as u8).wrapping_mul(17).wrapping_add(3);
            }
            let mut wire = std::vec![0u8; clean_len + IfacSize::WIDE.bytes()];
            let wire_bytes = context.mask_outbound(&clean, &mut wire).unwrap();
            assert_eq!(
                sha256(&wire[..wire_bytes]).as_slice(),
                bytes_from_hex(expected_hash).as_slice(),
            );
            let opened_len = context
                .unmask_inbound_in_place(&mut wire[..wire_bytes])
                .unwrap();
            assert_eq!(&wire[..opened_len], clean.as_slice());
        }
    }

    proptest! {
        #[test]
        fn arbitrary_open_headers_round_trip_for_every_ifac_size(
            mut clean in proptest::collection::vec(any::<u8>(), 3..=BROADCAST_MTU),
            size in 1usize..=IFAC_MAX_SIZE,
        ) {
            clean[0] &= !IFAC_FLAG;
            let size = IfacSize::new(size).unwrap();
            let ctx = IfacContext::derive(Some("propnet"), Some("propkey"), size).unwrap();
            prop_assert_eq!(ctx.ifac_size(), size);

            let mut wire = [0u8; TEST_MASK_LEN];
            let written = ctx
                .mask_outbound(&clean, &mut wire)
                .expect("generated payload fits the broadcast MTU and IFAC scratch");
            prop_assert_eq!(written, clean.len() + ctx.ifac_size().bytes());
            prop_assert_ne!(&wire[..written], clean.as_slice());

            let mut opened = [0u8; TEST_MASK_LEN];
            let opened_len = ctx
                .unmask_inbound(&wire[..written], &mut opened)
                .expect("the same IFAC context must verify its own packet");
            prop_assert_eq!(opened_len, clean.len());
            prop_assert_eq!(&opened[..opened_len], clean.as_slice());
        }
    }
}
