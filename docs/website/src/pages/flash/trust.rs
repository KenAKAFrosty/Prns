use prns_flash_manifest::minisign_public_key_id;
#[cfg(not(feature = "browser-test-fixture"))]
use prns_flash_manifest::PINNED_MINISIGN_PUBLIC_KEY;

#[cfg(feature = "browser-test-fixture")]
pub(super) const PUBLIC_KEY: &str =
    include_str!("../../../web-flasher/browser/fixtures/signed-candidate/minisign.pub");

#[cfg(not(feature = "browser-test-fixture"))]
pub(super) const PUBLIC_KEY: &str = PINNED_MINISIGN_PUBLIC_KEY;

#[cfg(feature = "browser-test-fixture")]
pub(super) const BROWSER_TEST_MARKER: &str = "PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1";

pub(super) fn key_is_configured() -> bool {
    #[cfg(feature = "browser-test-fixture")]
    {
        minisign_public_key_id(PUBLIC_KEY).is_some()
    }

    #[cfg(not(feature = "browser-test-fixture"))]
    {
        prns_flash_manifest::pinned_key_is_configured()
    }
}

pub(super) fn key_id() -> Option<String> {
    minisign_public_key_id(PUBLIC_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "browser-test-fixture"))]
    #[test]
    fn normal_build_uses_the_repository_pin() {
        assert_eq!(PUBLIC_KEY, PINNED_MINISIGN_PUBLIC_KEY);
    }

    #[cfg(feature = "browser-test-fixture")]
    #[test]
    fn browser_fixture_still_uses_real_minisign_verification() {
        const MANIFEST: &[u8] = include_bytes!(
            "../../../web-flasher/browser/fixtures/signed-candidate/releases/0.2.6/flash-manifest.json"
        );
        const SIGNATURE: &str = include_str!(
            "../../../web-flasher/browser/fixtures/signed-candidate/releases/0.2.6/flash-manifest.json.minisig"
        );

        assert_eq!(key_id().as_deref(), Some("6B62D3410E007120"));
        prns_flash_manifest::verify_minisign(MANIFEST, SIGNATURE, PUBLIC_KEY)
            .expect("the deterministic browser fixture must have a valid Minisign signature");
    }
}
