//! Owned public claim projection for allocator-equipped consumers.
//!
//! Verification uses the same bounded codec as the borrowed API. These public
//! fields carry no authority and never replace live peer authentication or the
//! application's explicit service/identity acceptance policy.

use alloc::{string::String, vec::Vec};

use crate::CardError;

/// Public fields returned after canonical framing and signature verification.
/// The wire codec bounds every field; constructing this record yourself is not
/// evidence of verification and grants no authority.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
pub struct ContactCardClaims {
    pub public_identity: Vec<u8>,
    pub service_name_hash: Vec<u8>,
    pub display_name: String,
    pub identity_hash: Vec<u8>,
    pub destination_hash: Vec<u8>,
}

/// Verify canonical public bytes before allocating their bounded projection.
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn verify_contact_card(wire: Vec<u8>) -> Result<ContactCardClaims, CardError> {
    let verified = crate::verify(&wire)?;
    Ok(ContactCardClaims {
        public_identity: verified.public_identity().to_vec(),
        service_name_hash: verified.service_name_hash().to_vec(),
        display_name: String::from(verified.display_name()),
        identity_hash: verified.identity_hash().to_vec(),
        destination_hash: verified.destination_hash().to_vec(),
    })
}

/// Derive the canonical service hash without an application-specific registry.
#[must_use]
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn service_name_hash(dotted_name: String) -> Vec<u8> {
    crate::service_name_hash(&dotted_name).to_vec()
}

/// Encode the exact bounded URI carrier. This does not verify the signature.
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn encode_contact_card_uri(wire: Vec<u8>) -> Result<String, crate::TokenError> {
    let mut output = [0; crate::MAX_URI_BYTES];
    let length = crate::encode_uri(&wire, &mut output)?;
    // The canonical encoder emits ASCII; keep conversion checked at this boundary.
    String::from_utf8(output[..length].to_vec()).map_err(|_| crate::TokenError::InvalidCharacter)
}

/// Decode the exact URI without URL normalization. Verify the returned public
/// bytes with `verify_contact_card` before accepting their service/identity claim.
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn decode_contact_card_uri(uri: String) -> Result<Vec<u8>, crate::TokenError> {
    let mut output = [0; crate::MAX_CARD_BYTES];
    let length = crate::decode_uri(&uri, &mut output)?;
    Ok(output[..length].to_vec())
}
