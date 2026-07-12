use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMac;

#[allow(clippy::expect_used)]
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// The tag comparison is constant-time; `==` against a computed tag would leak where it mismatched.
#[allow(clippy::expect_used)]
pub fn hmac_sha256_verify(key: &[u8], message: &[u8], tag: &[u8]) -> Result<(), InvalidMac> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(message);
    mac.verify_slice(tag).map_err(|_| InvalidMac)
}

/// [`hmac_sha256_verify`] for a message arriving in spans; the tag comparison stays constant-time.
pub struct HmacSha256Stream {
    mac: HmacSha256,
}

impl HmacSha256Stream {
    #[allow(clippy::expect_used)]
    pub fn keyed(key: &[u8]) -> Self {
        Self {
            mac: HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any length"),
        }
    }

    pub fn update(&mut self, message_span: &[u8]) {
        self.mac.update(message_span);
    }

    pub fn verify(self, tag: &[u8]) -> Result<(), InvalidMac> {
        self.mac.verify_slice(tag).map_err(|_| InvalidMac)
    }
}
