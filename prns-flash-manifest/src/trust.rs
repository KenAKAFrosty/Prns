use minisign_verify::{PublicKey, Signature};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Repository-pinned release verification key.
pub const PINNED_MINISIGN_PUBLIC_KEY: &str = include_str!("../../release/keys/minisign.pub");

/// Signature verification failure.
#[derive(Debug, Error)]
pub enum TrustError {
    /// Release key custody has not been configured.
    #[error("the PRNS release verification key has not been configured")]
    KeyNotConfigured,
    /// Public key is malformed.
    #[error("the pinned Minisign public key is invalid: {0}")]
    InvalidPublicKey(String),
    /// Signature document is malformed.
    #[error("the Minisign signature is invalid: {0}")]
    InvalidSignatureDocument(String),
    /// Cryptographic verification failed.
    #[error("Minisign verification failed: {0}")]
    Verification(String),
}

/// Whether a real release public key has replaced the custody marker.
pub fn pinned_key_is_configured() -> bool {
    !PINNED_MINISIGN_PUBLIC_KEY.contains("PRNS_RELEASE_KEY_NOT_CONFIGURED")
        && PublicKey::decode(PINNED_MINISIGN_PUBLIC_KEY).is_ok()
}

/// Return the canonical 16-hex-digit key ID declared by a standard Minisign public key.
pub fn minisign_public_key_id(public_key_document: &str) -> Option<String> {
    let value = public_key_document
        .lines()
        .next()?
        .strip_prefix("untrusted comment: minisign public key ")?
        .trim();
    if value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(value.to_ascii_uppercase())
    } else {
        None
    }
}

/// Return the key ID of the repository-pinned public key when custody is configured.
pub fn pinned_key_id() -> Option<String> {
    minisign_public_key_id(PINNED_MINISIGN_PUBLIC_KEY)
}

/// Verify a Minisign signature without accepting legacy, non-prehashed signatures.
pub fn verify_minisign(
    data: &[u8],
    signature_document: &str,
    public_key_document: &str,
) -> Result<(), TrustError> {
    if public_key_document.contains("PRNS_RELEASE_KEY_NOT_CONFIGURED") {
        return Err(TrustError::KeyNotConfigured);
    }
    let public_key = PublicKey::decode(public_key_document)
        .map_err(|error| TrustError::InvalidPublicKey(error.to_string()))?;
    let signature = Signature::decode(signature_document)
        .map_err(|error| TrustError::InvalidSignatureDocument(error.to_string()))?;
    public_key
        .verify(data, &signature, false)
        .map_err(|error| TrustError::Verification(error.to_string()))
}

/// Lowercase SHA-256 digest used by the release manifest.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBLIC_KEY: &str = "untrusted comment: minisign public key 1FB2CA18B2C25E1F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==\n";

    #[test]
    fn valid_signature_verifies() -> Result<(), TrustError> {
        verify_minisign(b"test", TEST_SIGNATURE, TEST_PUBLIC_KEY)
    }

    #[test]
    fn standard_public_key_id_is_extracted() {
        assert_eq!(
            minisign_public_key_id(TEST_PUBLIC_KEY).as_deref(),
            Some("1FB2CA18B2C25E1F")
        );
        assert_eq!(
            minisign_public_key_id("untrusted comment: PRNS_RELEASE_KEY_NOT_CONFIGURED\n"),
            None
        );
    }

    #[test]
    fn tampering_is_rejected() {
        assert!(matches!(
            verify_minisign(b"tampered", TEST_SIGNATURE, TEST_PUBLIC_KEY),
            Err(TrustError::Verification(_))
        ));
    }

    #[test]
    fn wrong_key_and_malformed_signature_are_rejected() {
        let wrong_key = TEST_PUBLIC_KEY.replace("73Y7GFO3", "73Y7GFO2");
        assert!(verify_minisign(b"test", TEST_SIGNATURE, &wrong_key).is_err());
        assert!(matches!(
            verify_minisign(b"test", "not a signature", TEST_PUBLIC_KEY),
            Err(TrustError::InvalidSignatureDocument(_))
        ));
    }

    #[test]
    fn hash_is_lowercase_sha256() {
        assert_eq!(
            sha256_hex(b"test"),
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }
}
