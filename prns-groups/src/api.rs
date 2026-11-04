//! One immutable mutation input, independent of its execution clock or caller.
pub use prns_groups_core::enrollment::InvitationId;
use prns_groups_core::{
    ClockReading, ControlReference, Endpoint, GroupId, NamespaceId, RecordId, SchemaId,
};
use prns_host::IdentityHash;

/// Exact retained bytes. Fresh IDs must satisfy RequestToken's retry policy;
/// opaque older IDs may only recover an already retained result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RequestId(pub [u8; 32]);
impl From<crate::RequestToken> for RequestId {
    fn from(token: crate::RequestToken) -> Self {
        Self(token.into_bytes())
    }
}

/// Receipt obligations, never a confidentiality filter: all MLS readers decrypt.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupTargets"))]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Targets {
    /// Strictly sorted, unique endpoints.
    Explicit(Vec<Endpoint>),
    /// Resolve once under original_scope. Retries retain the original targets.
    OtherMembers,
}

/// Generic immutable content. Its Debug output deliberately omits the body.
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupPublicationIntent"))]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PublicationIntent {
    pub group: GroupId,
    pub namespace: NamespaceId,
    pub schema: SchemaId,
    pub targets: Targets,
    pub body: Vec<u8>,
}
impl core::fmt::Debug for PublicationIntent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PublicationIntent")
            .field("namespace", &self.namespace)
            .field("schema", &self.schema)
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}
impl PublicationIntent {
    /// Encode the optional text profile. This generic intent can execute or
    /// recover in a build without the text feature.
    #[cfg(feature = "text")]
    pub fn text(group: GroupId, text: &str) -> Result<Self, crate::Error> {
        let text = prns_group_text::Text::new(text).map_err(|_| crate::Error::InvalidInput)?;
        let mut buffer = [0; prns_group_text::MAX_ENCODED_BYTES];
        let body = text
            .encode(&mut buffer)
            .map_err(|_| crate::Error::InvalidInput)?
            .to_vec();
        Ok(Self {
            group,
            namespace: prns_group_text::NAMESPACE,
            schema: prns_group_text::SCHEMA,
            targets: Targets::OtherMembers,
            body,
        })
    }
}

/// Encode the selected text profile into the ordinary generic publication input.
/// This performs no admission or persistence; callers submit the returned intent
/// with their usual request ID and recovery policy.
#[cfg(feature = "text")]
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn text_publication(
    group: GroupId,
    text: String,
) -> Result<PublicationIntent, crate::ApiError> {
    PublicationIntent::text(group, &text).map_err(crate::ApiError::from)
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupIntent"))]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Intent {
    Publish(PublicationIntent),
    Create {
        title: String,
        profile: crate::GroupProfile,
    },
    Invite {
        group: GroupId,
        #[serde(with = "identity_hash")]
        recipient: IdentityHash,
    },
    Respond {
        invitation: InvitationId,
        accept: bool,
    },
    CancelInvitation {
        invitation: InvitationId,
    },
    DismissInvitation {
        invitation: InvitationId,
    },
    RemoveMember {
        group: GroupId,
        member: Endpoint,
    },
    Leave {
        group: GroupId,
    },
    Close {
        group: GroupId,
    },
    DeleteLocal {
        group: GroupId,
    },
}
impl Intent {
    pub(crate) fn validate(&self) -> Result<(), crate::Error> {
        if let Intent::Create { title, profile } = self {
            if title.trim().is_empty() || title.len() > crate::profile::MAX_TITLE_BYTES {
                return Err(crate::Error::InvalidInput);
            }
            profile.validate().map_err(crate::Error::Api)?;
        }
        if let Intent::Publish(value) = self {
            if let Targets::Explicit(targets) = &value.targets {
                if targets.windows(2).any(|p| p[0] >= p[1]) {
                    return Err(crate::Error::InvalidInput);
                }
                if targets.is_empty() {
                    return Err(crate::Error::NoRecipients);
                }
            }
        }
        Ok(())
    }
    pub fn group(&self) -> Option<GroupId> {
        match self {
            Self::Publish(value) => Some(value.group),
            Self::Invite { group, .. }
            | Self::RemoveMember { group, .. }
            | Self::Leave { group }
            | Self::Close { group }
            | Self::DeleteLocal { group } => Some(*group),
            _ => None,
        }
    }
}

/// Captured local authorization context. The store incarnation selects one
/// membership for recovery even after leaving and rejoining. An unjoined
/// membership has no signed control yet, but still fences local deletion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupOriginalScope"))]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum OriginalScope {
    Control {
        control: ControlReference,
        incarnation: prns_groups_store::StoreIncarnation,
    },
    Unjoined(prns_groups_store::StoreIncarnation),
}
impl OriginalScope {
    pub(crate) fn control(self) -> Option<ControlReference> {
        match self {
            Self::Control { control, .. } => Some(control),
            Self::Unjoined(_) => None,
        }
    }
    pub(crate) fn incarnation(self) -> prns_groups_store::StoreIncarnation {
        match self {
            Self::Control { incarnation, .. } | Self::Unjoined(incarnation) => incarnation,
        }
    }
}

/// Persist unchanged after an ambiguous result. Group-scoped mutations require
/// original_scope; Create and stable invitation-ID actions require None.
/// Exact retained results recover before current control/time admission checks.
/// Publications bind body, schema, namespace, original control and resolved
/// receipt targets: OtherMembers and matching Explicit targets are equivalent.
/// IDs must be unique across a caller's requests. Independent direct recovery
/// checks the existing domain receipt scope. Catalog-authorized native execution
/// retains the complete Request and requires exact equality, including selectors.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupRequest"))]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub id: RequestId,
    pub intent: Intent,
    pub original_scope: Option<OriginalScope>,
}
impl Request {
    pub(crate) fn validate(&self) -> Result<(), crate::Error> {
        let valid_scope = match (&self.intent, self.original_scope) {
            (Intent::DeleteLocal { .. }, Some(_)) => true,
            (intent, Some(OriginalScope::Control { .. })) if intent.group().is_some() => true,
            (intent, None) if intent.group().is_none() => true,
            _ => false,
        };
        if !valid_scope {
            return Err(crate::Error::InvalidInput);
        }
        self.intent.validate()?;
        Ok(())
    }
}

/// Trusted platform execution evidence, never peer time or immutable intent.
/// Unknown time permits exact retained recovery but cannot admit new work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupExecutionContext"))]
pub struct ExecutionContext {
    pub clock: ClockReading,
}

/// Local committed outcome, not remote delivery or enrollment completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupOutcome"))]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Outcome {
    Published(RecordId),
    Created(GroupId),
    Invited(InvitationId),
    Saved,
}

// Preserve the canonical cross-domain identity type at this persistence edge.
mod identity_hash {
    use prns_host::IdentityHash;
    pub fn serialize<S: serde::Serializer>(
        identity: &IdentityHash,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(identity.as_bytes(), serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<IdentityHash, D::Error> {
        <[u8; 16] as serde::Deserialize>::deserialize(deserializer).map(IdentityHash::new)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    #[test]
    fn encoded_text_is_a_generic_request_without_a_text_runtime() {
        let now = prns_groups_core::Timestamp(1_800_000_000);
        let identities = [IdentityHash::new([1; 16]), IdentityHash::new([2; 16])];
        let fixtures = crate::fixture::generate("opaque profile", &identities, now).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = redb::Database::create(dir.path().join("generic")).unwrap();
        let cfg = crate::fixture::load_and_install(&db, &fixtures[0], identities[0]).unwrap();
        // Public text wire fixture; the generic executor never loads a decoder.
        let publication = PublicationIntent {
            group: cfg.group,
            namespace: NamespaceId(0x7465_7874),
            schema: SchemaId(1),
            targets: Targets::OtherMembers,
            body: b"GTX\x01\x00\x02hi".to_vec(),
        };
        #[cfg(feature = "text")]
        assert_eq!(
            publication,
            PublicationIntent::text(cfg.group, "hi").unwrap()
        );
        let request = Request {
            id: RequestId::from(crate::RequestToken::new(now.0, [1; 20]).unwrap()),
            intent: Intent::Publish(publication),
            original_scope: Some(OriginalScope::Control {
                control: cfg.control,
                incarnation: cfg.incarnation,
            }),
        };
        let mut session = crate::StoreSession::open(&db, cfg.clone()).unwrap();
        let context = ExecutionContext {
            clock: ClockReading::Trusted(prns_groups_core::ClockInterval::new(now, now).unwrap()),
        };
        let first = session.execute_request(request.clone(), context).unwrap();
        assert!(matches!(first, Outcome::Published(_)));
        drop(session);
        let mut session = crate::StoreSession::open(&db, cfg).unwrap();
        assert_eq!(
            session
                .execute_request(
                    request.clone(),
                    ExecutionContext {
                        clock: ClockReading::Unknown
                    }
                )
                .unwrap(),
            first
        );
        // A control reference can be shared by independent local membership
        // stores. It cannot authorize recovery or publication in another one.
        let mut wrong_membership = request.clone();
        let Some(OriginalScope::Control {
            control,
            incarnation,
        }) = request.original_scope
        else {
            panic!("joined scope")
        };
        let mut other = *incarnation.as_bytes();
        other[0] ^= 1;
        wrong_membership.original_scope = Some(OriginalScope::Control {
            control,
            incarnation: prns_groups_store::StoreIncarnation::new(other).unwrap(),
        });
        assert!(matches!(
            session.execute_request(wrong_membership, context),
            Err(crate::Error::Enrollment(
                crate::enrollment::EnrollmentError::StaleMembership
            ))
        ));
        assert_eq!(
            session
                .execute_request(
                    request.clone(),
                    ExecutionContext {
                        clock: ClockReading::Unknown
                    }
                )
                .unwrap(),
            first
        );
        let mut changed = request;
        let Some(OriginalScope::Control {
            mut control,
            incarnation,
        }) = changed.original_scope
        else {
            panic!("control")
        };
        control.era.0 .0[0] ^= 1;
        changed.original_scope = Some(OriginalScope::Control {
            control,
            incarnation,
        });
        assert!(matches!(
            session.execute_request(changed, context),
            Err(crate::Error::IdempotencyConflict)
        ));
    }
}
