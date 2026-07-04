//! RNS's Fernet-style token: `iv ‖ AES-CBC(PKCS7(plaintext)) ‖ HMAC-SHA256`.
//! Encrypt-then-MAC; the key splits into a signing half and an encryption half (16+16 for a 32-byte key → AES-128, 32+32 for a 64-byte key → AES-256).

use aes::{Aes128, Aes256};
use cbc::cipher::block_padding::{Pkcs7, UnpadError};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use cbc::{Decryptor, Encryptor};

use super::mac::InvalidMac;
use super::{hmac_sha256, hmac_sha256_verify};

const IV_LEN: usize = 16;
const MAC_LEN: usize = 32;
const BLOCK_LEN: usize = 16;

/// RNS 1.3.5 `Identity.TOKEN_OVERHEAD`: the 16-byte IV and 32-byte HMAC around every sealed payload.
pub const TOKEN_OVERHEAD: usize = IV_LEN + MAC_LEN;

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
    // PKCS#7 always adds 1..=BLOCK_LEN bytes, so this rounds strictly up.
    let padded_len = (plain_len / BLOCK_LEN + 1) * BLOCK_LEN;
    let total = IV_LEN + padded_len + MAC_LEN;
    if out.len() < total {
        return Err(BufferTooShort);
    }

    out[..IV_LEN].copy_from_slice(iv);
    let cipher_region = &mut out[IV_LEN..IV_LEN + padded_len];
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

    let mac = hmac_sha256(key.signing_key, &out[..IV_LEN + padded_len]);
    out[IV_LEN + padded_len..total].copy_from_slice(&mac);
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
