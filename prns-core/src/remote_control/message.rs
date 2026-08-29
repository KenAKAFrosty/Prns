const MESSAGE_HEADER_ENCODED_LEN: usize = 2;
const DESCRIPTION_COUNT_ENCODED_LEN: usize = 1;
const PROTOCOL_ERROR_KIND_ENCODED_LEN: usize = 1;
const PROTOCOL_ERROR_DETAIL_ENCODED_LEN: usize = 1;
const REQUEST_KIND_BITMAP_LEN: usize = 32;

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlProtocolVersion {
        V1 = 1,
    }
}

impl RemoteControlProtocolVersion {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlRequestKind {
        Describe = 0x01,
        AnnounceSelf = 0x02,
    }
}

impl RemoteControlRequestKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }

    #[must_use]
    pub const fn maximum_response_encoded_len(self) -> usize {
        match self {
            Self::Describe => RemoteControlResponse::MAX_ENCODED_LEN,
            Self::AnnounceSelf => MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
                RemoteControlAnnounceSelfOutcome::ENCODED_LEN,
                RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
            )),
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlResponseKind {
        Describe = 0x01,
        AnnounceSelf = 0x02,
        ProtocolError = 0xFF,
    }
}

impl RemoteControlResponseKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlProtocolErrorKind {
        MalformedRequest = 0x01,
        UnsupportedVersion = 0x02,
        UnknownRequestKind = 0x03,
    }
}

impl RemoteControlProtocolErrorKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlAnnounceSelfOutcome {
        Announced = 0x01,
        Unavailable = 0x02,
        Rejected = 0x03,
        WriteFailed = 0x04,
    }
}

impl RemoteControlAnnounceSelfOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    fn from_wire(value: u8) -> Option<Self> {
        enum_from_wire(value, Self::ALL, Self::wire_value)
    }

    #[must_use]
    pub const fn encoded_len(self) -> usize {
        Self::ENCODED_LEN
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlRequest {
    Describe,
    AnnounceSelf,
}

impl RemoteControlRequest {
    pub const MAX_ENCODED_LEN: usize = MESSAGE_HEADER_ENCODED_LEN;

    #[must_use]
    pub const fn kind(self) -> RemoteControlRequestKind {
        match self {
            Self::Describe => RemoteControlRequestKind::Describe,
            Self::AnnounceSelf => RemoteControlRequestKind::AnnounceSelf,
        }
    }

    #[must_use]
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::Describe => MESSAGE_HEADER_ENCODED_LEN,
            Self::AnnounceSelf => MESSAGE_HEADER_ENCODED_LEN,
        }
    }

    #[must_use]
    pub const fn maximum_response_encoded_len(self) -> usize {
        self.kind().maximum_response_encoded_len()
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, RemoteControlRequestParseError> {
        let Some((version, rest)) = bytes.split_first() else {
            return Err(RemoteControlRequestParseError::Truncated);
        };
        let Some((kind, body)) = rest.split_first() else {
            return Err(RemoteControlRequestParseError::Truncated);
        };
        if RemoteControlProtocolVersion::from_wire(*version).is_none() {
            return Err(RemoteControlRequestParseError::UnsupportedVersion { found: *version });
        }
        let Some(kind) = RemoteControlRequestKind::from_wire(*kind) else {
            return Err(RemoteControlRequestParseError::UnknownRequestKind { found: *kind });
        };
        match kind {
            RemoteControlRequestKind::Describe if body.is_empty() => Ok(Self::Describe),
            RemoteControlRequestKind::AnnounceSelf if body.is_empty() => Ok(Self::AnnounceSelf),
            RemoteControlRequestKind::Describe | RemoteControlRequestKind::AnnounceSelf => {
                Err(RemoteControlRequestParseError::Malformed)
            }
        }
    }

    pub fn write_into(self, out: &mut [u8]) -> Result<usize, RemoteControlMessageWriteError> {
        let encoded_len = self.encoded_len();
        let Some([version, kind]) = out.get_mut(..encoded_len) else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        *version = RemoteControlProtocolVersion::V1.wire_value();
        *kind = self.kind().wire_value();
        Ok(encoded_len)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlRequestSet {
    bits: [u8; REQUEST_KIND_BITMAP_LEN],
    len: u8,
}

impl RemoteControlRequestSet {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            bits: [0; REQUEST_KIND_BITMAP_LEN],
            len: 0,
        }
    }

    #[must_use]
    pub fn only(kind: RemoteControlRequestKind) -> Self {
        let mut requests = Self::empty();
        let _inserted = requests.insert(kind);
        requests
    }

    #[must_use]
    pub fn all() -> Self {
        let mut supported = Self::empty();
        for kind in RemoteControlRequestKind::ALL {
            let _inserted = supported.insert(kind);
        }
        supported
    }

    #[must_use]
    pub fn supports(&self, kind: RemoteControlRequestKind) -> bool {
        let (index, mask) = request_kind_position(kind);
        self.bits.get(index).is_some_and(|byte| *byte & mask != 0)
    }

    pub fn insert(&mut self, kind: RemoteControlRequestKind) -> bool {
        let (index, mask) = request_kind_position(kind);
        let Some(byte) = self.bits.get_mut(index) else {
            return false;
        };
        if *byte & mask != 0 {
            return false;
        }
        let Some(len) = self.len.checked_add(1) else {
            return false;
        };
        *byte |= mask;
        self.len = len;
        true
    }

    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = RemoteControlRequestKind> + '_ {
        RemoteControlRequestKind::ALL
            .into_iter()
            .filter(|kind| self.supports(*kind))
    }

    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut intersection = Self::empty();
        for kind in self.iter() {
            if other.supports(kind) {
                let _inserted = intersection.insert(kind);
            }
        }
        intersection
    }
}

impl core::fmt::Debug for RemoteControlRequestSet {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_set().entries(self.iter()).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlDescription {
    available_requests: RemoteControlRequestSet,
}

impl RemoteControlDescription {
    #[must_use]
    pub const fn available_requests(&self) -> &RemoteControlRequestSet {
        &self.available_requests
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlDescriptionError {
    DescribeUnavailable,
}

impl TryFrom<RemoteControlRequestSet> for RemoteControlDescription {
    type Error = RemoteControlDescriptionError;

    fn try_from(available_requests: RemoteControlRequestSet) -> Result<Self, Self::Error> {
        if !available_requests.supports(RemoteControlRequestKind::Describe) {
            return Err(RemoteControlDescriptionError::DescribeUnavailable);
        }
        Ok(Self { available_requests })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlProtocolError {
    MalformedRequest,
    UnsupportedVersion { found: u8 },
    UnknownRequestKind { found: u8 },
}

impl RemoteControlProtocolError {
    const MAX_ENCODED_BODY_LEN: usize =
        PROTOCOL_ERROR_KIND_ENCODED_LEN.saturating_add(PROTOCOL_ERROR_DETAIL_ENCODED_LEN);

    #[must_use]
    pub const fn kind(self) -> RemoteControlProtocolErrorKind {
        match self {
            Self::MalformedRequest => RemoteControlProtocolErrorKind::MalformedRequest,
            Self::UnsupportedVersion { .. } => RemoteControlProtocolErrorKind::UnsupportedVersion,
            Self::UnknownRequestKind { .. } => RemoteControlProtocolErrorKind::UnknownRequestKind,
        }
    }

    const fn encoded_body_len(self) -> usize {
        match self {
            Self::MalformedRequest => PROTOCOL_ERROR_KIND_ENCODED_LEN,
            Self::UnsupportedVersion { .. } | Self::UnknownRequestKind { .. } => {
                Self::MAX_ENCODED_BODY_LEN
            }
        }
    }

    const fn found(self) -> Option<u8> {
        match self {
            Self::MalformedRequest => None,
            Self::UnsupportedVersion { found } | Self::UnknownRequestKind { found } => Some(found),
        }
    }
}

impl From<RemoteControlRequestParseError> for RemoteControlProtocolError {
    fn from(error: RemoteControlRequestParseError) -> Self {
        match error {
            RemoteControlRequestParseError::Truncated
            | RemoteControlRequestParseError::Malformed => Self::MalformedRequest,
            RemoteControlRequestParseError::UnsupportedVersion { found } => {
                Self::UnsupportedVersion { found }
            }
            RemoteControlRequestParseError::UnknownRequestKind { found } => {
                Self::UnknownRequestKind { found }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlResponse {
    Describe(RemoteControlDescription),
    AnnounceSelf(RemoteControlAnnounceSelfOutcome),
    ProtocolError(RemoteControlProtocolError),
}

impl RemoteControlResponse {
    pub const MAX_ENCODED_LEN: usize = MESSAGE_HEADER_ENCODED_LEN.saturating_add(maximum(
        DESCRIPTION_COUNT_ENCODED_LEN.saturating_add(RemoteControlRequestKind::ALL.len()),
        maximum(
            RemoteControlAnnounceSelfOutcome::ENCODED_LEN,
            RemoteControlProtocolError::MAX_ENCODED_BODY_LEN,
        ),
    ));

    #[must_use]
    pub const fn kind(&self) -> RemoteControlResponseKind {
        match self {
            Self::Describe(_) => RemoteControlResponseKind::Describe,
            Self::AnnounceSelf(_) => RemoteControlResponseKind::AnnounceSelf,
            Self::ProtocolError(_) => RemoteControlResponseKind::ProtocolError,
        }
    }

    #[must_use]
    pub fn encoded_len(&self) -> usize {
        let body_len = match self {
            Self::Describe(description) => {
                DESCRIPTION_COUNT_ENCODED_LEN.saturating_add(description.available_requests.len())
            }
            Self::AnnounceSelf(outcome) => outcome.encoded_len(),
            Self::ProtocolError(error) => error.encoded_body_len(),
        };
        MESSAGE_HEADER_ENCODED_LEN.saturating_add(body_len)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, RemoteControlResponseParseError> {
        let Some((version, rest)) = bytes.split_first() else {
            return Err(RemoteControlResponseParseError::Truncated);
        };
        let Some((kind, body)) = rest.split_first() else {
            return Err(RemoteControlResponseParseError::Truncated);
        };
        if RemoteControlProtocolVersion::from_wire(*version).is_none() {
            return Err(RemoteControlResponseParseError::UnsupportedVersion { found: *version });
        }
        let Some(kind) = RemoteControlResponseKind::from_wire(*kind) else {
            return Err(RemoteControlResponseParseError::UnknownResponseKind { found: *kind });
        };
        match kind {
            RemoteControlResponseKind::Describe => parse_description(body).map(Self::Describe),
            RemoteControlResponseKind::AnnounceSelf => {
                parse_announce_self_outcome(body).map(Self::AnnounceSelf)
            }
            RemoteControlResponseKind::ProtocolError => {
                parse_protocol_error(body).map(Self::ProtocolError)
            }
        }
    }

    pub fn write_into(&self, out: &mut [u8]) -> Result<usize, RemoteControlMessageWriteError> {
        let encoded_len = self.encoded_len();
        let Some(target) = out.get_mut(..encoded_len) else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((version, rest)) = target.split_first_mut() else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((kind, body)) = rest.split_first_mut() else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        *version = RemoteControlProtocolVersion::V1.wire_value();
        *kind = self.kind().wire_value();
        match self {
            Self::Describe(description) => write_description(description, body),
            Self::AnnounceSelf(outcome) => write_announce_self_outcome(*outcome, body),
            Self::ProtocolError(error) => write_protocol_error(error, body),
        }
        Ok(encoded_len)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlRequestParseError {
    Truncated,
    UnsupportedVersion { found: u8 },
    UnknownRequestKind { found: u8 },
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlResponseParseError {
    Truncated,
    UnsupportedVersion { found: u8 },
    UnknownResponseKind { found: u8 },
    UnknownAnnounceSelfOutcome { found: u8 },
    UnknownProtocolErrorKind { found: u8 },
    UnknownRequestKind { found: u8 },
    NonCanonicalRequestSet,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlMessageWriteError {
    BufferTooShort,
}

fn request_kind_position(kind: RemoteControlRequestKind) -> (usize, u8) {
    let wire_value = kind.wire_value();
    let index = usize::from(wire_value >> 3);
    let mask = 1u8.wrapping_shl(u32::from(wire_value & 0x07));
    (index, mask)
}

fn enum_from_wire<T: Copy, const N: usize>(
    value: u8,
    variants: [T; N],
    wire_value: impl Fn(T) -> u8,
) -> Option<T> {
    variants
        .into_iter()
        .find(|variant| wire_value(*variant) == value)
}

const fn maximum(left: usize, right: usize) -> usize {
    if left > right {
        left
    } else {
        right
    }
}

fn parse_description(
    body: &[u8],
) -> Result<RemoteControlDescription, RemoteControlResponseParseError> {
    let Some((count, kinds)) = body.split_first() else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    if kinds.len() != usize::from(*count) {
        return Err(RemoteControlResponseParseError::Malformed);
    }
    let mut available_requests = RemoteControlRequestSet::empty();
    let mut previous = None;
    for wire_value in kinds {
        let Some(kind) = RemoteControlRequestKind::from_wire(*wire_value) else {
            return Err(RemoteControlResponseParseError::UnknownRequestKind { found: *wire_value });
        };
        if previous.is_some_and(|value| value >= *wire_value) || !available_requests.insert(kind) {
            return Err(RemoteControlResponseParseError::NonCanonicalRequestSet);
        }
        previous = Some(*wire_value);
    }
    RemoteControlDescription::try_from(available_requests).map_err(
        |RemoteControlDescriptionError::DescribeUnavailable| {
            RemoteControlResponseParseError::Malformed
        },
    )
}

fn parse_announce_self_outcome(
    body: &[u8],
) -> Result<RemoteControlAnnounceSelfOutcome, RemoteControlResponseParseError> {
    let [outcome] = body else {
        return Err(if body.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    RemoteControlAnnounceSelfOutcome::from_wire(*outcome)
        .ok_or(RemoteControlResponseParseError::UnknownAnnounceSelfOutcome { found: *outcome })
}

fn parse_protocol_error(
    body: &[u8],
) -> Result<RemoteControlProtocolError, RemoteControlResponseParseError> {
    let Some((kind, detail)) = body.split_first() else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    let Some(kind) = RemoteControlProtocolErrorKind::from_wire(*kind) else {
        return Err(RemoteControlResponseParseError::UnknownProtocolErrorKind { found: *kind });
    };
    match kind {
        RemoteControlProtocolErrorKind::MalformedRequest if detail.is_empty() => {
            Ok(RemoteControlProtocolError::MalformedRequest)
        }
        RemoteControlProtocolErrorKind::UnsupportedVersion => parse_error_detail(detail)
            .map(|found| RemoteControlProtocolError::UnsupportedVersion { found }),
        RemoteControlProtocolErrorKind::UnknownRequestKind => parse_error_detail(detail)
            .map(|found| RemoteControlProtocolError::UnknownRequestKind { found }),
        RemoteControlProtocolErrorKind::MalformedRequest => {
            Err(RemoteControlResponseParseError::Malformed)
        }
    }
}

fn parse_error_detail(detail: &[u8]) -> Result<u8, RemoteControlResponseParseError> {
    let [found] = detail else {
        return Err(if detail.is_empty() {
            RemoteControlResponseParseError::Truncated
        } else {
            RemoteControlResponseParseError::Malformed
        });
    };
    Ok(*found)
}

fn write_description(description: &RemoteControlDescription, body: &mut [u8]) {
    let Some((count, kinds)) = body.split_first_mut() else {
        return;
    };
    *count = description.available_requests.len;
    for (out, kind) in kinds.iter_mut().zip(description.available_requests.iter()) {
        *out = kind.wire_value();
    }
}

fn write_announce_self_outcome(outcome: RemoteControlAnnounceSelfOutcome, body: &mut [u8]) {
    if let Some(out) = body.first_mut() {
        *out = outcome.wire_value();
    }
}

fn write_protocol_error(error: &RemoteControlProtocolError, body: &mut [u8]) {
    let Some((kind, detail)) = body.split_first_mut() else {
        return;
    };
    *kind = error.kind().wire_value();
    if let (Some(found), Some(out)) = (error.found(), detail.first_mut()) {
        *out = found;
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(4)]
    fn remote_control_request_parse_terminates_for_every_distinct_wire_shape() {
        let bytes: [u8; RemoteControlRequest::MAX_ENCODED_LEN + 1] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let _result = RemoteControlRequest::parse(&bytes[..len]);
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn remote_control_response_parse_terminates_for_every_distinct_wire_shape() {
        let bytes: [u8; RemoteControlResponse::MAX_ENCODED_LEN + 1] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let _result = RemoteControlResponse::parse(&bytes[..len]);
    }

    #[kani::proof]
    #[kani::unwind(4)]
    fn request_set_intersection_preserves_exact_membership() {
        let mut left = RemoteControlRequestSet::empty();
        let mut right = RemoteControlRequestSet::empty();
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::Describe);
        }
        if kani::any() {
            let _inserted = left.insert(RemoteControlRequestKind::AnnounceSelf);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::Describe);
        }
        if kani::any() {
            let _inserted = right.insert(RemoteControlRequestKind::AnnounceSelf);
        }
        let intersection = left.intersection(&right);
        for kind in RemoteControlRequestKind::ALL {
            assert_eq!(
                intersection.supports(kind),
                left.supports(kind) && right.supports(kind)
            );
        }
        assert!(intersection.len() <= left.len());
        assert!(intersection.len() <= right.len());
    }
}
