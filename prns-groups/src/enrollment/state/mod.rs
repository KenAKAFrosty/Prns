//! Native enrollment facts in the existing MLS application key space. Only the
//! current state is loaded on adapter entry; historical rows are point reads.
//! The wrapper carries the backend's exact local limits, not a second selection.
use super::*;
use prns_groups_store::AtomicBackend;
use prns_groups_store_redb::RedbLimits;
use std::ops::{Deref, DerefMut};

mod authors;
pub(crate) use authors::historical_author;
use authors::Facts;
mod inspect;
mod projection;
mod rows;
pub(super) use inspect::{Inspection, InspectionContext};
mod stage;
pub(super) use projection::{visit_mutations, Projection};

const VERSION: u8 = 3;
const PREFIX: &[u8] = b"enrollment/facts/v3/";
const CONTROL: u8 = 1;
const CONTROL_HEAD: u8 = 2;
const CONTROL_PARENT: u8 = 3;
const ADDED: u8 = 4;
const ADDED_EPOCH: u8 = 5;
const ADDED_MEMBER: u8 = 6;
const ADDED_ORDER: u8 = 7;
const TERMINAL: u8 = 8;
const TERMINAL_SUBJECT: u8 = 9;
const TERMINAL_ORDER: u8 = 10;
const ACK: u8 = 11;
const ACK_ORDER: u8 = 12;
const REQUEST: u8 = 13;
const REQUEST_ORDER: u8 = 14;
const PAST_MEMBER: u8 = 15;
const PAST_MEMBER_ORDER: u8 = 16;
const CONTROL_ORDER: u8 = 17;

/// The one persisted header. Collections here are only the current/required
/// signed roster and one pending KeyPackage, each bounded by the selected row.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Header {
    version: u8,
    pub current: Option<Signed>,
    pub required_tip: Option<Signed>,
    #[serde(with = "optional_package")]
    pub package: Option<Vec<u8>>,
    pub lifecycle: GroupLifecycle,
    pub control_count: u64,
    pub added_count: u64,
    pub added_member_count: u64,
    pub terminal_count: u64,
    pub terminal_subject_count: u64,
    pub ack_count: u64,
    pub management_count: u64,
    pub past_member_count: u64,
}
impl Default for Header {
    fn default() -> Self {
        Self {
            version: VERSION,
            current: None,
            required_tip: None,
            package: None,
            lifecycle: GroupLifecycle::Active,
            control_count: 0,
            added_count: 0,
            added_member_count: 0,
            terminal_count: 0,
            terminal_subject_count: 0,
            ack_count: 0,
            management_count: 0,
            past_member_count: 0,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored<T> {
    ordinal: u64,
    value: T,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PastMember {
    member: Member,
    removed_epoch: u64,
}

#[derive(Clone)]
pub(super) struct Snapshot {
    header: Header,
    limits: RedbLimits,
}
impl Deref for Snapshot {
    type Target = Header;
    fn deref(&self) -> &Header {
        &self.header
    }
}
impl DerefMut for Snapshot {
    fn deref_mut(&mut self) -> &mut Header {
        &mut self.header
    }
}
impl Snapshot {
    pub(super) fn limits(&self) -> RedbLimits {
        self.limits
    }
    pub(super) fn ready(&self) -> Result<bool, EnrollmentError> {
        self.header.ready()
    }
    fn decode(bytes: Option<&[u8]>, limits: RedbLimits) -> Result<Self, EnrollmentError> {
        let header = match bytes {
            Some(bytes)
                if u64::try_from(bytes.len()).map_err(|_| EnrollmentError::Capacity)?
                    <= limits.max_value_bytes =>
            {
                serde_json::from_slice(bytes).map_err(|_| EnrollmentError::Corrupt)?
            }
            Some(_) => return Err(EnrollmentError::Capacity),
            None => Header::default(),
        };
        let result = Self { header, limits };
        result.validate_counts()?;
        Ok(result)
    }
    fn value_limit(&self) -> Result<usize, EnrollmentError> {
        usize::try_from(self.limits.max_value_bytes).map_err(|_| EnrollmentError::Capacity)
    }
    fn validate_counts(&self) -> Result<(), EnrollmentError> {
        if self.version != VERSION {
            return Err(EnrollmentError::UnsupportedVersion);
        }
        if self.added_member_count > self.added_count
            || self.terminal_subject_count > self.terminal_count
        {
            return Err(EnrollmentError::Corrupt);
        }
        let count = [
            (self.control_count, 4),
            (self.added_count, 3),
            (self.added_member_count, 1),
            (self.terminal_count, 2),
            (self.terminal_subject_count, 1),
            (self.ack_count, 2),
            (self.management_count, 2),
            (self.past_member_count, 2),
        ]
        .into_iter()
        .try_fold(1u64, |total, (n, width)| {
            n.checked_mul(width)
                .and_then(|n| total.checked_add(n))
                .ok_or(EnrollmentError::Corrupt)
        })?;
        if count > self.limits.max_live_objects {
            return Err(EnrollmentError::Corrupt);
        }
        Ok(())
    }
}
impl Header {
    fn ready(&self) -> Result<bool, EnrollmentError> {
        let Some(current) = &self.current else {
            return Ok(false);
        };
        let Some(tip) = &self.required_tip else {
            return Ok(true);
        };
        if current.manifest.epoch < tip.manifest.epoch {
            return Ok(false);
        }
        if current.manifest.epoch == tip.manifest.epoch && current.head()? != tip.head()? {
            return Err(EnrollmentError::Authentication);
        }
        Ok(true)
    }
}
pub(super) fn load_backend(
    backend: &mut RedbBackend<'_>,
    limits: RedbLimits,
) -> Result<Snapshot, EnrollmentError> {
    let key =
        prns_groups_mls::ApplicationKey::new(STATE_KEY).map_err(|_| EnrollmentError::Capacity)?;
    let bytes = backend.read(
        key.bytes(),
        usize::try_from(limits.max_value_bytes).map_err(|_| EnrollmentError::Capacity)?,
    )?;
    Snapshot::decode(bytes.as_deref(), limits)
}
pub(super) fn load(
    adapter: &Adapter<RedbBackend<'_>>,
    limits: RedbLimits,
) -> Result<Snapshot, EnrollmentError> {
    let bytes = adapter
        .application_read(STATE_KEY)
        .map_err(|_| EnrollmentError::Corrupt)?;
    Snapshot::decode(bytes.as_ref().map(|b| b.as_slice()), limits)
}
fn key(kind: u8, id: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(PREFIX.len() + 1 + id.len());
    key.extend_from_slice(PREFIX);
    key.push(kind);
    key.extend_from_slice(id);
    key
}
fn member_key(member: &Member) -> [u8; 40] {
    member_key_parts(member.endpoint, member.incarnation)
}
fn member_key_parts(endpoint: [u8; 32], incarnation: u64) -> [u8; 40] {
    let mut key = [0; 40];
    key[..32].copy_from_slice(&endpoint);
    key[32..].copy_from_slice(&incarnation.to_be_bytes());
    key
}
fn subject(kind: &TerminalKind) -> Vec<u8> {
    match kind {
        TerminalKind::Closed => vec![0],
        TerminalKind::Removed(member) => {
            let mut key = vec![1];
            key.extend_from_slice(&member_key(member));
            key
        }
    }
}
fn read<T: serde::de::DeserializeOwned>(
    facts: &impl Facts,
    key: &[u8],
) -> Result<Option<T>, EnrollmentError> {
    #[cfg(test)]
    READS.with(|n| n.set(n.get() + 1));
    facts.application_value(key, |bytes| {
        bytes
            .map(|bytes| serde_json::from_slice(bytes).map_err(|_| EnrollmentError::Corrupt))
            .transpose()
    })
}
fn required<T>(value: Option<T>) -> Result<T, EnrollmentError> {
    value.ok_or(EnrollmentError::Corrupt)
}

#[cfg(test)]
std::thread_local! { static READS: std::cell::Cell<usize> = const {std::cell::Cell::new(0)}; }
#[cfg(test)]
#[path = "../state_tests.rs"]
mod tests;

// Reuse the canonical byte codec; an optional pending package must not expand
// into a decimal byte array while the network profile was sized for base64.
mod optional_package {
    use super::*;
    #[derive(Serialize)]
    struct Borrowed<'a>(#[serde(with = "model::wire_bytes")] &'a [u8]);
    #[derive(Deserialize)]
    struct Owned(#[serde(with = "model::wire_bytes")] Vec<u8>);
    pub(super) fn serialize<S: serde::Serializer>(
        value: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_deref().map(Borrowed).serialize(serializer)
    }
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<Owned>::deserialize(deserializer).map(|value| value.map(|owned| owned.0))
    }
}

/// A retained descriptor cannot reuse the smaller roster's reserve after a
/// membership transition. This is a point header read on the same exclusive lane.
pub(crate) fn check_provisioned(
    adapter: &Adapter<RedbBackend<'_>>,
    cfg: &Provisioned,
) -> Result<(), EnrollmentError> {
    let state = load(adapter, cfg.store_limits)?;
    let current = state.current.as_ref().ok_or(EnrollmentError::NotJoined)?;
    let lifecycle_matches = state.lifecycle == cfg.lifecycle
        || (state.lifecycle == GroupLifecycle::Active && cfg.lifecycle == GroupLifecycle::Leaving);
    if current.manifest.group != cfg.group.0
        || Some(IdentityHash::new(current.manifest.owner)) != cfg.owner_identity
        || current.head()? != cfg.control.head.0
        || cfg.control.era.0 != cfg.control.head
        || !lifecycle_matches
        || current.manifest.members.len() != cfg.readers.len()
    {
        return Err(EnrollmentError::StaleMembership);
    }
    for (member, reader) in current.manifest.members.iter().zip(&cfg.readers) {
        if member.endpoint()? != *reader {
            return Err(EnrollmentError::StaleMembership);
        }
    }
    Ok(())
}
