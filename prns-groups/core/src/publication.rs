use crate::{Endpoint, GroupId, PublicationValidity};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RecordId(pub [u8; 32]);

/// Identity of one exact encrypted representation, distinct from its record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvelopeId(pub [u8; 32]);

/// An application namespace within a group; applications own its assignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NamespaceId(pub u32);

/// An application schema within its namespace; this is not a wire version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SchemaId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ControlId(pub [u8; 32]);

/// The control barrier that began an authorization era, not an MLS epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AuthorizationEra(pub ControlId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupControlReference"))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ControlReference {
    pub head: ControlId,
    pub era: AuthorizationEra,
}

/// Original publication facts, preserved through retries and re-encryption.
///
/// A service must atomically bind these fields, the body commitment and the
/// original target list to one record ID. This value supplies neither signature
/// verification nor authorization; those checks precede durable admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupPublicationMetadata"))]
pub struct PublicationMetadata {
    pub group: GroupId,
    pub record: RecordId,
    pub author: Endpoint,
    pub namespace: NamespaceId,
    pub schema: SchemaId,
    pub validity: PublicationValidity,
    pub original_control: ControlReference,
}
