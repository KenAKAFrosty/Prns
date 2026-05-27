use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::CryptoError;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256 of `message` under `key` (HMAC accepts any key length).
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// Verify `tag` is the HMAC-SHA256 of `message` under `key`, in constant time.
pub fn hmac_sha256_verify(key: &[u8], message: &[u8], tag: &[u8]) -> Result<(), CryptoError> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(message);
    mac.verify_slice(tag).map_err(|_| CryptoError::InvalidMac)
}
