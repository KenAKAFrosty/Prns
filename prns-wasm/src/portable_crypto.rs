use personal_rns::crypto::{
    x25519_diffie_hellman, Ed25519PublicKey, Ed25519Signature, Ed25519Verifier, X25519PublicKey,
    X25519SecretKey,
};
use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
enum PortableCryptoInputError {
    Ed25519PublicKey,
    Ed25519Signature,
    X25519Secret,
    X25519PublicKey,
}

impl PortableCryptoInputError {
    fn message(&self) -> &'static str {
        match self {
            Self::Ed25519PublicKey => "Ed25519 public key must be exactly 32 bytes",
            Self::Ed25519Signature => "Ed25519 signature must be exactly 64 bytes",
            Self::X25519Secret => "X25519 secret scalar must be exactly 32 bytes",
            Self::X25519PublicKey => "X25519 public key must be exactly 32 bytes",
        }
    }
}

#[wasm_bindgen(js_name = portableEd25519Verify)]
pub fn portable_ed25519_verify(
    public_key: Vec<u8>,
    message: Vec<u8>,
    signature: Vec<u8>,
) -> Result<bool, JsValue> {
    verify_ed25519(public_key, &message, signature)
        .map_err(|error| JsValue::from_str(error.message()))
}

#[wasm_bindgen(js_name = portableLinkProofVerify)]
pub fn portable_link_proof_verify(
    public_key: Vec<u8>,
    message: Vec<u8>,
    signature: Vec<u8>,
    secret_scalar: Vec<u8>,
    peer_public_key: Vec<u8>,
) -> Result<Option<Vec<u8>>, JsValue> {
    link_proof_verify(
        public_key,
        &message,
        signature,
        secret_scalar,
        peer_public_key,
    )
    .map_err(|error| JsValue::from_str(error.message()))
}

fn verify_ed25519(
    public_key: Vec<u8>,
    message: &[u8],
    signature: Vec<u8>,
) -> Result<bool, PortableCryptoInputError> {
    let public_key: [u8; Ed25519PublicKey::LEN] = public_key
        .try_into()
        .map_err(|_| PortableCryptoInputError::Ed25519PublicKey)?;
    let signature: [u8; Ed25519Signature::LEN] = signature
        .try_into()
        .map_err(|_| PortableCryptoInputError::Ed25519Signature)?;
    let Ok(verifier) = Ed25519Verifier::new(&Ed25519PublicKey(public_key)) else {
        return Ok(false);
    };
    Ok(verifier
        .verify(message, &Ed25519Signature(signature))
        .is_ok())
}

fn link_proof_verify(
    public_key: Vec<u8>,
    message: &[u8],
    signature: Vec<u8>,
    secret_scalar: Vec<u8>,
    peer_public_key: Vec<u8>,
) -> Result<Option<Vec<u8>>, PortableCryptoInputError> {
    let secret_scalar = Zeroizing::new(secret_scalar);
    let peer_public_key: [u8; X25519PublicKey::LEN] = peer_public_key
        .try_into()
        .map_err(|_| PortableCryptoInputError::X25519PublicKey)?;
    if !verify_ed25519(public_key, message, signature)? {
        return Ok(None);
    }
    let secret_scalar: [u8; X25519SecretKey::LEN] = secret_scalar
        .as_slice()
        .try_into()
        .map_err(|_| PortableCryptoInputError::X25519Secret)?;
    let shared = x25519_diffie_hellman(
        &X25519SecretKey::new(secret_scalar),
        &X25519PublicKey(peer_public_key),
    );
    Ok(Some(shared.as_bytes().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::{link_proof_verify, verify_ed25519};
    use personal_rns::crypto::{ed25519_public_key, ed25519_sign, Ed25519SecretKey};

    #[test]
    fn portable_protocol_crypto_matches_native_vectors() {
        let signing_secret = Ed25519SecretKey::new([0x11; 32]);
        let signing_public = ed25519_public_key(&signing_secret);
        let message = b"sign-this";
        let signature = ed25519_sign(&signing_secret, message);
        assert_eq!(
            verify_ed25519(signing_public.0.to_vec(), message, signature.0.to_vec()),
            Ok(true),
        );
        assert_eq!(
            verify_ed25519(
                signing_public.0.to_vec(),
                b"sign-thus",
                signature.0.to_vec(),
            ),
            Ok(false),
        );
        assert_eq!(
            link_proof_verify(
                signing_public.0.to_vec(),
                message,
                signature.0.to_vec(),
                vec![0x22; 32],
                vec![
                    0x7b, 0x0d, 0x47, 0xd9, 0x34, 0x27, 0xf8, 0x31, 0x11, 0x60, 0x78, 0x1c, 0x7c,
                    0x73, 0x3f, 0xd8, 0x9f, 0x88, 0x97, 0x0a, 0xef, 0x49, 0x0d, 0x8a, 0xa0, 0xee,
                    0x19, 0xa4, 0xcb, 0x8a, 0x1b, 0x14,
                ],
            ),
            Ok(Some(vec![
                0x1f, 0xdc, 0x19, 0x2f, 0xaa, 0x02, 0x12, 0xa9, 0xaa, 0xe7, 0xbb, 0x4f, 0x41, 0xb5,
                0x80, 0x22, 0x7f, 0xd5, 0xad, 0x3e, 0x5d, 0x77, 0x7f, 0xaa, 0xe2, 0x30, 0xdf, 0xe9,
                0x73, 0xf3, 0xe8, 0x05,
            ])),
        );
    }
}
