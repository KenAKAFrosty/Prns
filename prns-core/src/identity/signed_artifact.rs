use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::crypto::{sha256, Ed25519Signature};
use crate::message_pack::{
    decode_owned, encode_owned, MessagePackDecodeLimits, MessagePackOwnedError, MessagePackValue,
};

use super::{IdentityHash, PrivateIdentityMaterial, PublicIdentityMaterial};

pub const SIGNED_ARTIFACT_SIGNATURE_LEN: usize = Ed25519Signature::LEN;

const SIGNED_ARTIFACT_MAXIMUM_LENGTH: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedSignedArtifact {
    pub signer: PublicIdentityMaterial,
    pub metadata: Vec<(String, MessagePackValue)>,
    pub embedded_message: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedArtifactError {
    TooShort,
    TooLarge,
    MessagePack(MessagePackOwnedError),
    InvalidEnvelope,
    UnsupportedHashType,
    InvalidPublicKey,
    InvalidSigner,
    UnexpectedSigner,
    MessageRequired,
    InvalidMessageHash,
    InvalidSignature,
}

pub fn create_signed_artifact(
    signer: &PrivateIdentityMaterial,
    message: &[u8],
    embed_message: bool,
    metadata: &[(String, MessagePackValue)],
) -> Result<Vec<u8>, SignedArtifactError> {
    let metadata_length = metadata
        .iter()
        .filter(|(key, _)| key != "signer" && key != "pubkey")
        .count()
        .checked_add(2)
        .ok_or(SignedArtifactError::TooLarge)?;
    let mut metadata_entries = Vec::with_capacity(metadata_length);
    metadata_entries.push((
        MessagePackValue::String("signer".to_string()),
        MessagePackValue::Binary(signer.identity_hash().as_bytes().to_vec()),
    ));
    metadata_entries.push((
        MessagePackValue::String("pubkey".to_string()),
        MessagePackValue::Binary(signer.public().as_bytes().to_vec()),
    ));
    for (key, value) in metadata {
        if key != "signer" && key != "pubkey" {
            metadata_entries.push((MessagePackValue::String(key.clone()), value.clone()));
        }
    }
    let mut envelope_entries = Vec::with_capacity(if embed_message { 4 } else { 3 });
    envelope_entries.push((
        MessagePackValue::String("hashtype".to_string()),
        MessagePackValue::String("sha256".to_string()),
    ));
    envelope_entries.push((
        MessagePackValue::String("hash".to_string()),
        MessagePackValue::Binary(sha256(message).to_vec()),
    ));
    envelope_entries.push((
        MessagePackValue::String("meta".to_string()),
        MessagePackValue::Map(metadata_entries),
    ));
    if embed_message {
        envelope_entries.push((
            MessagePackValue::String("message".to_string()),
            MessagePackValue::Binary(message.to_vec()),
        ));
    }
    let envelope = encode_owned(&MessagePackValue::Map(envelope_entries))
        .map_err(SignedArtifactError::MessagePack)?;
    let artifact_length = envelope
        .len()
        .checked_add(SIGNED_ARTIFACT_SIGNATURE_LEN)
        .ok_or(SignedArtifactError::TooLarge)?;
    if artifact_length > SIGNED_ARTIFACT_MAXIMUM_LENGTH {
        return Err(SignedArtifactError::TooLarge);
    }
    let signature = signer.sign(&envelope);
    let mut artifact = Vec::with_capacity(artifact_length);
    artifact.extend_from_slice(&signature.0);
    artifact.extend_from_slice(&envelope);
    Ok(artifact)
}

pub fn validate_signed_artifact(
    artifact: &[u8],
    message: Option<&[u8]>,
    required_signer: Option<IdentityHash>,
) -> Result<ValidatedSignedArtifact, SignedArtifactError> {
    if artifact.len() <= SIGNED_ARTIFACT_SIGNATURE_LEN {
        return Err(SignedArtifactError::TooShort);
    }
    if artifact.len() > SIGNED_ARTIFACT_MAXIMUM_LENGTH {
        return Err(SignedArtifactError::TooLarge);
    }
    let (signature, envelope) = artifact.split_at(SIGNED_ARTIFACT_SIGNATURE_LEN);
    let signature: [u8; SIGNED_ARTIFACT_SIGNATURE_LEN] = signature
        .try_into()
        .map_err(|_| SignedArtifactError::TooShort)?;
    let decoded = decode_owned(envelope, MessagePackDecodeLimits::default())
        .map_err(SignedArtifactError::MessagePack)?;
    let entries = map(&decoded)?;
    let hash_type = string(field(entries, "hashtype")?)?;
    if hash_type != "sha256" {
        return Err(SignedArtifactError::UnsupportedHashType);
    }
    let expected_hash = binary(field(entries, "hash")?)?;
    let metadata_entries = map(field(entries, "meta")?)?;
    let signer_hash = binary(field(metadata_entries, "signer")?)?;
    let public_key = binary(field(metadata_entries, "pubkey")?)?;
    let signer = PublicIdentityMaterial::from_slice(public_key)
        .map_err(|_| SignedArtifactError::InvalidPublicKey)?;
    if signer.identity_hash().as_bytes().as_slice() != signer_hash {
        return Err(SignedArtifactError::InvalidSigner);
    }
    if required_signer.is_some_and(|required| required != signer.identity_hash()) {
        return Err(SignedArtifactError::UnexpectedSigner);
    }
    let embedded_message = optional_field(entries, "message")
        .map(binary)
        .transpose()?
        .map(<[u8]>::to_vec);
    let message = message
        .or(embedded_message.as_deref())
        .ok_or(SignedArtifactError::MessageRequired)?;
    if sha256(message).as_slice() != expected_hash {
        return Err(SignedArtifactError::InvalidMessageHash);
    }
    signer
        .verify(envelope, &Ed25519Signature(signature))
        .map_err(|_| SignedArtifactError::InvalidSignature)?;
    let mut metadata = Vec::with_capacity(metadata_entries.len().saturating_sub(2));
    for (key, value) in metadata_entries {
        let MessagePackValue::String(key) = key else {
            return Err(SignedArtifactError::InvalidEnvelope);
        };
        if key != "signer" && key != "pubkey" {
            metadata.push((key.clone(), value.clone()));
        }
    }
    Ok(ValidatedSignedArtifact {
        signer,
        metadata,
        embedded_message,
    })
}

fn map(
    value: &MessagePackValue,
) -> Result<&[(MessagePackValue, MessagePackValue)], SignedArtifactError> {
    match value {
        MessagePackValue::Map(entries) => Ok(entries),
        _ => Err(SignedArtifactError::InvalidEnvelope),
    }
}

fn field<'a>(
    entries: &'a [(MessagePackValue, MessagePackValue)],
    name: &str,
) -> Result<&'a MessagePackValue, SignedArtifactError> {
    optional_field(entries, name).ok_or(SignedArtifactError::InvalidEnvelope)
}

fn optional_field<'a>(
    entries: &'a [(MessagePackValue, MessagePackValue)],
    name: &str,
) -> Option<&'a MessagePackValue> {
    entries.iter().find_map(|(key, value)| match key {
        MessagePackValue::String(key) if key == name => Some(value),
        _ => None,
    })
}

fn string(value: &MessagePackValue) -> Result<&str, SignedArtifactError> {
    match value {
        MessagePackValue::String(value) => Ok(value),
        _ => Err(SignedArtifactError::InvalidEnvelope),
    }
}

fn binary(value: &MessagePackValue) -> Result<&[u8], SignedArtifactError> {
    match value {
        MessagePackValue::Binary(value) => Ok(value),
        _ => Err(SignedArtifactError::InvalidEnvelope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RNS_RSG: &str = "e44d954d391c2393bdd24ebcfb94ba12db4ea2fc6c0f37b34b4072d10657655a521037c83b09f96a894640e7d5d9796022be27f7e177201a2185a298b844e30283a86861736874797065a6736861323536a468617368c4200e231b72dd5437d4095002c7d07b34ff13571911857c83b1281e576cee65fa4ea46d65746182a67369676e6572c4104cd0cc45a7405dbd5cf9b5be1ef92f10a67075626b6579c4400faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737";
    const RNS_RSM: &str = "559d62a364ad9f2968dd9c659434de9094dadd01e810ebb1dcbedcfdc184985e7cd125ed350242da97e9e446a2f59322b938d436c1cd48e6579923cd95f1ff0184a86861736874797065a6736861323536a468617368c42048a2c05ff78d6f7880eff7696a3e4a352131a2b3e3d9c2abc2d8b0aef09e8527a46d65746186a67369676e6572c4104cd0cc45a7405dbd5cf9b5be1ef92f10a67075626b6579c4400faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737a46e616d65a450726e73a776657273696f6e03a47461677392a36f6e65a374776fa6737461626c65c3a76d657373616765c40e6d6573736167652d6f7261636c65";

    fn fixed_private() -> PrivateIdentityMaterial {
        let mut bytes = [0u8; 64];
        bytes[..32].fill(0x22);
        bytes[32..].fill(0x11);
        PrivateIdentityMaterial::from_bytes(bytes)
    }

    fn from_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let digits = core::str::from_utf8(pair).unwrap();
                u8::from_str_radix(digits, 16).unwrap()
            })
            .collect()
    }

    #[test]
    fn creates_the_rns_1_3_8_canonical_rsg() {
        let artifact =
            create_signed_artifact(&fixed_private(), b"artifact-oracle", false, &[]).unwrap();
        assert_eq!(artifact, from_hex(RNS_RSG));
    }

    #[test]
    fn creates_and_validates_the_rns_1_3_8_canonical_rsm() {
        let metadata = vec![
            (
                "name".to_string(),
                MessagePackValue::String("Prns".to_string()),
            ),
            ("version".to_string(), MessagePackValue::Unsigned(3)),
            (
                "tags".to_string(),
                MessagePackValue::Array(vec![
                    MessagePackValue::String("one".to_string()),
                    MessagePackValue::String("two".to_string()),
                ]),
            ),
            ("stable".to_string(), MessagePackValue::Boolean(true)),
        ];
        let artifact =
            create_signed_artifact(&fixed_private(), b"message-oracle", true, &metadata).unwrap();
        assert_eq!(artifact, from_hex(RNS_RSM));
        let validated = validate_signed_artifact(&artifact, None, None).unwrap();
        assert_eq!(
            validated.embedded_message.as_deref(),
            Some(b"message-oracle".as_slice())
        );
        assert_eq!(validated.metadata, metadata);
    }

    #[test]
    fn validates_an_rns_artifact_and_rejects_wrong_message_or_signer() {
        let artifact = from_hex(RNS_RSG);
        let validated =
            validate_signed_artifact(&artifact, Some(b"artifact-oracle"), None).unwrap();
        assert_eq!(
            validated.signer.identity_hash(),
            fixed_private().identity_hash()
        );
        assert_eq!(
            validate_signed_artifact(&artifact, Some(b"wrong"), None),
            Err(SignedArtifactError::InvalidMessageHash)
        );
        assert_eq!(
            validate_signed_artifact(
                &artifact,
                Some(b"artifact-oracle"),
                Some(IdentityHash::new([0x55; 16]))
            ),
            Err(SignedArtifactError::UnexpectedSigner)
        );
    }
}
