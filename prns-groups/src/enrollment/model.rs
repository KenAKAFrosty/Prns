use super::*;

mod bounds;
pub(crate) use bounds::{required_frame_bytes, required_frame_bytes_for_members};

pub const MAX_FRAME_BYTES: usize = crate::GroupProfile::DEFAULT.max_enrollment_frame_bytes as usize;
pub const INVITATION_LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;
pub(super) const WIRE_VERSION: u8 = 3;
pub(super) const STATE_KEY: &[u8] = b"enrollment/state/v2";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Enum))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupInvitationState"))]
pub enum InvitationState {
    Pending,
    JoinRequested,
    Joined,
    Declined,
    Expired,
    Cancelled,
    /// The authenticated recipient explicitly cannot support the offered profile.
    UnsupportedProfile,
}
#[derive(Clone, Debug)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupInvitation"))]
pub struct Invitation {
    pub id: crate::InvitationId,
    pub group: GroupId,
    pub title: String,
    /// The immutable authenticated profile offered by the group owner.
    pub profile: crate::GroupProfile,
    pub inviter: IdentityHash,
    pub recipient: IdentityHash,
    pub expires_at_secs: u64,
    pub outgoing: bool,
    pub state: InvitationState,
    /// The accepted join is retrying while the owner finishes its current
    /// authored deliveries. This does not assert that every member is drained.
    pub waiting_for_delivery: bool,
    pub can_cancel: bool,
    pub can_dismiss: bool,
}
#[derive(Clone, Debug)]
pub struct Work {
    pub id: [u8; 32],
    pub recipient: IdentityHash,
    pub frame: Vec<u8>,
}

/// A live, exclusive page over exact enrollment work identities. A short or
/// empty page can still have a continuation because skipped candidates consume
/// the selected visit budget. Restart from None after the traversal completes.
#[derive(Clone, Debug)]
pub struct WorkPage {
    pub items: Vec<Work>,
    pub next: Option<[u8; 32]>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Member {
    pub rns: [u8; 16],
    pub endpoint: [u8; 32],
    pub incarnation: u64,
    pub key: [u8; 32],
}
impl Member {
    pub fn binding(&self) -> MemberBinding {
        MemberBinding {
            endpoint: self.endpoint,
            incarnation: self.incarnation,
            signing_key: self.key,
        }
    }
    pub fn endpoint(&self) -> Result<Endpoint, EnrollmentError> {
        Ok(Endpoint {
            id: EndpointId(self.endpoint),
            incarnation: EndpointIncarnation::new(self.incarnation)
                .map_err(|_| EnrollmentError::Invalid)?,
        })
    }
    pub fn new(rns: IdentityHash, binding: MemberBinding) -> Self {
        Self {
            rns: rns.into_bytes(),
            endpoint: binding.endpoint,
            incarnation: binding.incarnation,
            key: binding.signing_key,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub version: u8,
    pub group: [u8; 32],
    pub owner: [u8; 16],
    pub title: String,
    pub profile: crate::GroupProfile,
    pub parent: [u8; 32],
    pub revision: u64,
    pub epoch: u64,
    pub commit_digest: [u8; 32],
    pub kind: ControlKind,
    pub members: Vec<Member>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Signed {
    pub manifest: Manifest,
    #[serde(with = "wire_bytes")]
    pub signature: Vec<u8>,
}
impl Signed {
    pub fn head(&self) -> Result<[u8; 32], EnrollmentError> {
        Ok(hash(&encode(self)?))
    }
    pub fn verify(
        &self,
        group: [u8; 32],
        owner: &Member,
        title: &str,
        profile: crate::GroupProfile,
    ) -> Result<(), EnrollmentError> {
        let m = &self.manifest;
        if m.version != WIRE_VERSION
            || m.group != group
            || m.owner != owner.rns
            || m.title != title
            || m.profile != profile
            || m.members.is_empty()
            || m.members.len()
                > usize::try_from(profile.max_members).map_err(|_| EnrollmentError::Invalid)?
            || m.revision != m.epoch
            || !m.members.contains(owner)
            || self.signature.len() != 64
            || m.members.windows(2).any(|p| p[0].endpoint >= p[1].endpoint)
            || m.members.iter().enumerate().any(|(i, x)| {
                x.incarnation == 0
                    || m.members[..i]
                        .iter()
                        .any(|y| x.rns == y.rns || x.key == y.key)
            })
        {
            return Err(EnrollmentError::Authentication);
        }
        owner
            .binding()
            .verify_enrollment(&encode(m)?, &self.signature)
            .map_err(|_| EnrollmentError::Authentication)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum ControlKind {
    Create,
    Add,
    Remove(Member),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Control {
    pub signed: Signed,
    #[serde(with = "wire_bytes")]
    pub commit: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Added {
    pub member: Member,
    pub invitation: [u8; 32],
    pub request_hash: [u8; 32],
    pub signed: Signed,
    #[serde(with = "wire_bytes")]
    pub welcome: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub group: [u8; 32],
    pub title: String,
    pub profile: crate::GroupProfile,
    pub owner: Member,
    pub local: Member,
    pub secret: Vec<u8>,
    pub namespace: [u8; 16],
    pub incarnation: [u8; 24],
    pub created: u64,
    pub generation: u32,
    pub invitation: Option<[u8; 32]>,
}
impl Drop for Entry {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.secret.zeroize();
    }
}
impl Entry {
    pub fn identity(&self) -> Result<LocalIdentity, EnrollmentError> {
        LocalIdentity::from_parts(self.local.binding(), self.secret.clone().into())
            .map_err(|_| EnrollmentError::Corrupt)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Offer {
    pub id: [u8; 32],
    pub group: [u8; 32],
    pub title: String,
    pub profile: crate::GroupProfile,
    pub owner: Member,
    pub recipient: [u8; 16],
    pub created: u64,
    pub expires: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Invite {
    /// Local first-admission sequence, assigned by the catalog writer. This is
    /// never part of Offer or another authenticated protocol frame.
    pub admission_revision: u64,
    pub offer: Offer,
    pub state: InvitationState,
    pub acknowledged: bool,
    #[serde(default)]
    pub terminal_at: Option<u64>,
    /// Bind a deferred join to its exact member and KeyPackage across restarts.
    #[serde(default)]
    pub deferred_join: Option<[u8; 32]>,
    pub dismissed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub id: [u8; 32],
    pub fingerprint: [u8; 32],
    pub result: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Frame {
    Leave(Departure),
    Terminal(Terminal),
    Cancel {
        invitation: [u8; 32],
        group: [u8; 32],
        expires: u64,
    },
    Offer(Offer),
    Join {
        invitation: [u8; 32],
        profile: [u8; 32],
        member: Member,
        #[serde(with = "wire_bytes")]
        package: Vec<u8>,
    },
    Joined {
        invitation: [u8; 32],
        head: [u8; 32],
    },
    Decline {
        invitation: [u8; 32],
    },
    Sync {
        member: Member,
        group: [u8; 32],
        head: [u8; 32],
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Reply {
    Ack,
    Expired,
    Cancelled,
    UnsupportedProfile,
    Terminal(Terminal),
    AwaitingDelivery,
    Welcome { added: Added, tip: Box<Signed> },
    Control(Option<Control>),
}

pub(super) fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
pub(super) fn profile_digest(profile: crate::GroupProfile) -> Result<[u8; 32], EnrollmentError> {
    Ok(hash(&encode(&("prns.groups.profile.v1", profile))?))
}
pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, EnrollmentError> {
    serde_json::to_vec(value).map_err(|_| EnrollmentError::Corrupt)
}
#[cfg(test)]
pub(super) fn frame<T: Serialize>(value: &T) -> Result<Vec<u8>, EnrollmentError> {
    frame_with_limit(value, MAX_FRAME_BYTES)
}
pub(super) fn frame_with_limit<T: Serialize>(
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, EnrollmentError> {
    encode_with_limit(
        &Wire {
            version: WIRE_VERSION,
            payload: value,
        },
        limit,
    )
}

/// Shared bounded encoder for an enrollment frame or one selected catalog/
/// namespace row. Callers supply that representation's own byte budget.
pub(super) fn encode_with_limit<T: Serialize>(
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, EnrollmentError> {
    let mut output = FrameWriter {
        bytes: Vec::new(),
        limit,
        refused: false,
    };
    let result = serde_json::to_writer(&mut output, value);
    if result.is_err() {
        return Err(if output.refused {
            EnrollmentError::Capacity
        } else {
            EnrollmentError::Corrupt
        });
    }
    Ok(output.bytes)
}
#[cfg(test)]
pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, EnrollmentError> {
    decode_with_limit(bytes, MAX_FRAME_BYTES)
}
pub(super) fn decode_with_limit<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    limit: usize,
) -> Result<T, EnrollmentError> {
    if bytes.len() > limit {
        return Err(EnrollmentError::Capacity);
    }
    require_version(bytes)?;
    let wire: Wire<T> = serde_json::from_slice(bytes).map_err(|_| EnrollmentError::Invalid)?;
    Ok(wire.payload)
}

/// Stop encoding at the selected ingress bound rather than allocating the
/// complete oversize JSON and rejecting it afterward. Growth is geometric but
/// never requests more than this operation's explicit byte budget.
struct FrameWriter {
    bytes: Vec<u8>,
    limit: usize,
    refused: bool,
}
impl std::io::Write for FrameWriter {
    fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
        let Some(length) = self
            .bytes
            .len()
            .checked_add(value.len())
            .filter(|n| *n <= self.limit)
        else {
            self.refused = true;
            return Err(std::io::Error::other(
                "enrollment frame exceeds selected limit",
            ));
        };
        if length > self.bytes.capacity() {
            let capacity = length.max(
                self.bytes
                    .capacity()
                    .saturating_mul(2)
                    .max(256)
                    .min(self.limit),
            );
            if self
                .bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .is_err()
            {
                self.refused = true;
                return Err(std::io::Error::other("enrollment frame allocation failed"));
            }
        }
        self.bytes.extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn random<const N: usize>() -> Result<[u8; N], EnrollmentError> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|_| EnrollmentError::Entropy)?;
    Ok(bytes)
}
pub(super) fn work_id(domain: &[u8], id: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"prns enrollment work v1");
    h.update(domain);
    h.update(id);
    h.finalize().into()
}
pub(super) fn require_time(offer: &Offer, now: u64) -> Result<(), EnrollmentError> {
    prns_groups_core::enrollment::validate_invitation_time(
        Timestamp(offer.created),
        Timestamp(offer.expires),
        Timestamp(now),
        Timestamp(now),
        INVITATION_LIFETIME_SECS,
    )
    .map_err(|_| EnrollmentError::Expired)
}

// Only public signatures and opaque MLS messages use this codec. The complete
// frame is bounded before decoding, and its authenticated group profile bounds
// MLS fields before use. This layer adds no second fixed payload ceiling.
pub(super) mod wire_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&base64::display::Base64Display::new(bytes, &STANDARD))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| serde::de::Error::custom("invalid enrollment byte field"))?;
        Ok(bytes)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire<T> {
    version: u8,
    payload: T,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Terminal {
    pub statement: TerminalStatement,
    #[serde(with = "wire_bytes")]
    pub signature: Vec<u8>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TerminalStatement {
    pub version: u8,
    pub group: [u8; 32],
    pub title: String,
    pub owner: Member,
    pub head: [u8; 32],
    pub revision: u64,
    pub kind: TerminalKind,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum TerminalKind {
    Removed(Member),
    Closed,
}
impl Terminal {
    pub fn verify(&self, group: [u8; 32], owner: &Member) -> Result<(), EnrollmentError> {
        if self.statement.version != WIRE_VERSION
            || self.statement.group != group
            || &self.statement.owner != owner
            || self.signature.len() != 64
        {
            return Err(EnrollmentError::Authentication);
        }
        owner
            .binding()
            .verify_enrollment(&encode(&self.statement)?, &self.signature)
            .map_err(|_| EnrollmentError::Authentication)
    }
    pub fn id(&self) -> Result<[u8; 32], EnrollmentError> {
        Ok(hash(&encode(self)?))
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Departure {
    pub id: [u8; 32],
    pub group: [u8; 32],
    pub owner: Member,
    pub member: Member,
    pub known_head: [u8; 32],
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Notification {
    pub proof: Terminal,
    pub recipient: [u8; 16],
    pub acknowledged: bool,
}
pub(super) fn require_version(bytes: &[u8]) -> Result<(), EnrollmentError> {
    #[derive(Deserialize)]
    struct Version {
        version: u8,
    }
    let header: Version =
        serde_json::from_slice(bytes).map_err(|_| EnrollmentError::UnsupportedVersion)?;
    if header.version != WIRE_VERSION {
        return Err(EnrollmentError::UnsupportedVersion);
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cancellation {
    pub invitation: [u8; 32],
    pub group: [u8; 32],
    pub owner: [u8; 16],
    pub expires: u64,
}
