use hkdf::Hkdf;
use sha2::Sha256;

/// HKDF-SHA256 (RFC 5869); `salt`/`info` map to RNS's `salt`/`context`. `N` stays
/// far under HKDF's 255*32 ceiling, so derivation cannot fail.
#[allow(clippy::expect_used)]
pub fn hkdf_sha256<const N: usize>(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, &mut out)
        .expect("HKDF output length is within RFC 5869 bounds");
    out
}

/// RNS masks derive a stream as long as the packet, so the length is runtime-sized.
#[allow(clippy::expect_used)]
pub fn hkdf_sha256_into(ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) {
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, out)
        .expect("HKDF output length is within RFC 5869 bounds");
}
