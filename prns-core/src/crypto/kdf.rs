use hkdf::Hkdf;
use sha2::Sha256;

/// HKDF-SHA256 (RFC 5869); `salt`/`info` map to RNS's `salt`/`context`.
#[allow(clippy::expect_used)]
pub fn hkdf_sha256<const N: usize>(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; N] {
    const { assert!(N <= 255 * 32) };
    let mut out = [0u8; N];
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, &mut out)
        .expect("HKDF output length is within RFC 5869 bounds");
    out
}

/// The requested output exceeds HKDF-SHA256's 255 * 32 byte ceiling (RFC 5869).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HkdfOutputTooLong;

/// RNS masks derive a stream as long as the packet, so the length is runtime-sized.
pub fn hkdf_sha256_into(
    ikm: &[u8],
    salt: &[u8],
    info: &[u8],
    out: &mut [u8],
) -> Result<(), HkdfOutputTooLong> {
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, out)
        .map_err(|hkdf::InvalidLength| HkdfOutputTooLong)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_output_beyond_the_rfc_5869_ceiling_is_refused() {
        let mut out = [0u8; 255 * 32 + 1];
        assert_eq!(
            hkdf_sha256_into(b"ikm", b"salt", &[], &mut out),
            Err(HkdfOutputTooLong)
        );
    }
}
