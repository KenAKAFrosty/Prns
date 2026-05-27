//! RNS's Fernet-style token: `iv ‖ AES-CBC(PKCS7(plaintext)) ‖ HMAC-SHA256`.
//! Encrypt-then-MAC; the key splits into a signing half and an encryption half
//! (16+16 for a 32-byte key → AES-128, 32+32 for a 64-byte key → AES-256).

use aes::{Aes128, Aes256};
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use cbc::{Decryptor, Encryptor};

use super::{hmac_sha256, hmac_sha256_verify, CryptoError};

const IV_LEN: usize = 16;
const MAC_LEN: usize = 32;
const BLOCK_LEN: usize = 16;

#[derive(Clone, Copy)]
enum AesMode {
    Aes128,
    Aes256,
}

/// A token key split into its signing and encryption halves, as RNS does.
pub struct TokenKey<'a> {
    signing_key: &'a [u8],
    encryption_key: &'a [u8],
    mode: AesMode,
}

impl<'a> TokenKey<'a> {
    /// Split a derived key (32 → AES-128, 64 → AES-256) into its halves.
    pub fn from_derived(key: &'a [u8]) -> Result<Self, CryptoError> {
        match key.len() {
            32 => Ok(Self {
                signing_key: &key[..16],
                encryption_key: &key[16..],
                mode: AesMode::Aes128,
            }),
            64 => Ok(Self {
                signing_key: &key[..32],
                encryption_key: &key[32..],
                mode: AesMode::Aes256,
            }),
            _ => Err(CryptoError::BadKeyLength),
        }
    }
}

/// Seal `plaintext` into a token written to `out`, with the host-supplied `iv`.
/// Returns the token length.
pub fn token_seal(
    key: &TokenKey,
    iv: &[u8; IV_LEN],
    plaintext: &[u8],
    out: &mut [u8],
) -> Result<usize, CryptoError> {
    // PKCS#7 always adds 1..=BLOCK_LEN bytes, so this rounds strictly up.
    let padded_len = (plaintext.len() / BLOCK_LEN + 1) * BLOCK_LEN;
    let total = IV_LEN + padded_len + MAC_LEN;
    if out.len() < total {
        return Err(CryptoError::BufferTooShort);
    }

    out[..IV_LEN].copy_from_slice(iv);
    let cipher_region = &mut out[IV_LEN..IV_LEN + padded_len];
    cipher_region[..plaintext.len()].copy_from_slice(plaintext);
    match key.mode {
        AesMode::Aes128 => Encryptor::<Aes128>::new_from_slices(key.encryption_key, iv)
            .map_err(|_| CryptoError::BadKeyLength)?
            .encrypt_padded_mut::<Pkcs7>(cipher_region, plaintext.len())
            .map_err(|_| CryptoError::BufferTooShort)?,
        AesMode::Aes256 => Encryptor::<Aes256>::new_from_slices(key.encryption_key, iv)
            .map_err(|_| CryptoError::BadKeyLength)?
            .encrypt_padded_mut::<Pkcs7>(cipher_region, plaintext.len())
            .map_err(|_| CryptoError::BufferTooShort)?,
    };

    let mac = hmac_sha256(key.signing_key, &out[..IV_LEN + padded_len]);
    out[IV_LEN + padded_len..total].copy_from_slice(&mac);
    Ok(total)
}

/// Open `token`, writing the plaintext to `out`. Verifies the MAC (constant
/// time) before decrypting. Returns the plaintext length.
pub fn token_open(key: &TokenKey, token: &[u8], out: &mut [u8]) -> Result<usize, CryptoError> {
    if token.len() < IV_LEN + BLOCK_LEN + MAC_LEN {
        return Err(CryptoError::MalformedToken);
    }
    let (signed_parts, tag) = token.split_at(token.len() - MAC_LEN);
    hmac_sha256_verify(key.signing_key, signed_parts, tag)?;

    let (iv, ciphertext) = signed_parts.split_at(IV_LEN);
    if ciphertext.len() % BLOCK_LEN != 0 {
        return Err(CryptoError::MalformedToken);
    }
    if out.len() < ciphertext.len() {
        return Err(CryptoError::BufferTooShort);
    }
    out[..ciphertext.len()].copy_from_slice(ciphertext);

    let plaintext = match key.mode {
        AesMode::Aes128 => Decryptor::<Aes128>::new_from_slices(key.encryption_key, iv)
            .map_err(|_| CryptoError::BadKeyLength)?
            .decrypt_padded_mut::<Pkcs7>(&mut out[..ciphertext.len()])
            .map_err(|_| CryptoError::InvalidPadding)?,
        AesMode::Aes256 => Decryptor::<Aes256>::new_from_slices(key.encryption_key, iv)
            .map_err(|_| CryptoError::BadKeyLength)?
            .decrypt_padded_mut::<Pkcs7>(&mut out[..ciphertext.len()])
            .map_err(|_| CryptoError::InvalidPadding)?,
    };
    Ok(plaintext.len())
}
