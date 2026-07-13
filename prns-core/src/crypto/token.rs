//! RNS's Fernet-style token: `iv ‖ AES-CBC(PKCS7(plaintext)) ‖ HMAC-SHA256`.
//! Encrypt-then-MAC; the key splits into a signing half and an encryption half (16+16 for a 32-byte key → AES-128, 32+32 for a 64-byte key → AES-256).

use aes::{Aes128, Aes256};
use cbc::cipher::block_padding::{Pkcs7, RawPadding, UnpadError};
use cbc::cipher::inout::InOutBuf;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use cbc::{Decryptor, Encryptor};

use super::mac::{HmacSha256Stream, InvalidMac};
use super::{hmac_sha256, hmac_sha256_verify};

const IV_LEN: usize = 16;
const MAC_LEN: usize = 32;
const BLOCK_LEN: usize = 16;

/// RNS 1.3.5 `Identity.TOKEN_OVERHEAD`: the 16-byte IV and 32-byte HMAC around every sealed payload.
pub const TOKEN_OVERHEAD: usize = IV_LEN + MAC_LEN;

/// PKCS#7 always pads (1..=`BLOCK_LEN` bytes), so a sealed token strictly outgrows its plaintext.
pub const fn sealed_len(plaintext_len: usize) -> usize {
    IV_LEN + (plaintext_len / BLOCK_LEN + 1) * BLOCK_LEN + MAC_LEN
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadKeyLength;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferTooShort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenOpenError {
    Malformed,
    InvalidMac,
    InvalidPadding,
    BufferTooShort,
}

#[derive(Clone, Copy)]
enum AesMode {
    Aes128,
    Aes256,
}

pub struct TokenKey<'a> {
    signing_key: &'a [u8],
    encryption_key: &'a [u8],
    mode: AesMode,
}

impl<'a> TokenKey<'a> {
    pub fn from_derived(key: &'a [u8]) -> Result<Self, BadKeyLength> {
        if let Ok(key) = <&[u8; 32]>::try_from(key) {
            return Ok(Self::from_aes128(key));
        }
        if let Ok(key) = <&[u8; 64]>::try_from(key) {
            return Ok(Self::from_aes256(key));
        }
        Err(BadKeyLength)
    }

    pub fn from_aes128(key: &'a [u8; 32]) -> Self {
        Self {
            signing_key: &key[..16],
            encryption_key: &key[16..],
            mode: AesMode::Aes128,
        }
    }

    pub fn from_aes256(key: &'a [u8; 64]) -> Self {
        Self {
            signing_key: &key[..32],
            encryption_key: &key[32..],
            mode: AesMode::Aes256,
        }
    }
}

pub fn token_seal(
    key: &TokenKey,
    iv: &[u8; IV_LEN],
    plaintext: &[u8],
    out: &mut [u8],
) -> Result<usize, BufferTooShort> {
    token_seal_chunks(key, iv, &[plaintext], out)
}

/// `chunks` seal exactly as if concatenated.
#[allow(clippy::expect_used)]
pub fn token_seal_chunks(
    key: &TokenKey,
    iv: &[u8; IV_LEN],
    chunks: &[&[u8]],
    out: &mut [u8],
) -> Result<usize, BufferTooShort> {
    let plain_len: usize = chunks.iter().map(|chunk| chunk.len()).sum();
    let total = sealed_len(plain_len);
    if out.len() < total {
        return Err(BufferTooShort);
    }

    out[..IV_LEN].copy_from_slice(iv);
    let cipher_region = &mut out[IV_LEN..total - MAC_LEN];
    let mut at = 0;
    for chunk in chunks {
        cipher_region[at..at + chunk.len()].copy_from_slice(chunk);
        at += chunk.len();
    }
    match key.mode {
        AesMode::Aes128 => Encryptor::<Aes128>::new_from_slices(key.encryption_key, iv)
            .expect("TokenKey construction sizes the key halves")
            .encrypt_padded_mut::<Pkcs7>(cipher_region, plain_len)
            .expect("the padded region was sized for PKCS#7 above"),
        AesMode::Aes256 => Encryptor::<Aes256>::new_from_slices(key.encryption_key, iv)
            .expect("TokenKey construction sizes the key halves")
            .encrypt_padded_mut::<Pkcs7>(cipher_region, plain_len)
            .expect("the padded region was sized for PKCS#7 above"),
    };

    let mac = hmac_sha256(key.signing_key, &out[..total - MAC_LEN]);
    out[total - MAC_LEN..total].copy_from_slice(&mac);
    Ok(total)
}

/// [`token_seal_chunks`] for a plaintext already sitting in `out` at its sealed offset (`IV_LEN..IV_LEN + plain_len`): pads, encrypts, and MACs in place, skipping the staging copy.
#[allow(clippy::expect_used)]
pub fn token_seal_in_place(
    key: &TokenKey,
    iv: &[u8; IV_LEN],
    out: &mut [u8],
    plain_len: usize,
) -> Result<usize, BufferTooShort> {
    let total = sealed_len(plain_len);
    if out.len() < total {
        return Err(BufferTooShort);
    }

    out[..IV_LEN].copy_from_slice(iv);
    let cipher_region = &mut out[IV_LEN..total - MAC_LEN];
    match key.mode {
        AesMode::Aes128 => Encryptor::<Aes128>::new_from_slices(key.encryption_key, iv)
            .expect("TokenKey construction sizes the key halves")
            .encrypt_padded_mut::<Pkcs7>(cipher_region, plain_len)
            .expect("the padded region was sized for PKCS#7 above"),
        AesMode::Aes256 => Encryptor::<Aes256>::new_from_slices(key.encryption_key, iv)
            .expect("TokenKey construction sizes the key halves")
            .encrypt_padded_mut::<Pkcs7>(cipher_region, plain_len)
            .expect("the padded region was sized for PKCS#7 above"),
    };

    let mac = hmac_sha256(key.signing_key, &out[..total - MAC_LEN]);
    out[total - MAC_LEN..total].copy_from_slice(&mac);
    Ok(total)
}

/// The mutation-free prefix of [`token_open_in_place`], for ratchet trials before the one in-place decrypt.
pub fn token_is_authentic(key: &TokenKey, token: &[u8]) -> bool {
    if token.len() < IV_LEN + BLOCK_LEN + MAC_LEN {
        return false;
    }
    let (signed_parts, tag) = token.split_at(token.len() - MAC_LEN);
    hmac_sha256_verify(key.signing_key, signed_parts, tag).is_ok()
}

/// MAC-verified (constant time) then decrypted in place; the plaintext is a sub-slice of `token`.
#[allow(clippy::expect_used)]
pub fn token_open_in_place<'t>(
    key: &TokenKey,
    token: &'t mut [u8],
) -> Result<&'t [u8], TokenOpenError> {
    if token.len() < IV_LEN + BLOCK_LEN + MAC_LEN {
        return Err(TokenOpenError::Malformed);
    }
    let (signed_parts, tag) = token.split_at_mut(token.len() - MAC_LEN);
    hmac_sha256_verify(key.signing_key, signed_parts, tag)
        .map_err(|InvalidMac| TokenOpenError::InvalidMac)?;

    let (iv, ciphertext) = signed_parts.split_at_mut(IV_LEN);
    if ciphertext.len() % BLOCK_LEN != 0 {
        return Err(TokenOpenError::Malformed);
    }

    let plaintext_len = match key.mode {
        AesMode::Aes128 => Decryptor::<Aes128>::new_from_slices(key.encryption_key, iv)
            .expect("TokenKey construction sizes the key halves")
            .decrypt_padded_mut::<Pkcs7>(ciphertext)
            .map_err(|UnpadError| TokenOpenError::InvalidPadding)?
            .len(),
        AesMode::Aes256 => Decryptor::<Aes256>::new_from_slices(key.encryption_key, iv)
            .expect("TokenKey construction sizes the key halves")
            .decrypt_padded_mut::<Pkcs7>(ciphertext)
            .map_err(|UnpadError| TokenOpenError::InvalidPadding)?
            .len(),
    };
    Ok(&ciphertext[..plaintext_len])
}

// Both variants are cipher states on a no_std-reachable path, with no Box to shrink them.
#[allow(clippy::large_enum_variant)]
enum StreamDecryptor {
    Aes128(Decryptor<Aes128>),
    Aes256(Decryptor<Aes256>),
}

impl StreamDecryptor {
    fn decrypt_in_place(&mut self, region: &mut [u8]) {
        let (blocks, tail) = InOutBuf::from(region).into_chunks();
        debug_assert!(tail.is_empty(), "regions arrive whole blocks at a time");
        match self {
            Self::Aes128(decryptor) => decryptor.decrypt_blocks_inout_mut(blocks),
            Self::Aes256(decryptor) => decryptor.decrypt_blocks_inout_mut(blocks),
        }
    }
}

/// [`token_open_in_place`] for a token landing in contiguous spans: [`absorb_to`](Self::absorb_to) authenticates and decrypts each span in place as it grows, and [`finalize`](Self::finalize) verifies the MAC in constant time before inspecting any padding, then names the plaintext.
/// The final ciphertext block waits for that verdict too, so nothing padding-shaped is ever examined ahead of authentication. Callers must not release a decrypted byte before `finalize`.
pub struct TokenOpenStream {
    hmac: HmacSha256Stream,
    decryptor: StreamDecryptor,
    token_len: usize,
    absorbed_byte_len: usize,
}

impl TokenOpenStream {
    /// `token_len` is the whole token's length; the shapes refused are exactly [`token_open_in_place`]'s `Malformed`.
    #[allow(clippy::expect_used)]
    pub fn begin(
        key: &TokenKey,
        iv: &[u8; IV_LEN],
        token_len: usize,
    ) -> Result<Self, TokenOpenError> {
        if token_len < IV_LEN + BLOCK_LEN + MAC_LEN
            || !(token_len - TOKEN_OVERHEAD).is_multiple_of(BLOCK_LEN)
        {
            return Err(TokenOpenError::Malformed);
        }
        let mut hmac = HmacSha256Stream::keyed(key.signing_key);
        hmac.update(iv);
        let decryptor = match key.mode {
            AesMode::Aes128 => StreamDecryptor::Aes128(
                Decryptor::<Aes128>::new_from_slices(key.encryption_key, iv)
                    .expect("TokenKey construction sizes the key halves"),
            ),
            AesMode::Aes256 => StreamDecryptor::Aes256(
                Decryptor::<Aes256>::new_from_slices(key.encryption_key, iv)
                    .expect("TokenKey construction sizes the key halves"),
            ),
        };
        Ok(Self {
            hmac,
            decryptor,
            token_len,
            absorbed_byte_len: IV_LEN,
        })
    }

    /// The whole-block span [`absorb_to`](Self::absorb_to) would process next, in token coordinates: everything past earlier calls up to `contiguous_byte_len`, minus the held-back final block.
    /// Empty when nothing is absorbable yet.
    pub fn pending_span(&self, contiguous_byte_len: usize) -> core::ops::Range<usize> {
        let held_back = self.token_len - MAC_LEN - BLOCK_LEN;
        let through = contiguous_byte_len.min(held_back);
        let whole_blocks_through =
            IV_LEN + (through.saturating_sub(IV_LEN) / BLOCK_LEN) * BLOCK_LEN;
        self.absorbed_byte_len..whole_blocks_through.max(self.absorbed_byte_len)
    }

    /// Whether every span before the held-back final block has been absorbed — all that [`finalize`](Self::finalize) requires.
    pub fn fully_absorbed(&self) -> bool {
        self.absorbed_byte_len == self.token_len - MAC_LEN - BLOCK_LEN
    }

    /// Authenticate then decrypt the token's contiguous prefix past what earlier calls covered, whole blocks at a time, returning the bytes decrypted by this call. `token` is the same buffer every call, filled at least to `contiguous_byte_len`.
    pub fn absorb_to<'t>(&mut self, token: &'t mut [u8], contiguous_byte_len: usize) -> &'t [u8] {
        let span = self.pending_span(contiguous_byte_len);
        let region = &mut token[span];
        self.absorb_span(region);
        region
    }

    /// [`absorb_to`](Self::absorb_to) for a span carried away from its token — exactly the bytes [`pending_span`](Self::pending_span) named, handed as their own slice and decrypted in place there.
    /// The offloading caller owns copying them back where they came from.
    pub fn absorb_span(&mut self, span: &mut [u8]) {
        debug_assert!(
            span.len().is_multiple_of(BLOCK_LEN)
                && self.absorbed_byte_len + span.len() <= self.token_len - MAC_LEN - BLOCK_LEN,
            "a span is whole blocks within the pending region"
        );
        if span.is_empty() {
            return;
        }
        self.hmac.update(span);
        self.decryptor.decrypt_in_place(span);
        self.absorbed_byte_len += span.len();
    }

    /// The MAC is verified in constant time before the held-back final block is decrypted and its padding inspected, preserving [`token_open_in_place`]'s refusal order.
    pub fn finalize(mut self, token: &mut [u8]) -> Result<&[u8], TokenOpenError> {
        let cipher_end = self.token_len - MAC_LEN;
        let last_block = cipher_end - BLOCK_LEN;
        debug_assert_eq!(
            self.absorbed_byte_len, last_block,
            "finalize expects every span before the final block absorbed"
        );
        self.hmac.update(&token[last_block..cipher_end]);
        self.hmac
            .verify(&token[cipher_end..self.token_len])
            .map_err(|InvalidMac| TokenOpenError::InvalidMac)?;
        self.decryptor
            .decrypt_in_place(&mut token[last_block..cipher_end]);
        let unpadded_len = Pkcs7::raw_unpad(&token[last_block..cipher_end])
            .map_err(|UnpadError| TokenOpenError::InvalidPadding)?
            .len();
        Ok(&token[IV_LEN..last_block + unpadded_len])
    }
}

/// Verifies the MAC (constant time) before decrypting.
/// `out` must hold the whole ciphertext (`token.len() - TOKEN_OVERHEAD`); padding is only stripped after the in-place decrypt.
#[allow(clippy::expect_used)]
pub fn token_open(key: &TokenKey, token: &[u8], out: &mut [u8]) -> Result<usize, TokenOpenError> {
    if token.len() < IV_LEN + BLOCK_LEN + MAC_LEN {
        return Err(TokenOpenError::Malformed);
    }
    let (signed_parts, tag) = token.split_at(token.len() - MAC_LEN);
    hmac_sha256_verify(key.signing_key, signed_parts, tag)
        .map_err(|InvalidMac| TokenOpenError::InvalidMac)?;

    let (iv, ciphertext) = signed_parts.split_at(IV_LEN);
    if ciphertext.len() % BLOCK_LEN != 0 {
        return Err(TokenOpenError::Malformed);
    }
    if out.len() < ciphertext.len() {
        return Err(TokenOpenError::BufferTooShort);
    }
    out[..ciphertext.len()].copy_from_slice(ciphertext);

    let plaintext = match key.mode {
        AesMode::Aes128 => Decryptor::<Aes128>::new_from_slices(key.encryption_key, iv)
            .expect("TokenKey construction sizes the key halves")
            .decrypt_padded_mut::<Pkcs7>(&mut out[..ciphertext.len()])
            .map_err(|UnpadError| TokenOpenError::InvalidPadding)?,
        AesMode::Aes256 => Decryptor::<Aes256>::new_from_slices(key.encryption_key, iv)
            .expect("TokenKey construction sizes the key halves")
            .decrypt_padded_mut::<Pkcs7>(&mut out[..ciphertext.len()])
            .map_err(|UnpadError| TokenOpenError::InvalidPadding)?,
    };
    Ok(plaintext.len())
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn aes128_key() -> [u8; 32] {
        core::array::from_fn(|i| i as u8)
    }

    fn aes256_key() -> [u8; 64] {
        core::array::from_fn(|i| (i * 3) as u8)
    }

    fn plaintext(len: usize) -> std::vec::Vec<u8> {
        (0..len).map(|i| (i * 7 + 1) as u8).collect()
    }

    fn sealed(key: &TokenKey, plain: &[u8]) -> std::vec::Vec<u8> {
        let mut out = std::vec![0u8; sealed_len(plain.len())];
        let n = token_seal(key, &[0x5A; IV_LEN], plain, &mut out).unwrap();
        assert_eq!(n, out.len());
        out
    }

    fn begin(key: &TokenKey, token: &[u8]) -> TokenOpenStream {
        let iv = token[..IV_LEN].try_into().unwrap();
        TokenOpenStream::begin(key, &iv, token.len()).unwrap()
    }

    #[test]
    fn ragged_spans_open_to_exactly_the_one_shot_plaintext() {
        let key128 = aes128_key();
        let key256 = aes256_key();
        for (key, plain_len) in [
            (TokenKey::from_aes128(&key128), 5usize),
            (TokenKey::from_aes128(&key128), 64),
            (TokenKey::from_aes256(&key256), 200),
            (TokenKey::from_aes256(&key256), 1_337),
        ] {
            let plain = plaintext(plain_len);
            let mut token = sealed(&key, &plain);
            let mut one_shot = token.clone();

            let mut stream = begin(&key, &token);
            let mut collected = std::vec::Vec::new();
            let mut contiguous = 0usize;
            for step in [1usize, 15, 16, 17, 464, 464, usize::MAX] {
                contiguous = contiguous.saturating_add(step).min(token.len());
                collected.extend_from_slice(stream.absorb_to(&mut token, contiguous));
            }
            assert_eq!(collected, plain[..collected.len()]);
            assert_eq!(stream.finalize(&mut token).unwrap(), &plain[..]);
            assert_eq!(
                token_open_in_place(&key, &mut one_shot).unwrap(),
                &plain[..]
            );
        }
    }

    #[test]
    fn a_repeated_or_backward_contiguous_length_absorbs_nothing() {
        let key_bytes = aes256_key();
        let key = TokenKey::from_aes256(&key_bytes);
        let plain = plaintext(100);
        let mut token = sealed(&key, &plain);
        let mut stream = begin(&key, &token);
        assert_eq!(stream.absorb_to(&mut token, 48).len(), 32);
        assert!(stream.absorb_to(&mut token, 48).is_empty());
        assert!(stream.absorb_to(&mut token, 20).is_empty());
        stream.absorb_to(&mut token, usize::MAX);
        assert_eq!(stream.finalize(&mut token).unwrap(), &plain[..]);
    }

    #[test]
    fn a_tampered_tail_or_body_refuses_with_invalid_mac() {
        let key_bytes = aes256_key();
        let key = TokenKey::from_aes256(&key_bytes);
        let plain = plaintext(100);

        let mut tail_flipped = sealed(&key, &plain);
        *tail_flipped.last_mut().unwrap() ^= 1;
        let mut stream = begin(&key, &tail_flipped);
        stream.absorb_to(&mut tail_flipped, usize::MAX);
        assert_eq!(
            stream.finalize(&mut tail_flipped).unwrap_err(),
            TokenOpenError::InvalidMac,
        );

        let mut body_flipped = sealed(&key, &plain);
        body_flipped[IV_LEN + 3] ^= 1;
        let mut stream = begin(&key, &body_flipped);
        stream.absorb_to(&mut body_flipped, usize::MAX);
        assert_eq!(
            stream.finalize(&mut body_flipped).unwrap_err(),
            TokenOpenError::InvalidMac,
        );
    }

    #[test]
    fn the_shapes_one_shot_open_calls_malformed_are_refused_at_begin() {
        let key_bytes = aes256_key();
        let key = TokenKey::from_aes256(&key_bytes);
        for token_len in [0, 16, 63, 65, 81] {
            assert_eq!(
                TokenOpenStream::begin(&key, &[0x5A; IV_LEN], token_len).err(),
                Some(TokenOpenError::Malformed),
            );
        }
    }

    #[test]
    fn valid_mac_with_broken_padding_refuses_after_the_mac_verdict() {
        use cbc::cipher::block_padding::NoPadding;
        let key_bytes = aes256_key();
        let key = TokenKey::from_aes256(&key_bytes);
        let iv = [0x5A; IV_LEN];
        let mut token = std::vec![0u8; IV_LEN + 32 + MAC_LEN];
        token[..IV_LEN].copy_from_slice(&iv);
        token[IV_LEN..IV_LEN + 32].copy_from_slice(&plaintext(32));
        token[IV_LEN + 31] = 0x00;
        Encryptor::<Aes256>::new_from_slices(key.encryption_key, &iv)
            .unwrap()
            .encrypt_padded_mut::<NoPadding>(&mut token[IV_LEN..IV_LEN + 32], 32)
            .unwrap();
        let mac = hmac_sha256(key.signing_key, &token[..IV_LEN + 32]);
        token[IV_LEN + 32..].copy_from_slice(&mac);

        let mut one_shot = token.clone();
        assert_eq!(
            token_open_in_place(&key, &mut one_shot).unwrap_err(),
            TokenOpenError::InvalidPadding,
        );

        let mut stream = begin(&key, &token);
        stream.absorb_to(&mut token, usize::MAX);
        assert_eq!(
            stream.finalize(&mut token).unwrap_err(),
            TokenOpenError::InvalidPadding,
        );
    }
}
