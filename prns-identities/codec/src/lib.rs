//! Explicit, bounded persistence encoding for identity catalogs.
//!
//! This is a local storage format, not a Reticulum packet or contact card. All
//! integers are big-endian; enum/option tags and lengths have explicit encodings.
//! The storage adapter supplies integrity, atomicity and rollback protection.
#![no_std]
#![forbid(unsafe_code)]

use prns_identities_core::*;

const MAGIC: [u8; 8] = *b"PRNSIDC1";
const VERSION: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    BufferTooSmall,
    Truncated,
    InvalidEncoding,
    UnsupportedVersion,
    CapacityMismatch,
    TrailingBytes,
    InvalidCatalog(Fault),
}

/// Conservative upper bound for allocating/placing an encoding buffer. `None`
/// means the dimensions or size cannot be represented on this target.
pub const fn max_encoded_len<const I: usize, const O: usize, const S: usize>() -> Option<usize> {
    if I > u32::MAX as usize || O > u32::MAX as usize || S > u32::MAX as usize {
        return None;
    }
    let Some(services) = S.checked_mul(32) else {
        return None;
    };
    let Some(record) = 512usize.checked_add(services) else {
        return None;
    };
    let Some(slots) = I.checked_add(O) else {
        return None;
    };
    let Some(records) = record.checked_mul(slots) else {
        return None;
    };
    records.checked_add(128)
}

/// Encode a validated catalog into caller memory. Output is usable only on
/// success; an undersized output may contain a partial encoding.
pub fn encode<const I: usize, const O: usize, const S: usize>(
    catalog: &Catalog<I, O, S>,
    output: &mut [u8],
) -> Result<usize, Error> {
    validate_catalog(catalog).map_err(Error::InvalidCatalog)?;
    let mut w = Writer {
        bytes: output,
        at: 0,
    };
    MAGIC.encode(&mut w)?;
    VERSION.encode(&mut w)?;
    for capacity in [I, O, S] {
        u32::try_from(capacity)
            .map_err(|_| Error::CapacityMismatch)?
            .encode(&mut w)?;
    }
    catalog.format.encode(&mut w)?;
    catalog.incarnation.encode(&mut w)?;
    catalog.revision.encode(&mut w)?;
    catalog.session.encode(&mut w)?;
    catalog.issued_sequence.encode(&mut w)?;
    catalog.completed_floor.encode(&mut w)?;
    for identity in &catalog.identities {
        identity.encode(&mut w)?;
    }
    for operation in &catalog.operations {
        operation.encode(&mut w)?;
    }
    Ok(w.at)
}

/// Decode directly into caller memory, then validate all lifecycle invariants.
/// On any error `into` is unusable and must not authorize effects. No replacement
/// catalog is constructed or returned on the stack. Rejects trailing bytes and
/// mismatched compile-time capacities rather than silently dropping records.
pub fn decode<const I: usize, const O: usize, const S: usize>(
    input: &[u8],
    into: &mut Catalog<I, O, S>,
) -> Result<(), Error> {
    // Invalidate even when framing fails before touching another field.
    into.format = 0;
    let mut r = Reader {
        bytes: input,
        at: 0,
    };
    if <[u8; 8]>::decode(&mut r)? != MAGIC {
        return Err(Error::InvalidEncoding);
    }
    if u16::decode(&mut r)? != VERSION {
        return Err(Error::UnsupportedVersion);
    }
    for expected in [I, O, S] {
        let capacity = u32::decode(&mut r)?;
        if usize::try_from(capacity).ok() != Some(expected) {
            return Err(Error::CapacityMismatch);
        }
    }
    let format = u16::decode(&mut r)?;
    into.incarnation = Codec::decode(&mut r)?;
    into.revision = Codec::decode(&mut r)?;
    into.session = Codec::decode(&mut r)?;
    into.issued_sequence = Codec::decode(&mut r)?;
    into.completed_floor = Codec::decode(&mut r)?;
    for identity in &mut into.identities {
        *identity = Codec::decode(&mut r)?;
    }
    for operation in &mut into.operations {
        *operation = Codec::decode(&mut r)?;
    }
    if r.at != input.len() {
        return Err(Error::TrailingBytes);
    }
    into.format = format;
    if let Err(error) = validate_catalog(into) {
        into.format = 0;
        return Err(Error::InvalidCatalog(error));
    }
    Ok(())
}

/// Bound for a standalone canonical creation intent, including all service slots.
pub const fn max_create_request_encoded_len<const S: usize>() -> Option<usize> {
    match S.checked_mul(5) {
        Some(services) => services.checked_add(256),
        None => None,
    }
}

/// Encode the same creation datatype used by the portable catalog. Native
/// envelopes use this instead of maintaining a second intent encoding.
pub fn encode_create_request<const S: usize>(
    request: &CreateRequest<S>,
    output: &mut [u8],
) -> Result<usize, Error> {
    validate_create_request(request).map_err(Error::InvalidCatalog)?;
    encode_value(request, output)
}

pub fn decode_create_request<const S: usize>(input: &[u8]) -> Result<CreateRequest<S>, Error> {
    let request = decode_value(input)?;
    validate_create_request(&request).map_err(Error::InvalidCatalog)?;
    Ok(request)
}

/// Standalone result encoding shares the catalog's tags and canonical values.
pub fn encode_operation_result(
    result: &OperationResult,
    output: &mut [u8],
) -> Result<usize, Error> {
    encode_value(result, output)
}

pub fn decode_operation_result(input: &[u8]) -> Result<OperationResult, Error> {
    decode_value(input)
}

fn encode_value<T: Codec>(value: &T, output: &mut [u8]) -> Result<usize, Error> {
    let mut writer = Writer {
        bytes: output,
        at: 0,
    };
    value.encode(&mut writer)?;
    Ok(writer.at)
}

fn decode_value<T: Codec>(input: &[u8]) -> Result<T, Error> {
    let mut reader = Reader {
        bytes: input,
        at: 0,
    };
    let value = T::decode(&mut reader)?;
    if reader.at != input.len() {
        return Err(Error::TrailingBytes);
    }
    Ok(value)
}

struct Writer<'a> {
    bytes: &'a mut [u8],
    at: usize,
}
impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let end = self
            .at
            .checked_add(bytes.len())
            .ok_or(Error::BufferTooSmall)?;
        self.bytes
            .get_mut(self.at..end)
            .ok_or(Error::BufferTooSmall)?
            .copy_from_slice(bytes);
        self.at = end;
        Ok(())
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(length).ok_or(Error::Truncated)?;
        let result = self.bytes.get(self.at..end).ok_or(Error::Truncated)?;
        self.at = end;
        Ok(result)
    }
}
trait Codec: Sized {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error>;
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error>;
}
impl<const N: usize> Codec for [u8; N] {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        w.put(self)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        r.take(N)?.try_into().map_err(|_| Error::Truncated)
    }
}
macro_rules! integer {
    ($($t:ty),*) => {$ (
        impl Codec for $t {
            fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> { w.put(&self.to_be_bytes()) }
            fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
                Ok(Self::from_be_bytes(Codec::decode(r)?))
            }
        }
    )*};
}
integer!(u8, u16, u32, u64);
impl Codec for bool {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        u8::from(*self).encode(w)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::InvalidEncoding),
        }
    }
}
impl<T: Codec> Codec for Option<T> {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        match self {
            None => 0u8.encode(w),
            Some(v) => {
                1u8.encode(w)?;
                v.encode(w)
            }
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(None),
            1 => Ok(Some(T::decode(r)?)),
            _ => Err(Error::InvalidEncoding),
        }
    }
}
fn encode_entries<T: Codec, const N: usize>(
    values: &[Option<T>; N],
    w: &mut Writer<'_>,
) -> Result<(), Error> {
    for value in values {
        value.encode(w)?;
    }
    Ok(())
}
fn decode_entries<T: Codec + Copy, const N: usize>(
    r: &mut Reader<'_>,
) -> Result<[Option<T>; N], Error> {
    let mut values = [None; N];
    for value in &mut values {
        *value = Codec::decode(r)?;
    }
    Ok(values)
}
macro_rules! product {
    ($t:ty, $($field:ident),+ $(,)?) => {
        impl Codec for $t {
            fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> { $( self.$field.encode(w)?; )+ Ok(()) }
            fn decode(r: &mut Reader<'_>) -> Result<Self, Error> { Ok(Self { $( $field: Codec::decode(r)?, )+ }) }
        }
    };
}
macro_rules! generic_product {
    ($t:ident, $($field:ident),+ $(,)?) => {
        impl<const S: usize> Codec for $t<S> {
            fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> { $( self.$field.encode(w)?; )+ Ok(()) }
            fn decode(r: &mut Reader<'_>) -> Result<Self, Error> { Ok(Self { $( $field: Codec::decode(r)?, )+ }) }
        }
    };
}
macro_rules! newtype {
    ($t:ident) => {
        impl Codec for $t {
            fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
                self.0.encode(w)
            }
            fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
                Ok(Self(Codec::decode(r)?))
            }
        }
    };
}
macro_rules! tagged {
    ($t:ty, $($tag:literal => $variant:ident),+ $(,)?) => {
        impl Codec for $t {
            fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
                let tag: u8 = match self { $( Self::$variant => $tag, )+ }; tag.encode(w)
            }
            fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
                match u8::decode(r)? { $( $tag => Ok(Self::$variant), )+ _ => Err(Error::InvalidEncoding) }
            }
        }
    };
}
newtype!(KeyReference);
newtype!(PublicIdentityHash);
newtype!(CommandDigest);
newtype!(ServiceId);
product!(LocalIdentityId, incarnation, sequence);
product!(OperationToken, incarnation, sequence, digest);
product!(Metadata, local_label, published_name);
product!(
    ServiceDependency,
    service,
    revision,
    pending_messages,
    unsettled_control,
    authority_count,
    recovery_pending,
    quiescence_supported
);
product!(IdentityGuard, id, public_hash, lifecycle_revision);
product!(PendingTransition, operation, target, phase);
generic_product!(PreparedTransition, identity, target, review);
generic_product!(CreateRequest, source, metadata, services);
generic_product!(OperationRecord, token, command, state, unpublished_key);
generic_product!(
    IdentityRecord,
    id,
    public_hash,
    key_reference,
    metadata,
    metadata_revision,
    lifecycle_revision,
    desired,
    initial_services,
    services,
    pending,
    recovery_cleanup
);
tagged!(DesiredState, 0 => Enabled, 1 => Paused, 2 => Retired);
impl Codec for TransitionTarget {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        match self {
            Self::Enable => 0u8.encode(w),
            Self::Pause => 1u8.encode(w),
            Self::Retire => 2u8.encode(w),
            Self::AddService(service) => {
                3u8.encode(w)?;
                service.encode(w)
            }
            Self::RemoveService(service) => {
                4u8.encode(w)?;
                service.encode(w)
            }
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(Self::Enable),
            1 => Ok(Self::Pause),
            2 => Ok(Self::Retire),
            3 => Ok(Self::AddService(Codec::decode(r)?)),
            4 => Ok(Self::RemoveService(Codec::decode(r)?)),
            _ => Err(Error::InvalidEncoding),
        }
    }
}
tagged!(RetirementBlock, 0 => ChangedDependencies, 1 => IncompleteReport, 2 => UnqualifiedService, 3 => RecoveryPending, 4 => AuthorityOwned, 5 => UnsettledControl, 6 => PendingMessages);

impl<const N: usize> Codec for BoundedText<N> {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        let text = self.as_str().as_bytes();
        u16::try_from(text.len())
            .map_err(|_| Error::InvalidEncoding)?
            .encode(w)?;
        w.put(text)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        let length = usize::from(u16::decode(r)?);
        if length > N {
            return Err(Error::InvalidEncoding);
        }
        let text = core::str::from_utf8(r.take(length)?).map_err(|_| Error::InvalidEncoding)?;
        Self::new(text).map_err(|_| Error::InvalidEncoding)
    }
}
impl<const S: usize> Codec for ServiceSet<S> {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        encode_entries(&self.entries, w)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            entries: decode_entries(r)?,
        })
    }
}
impl<const S: usize> Codec for DependencyReport<S> {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        encode_entries(&self.entries, w)?;
        self.complete.encode(w)?;
        self.other_key_uses.encode(w)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            entries: decode_entries(r)?,
            complete: Codec::decode(r)?,
            other_key_uses: Codec::decode(r)?,
        })
    }
}
impl Codec for KeySource {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        match self {
            Self::Generate => 0u8.encode(w),
            Self::Import { expected_hash } => {
                1u8.encode(w)?;
                expected_hash.encode(w)
            }
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(Self::Generate),
            1 => Ok(Self::Import {
                expected_hash: Codec::decode(r)?,
            }),
            _ => Err(Error::InvalidEncoding),
        }
    }
}
impl<const S: usize> Codec for Command<S> {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        match self {
            Self::Create(v) => {
                0u8.encode(w)?;
                v.encode(w)
            }
            Self::Transition(v) => {
                1u8.encode(w)?;
                v.encode(w)
            }
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(Self::Create(Codec::decode(r)?)),
            1 => Ok(Self::Transition(Codec::decode(r)?)),
            _ => Err(Error::InvalidEncoding),
        }
    }
}
impl Codec for ServiceFailure {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        match self {
            Self::CapacityExceeded => 0u8.encode(w),
            Self::StorageUnavailable => 1u8.encode(w),
            Self::Unavailable(v) => {
                2u8.encode(w)?;
                v.encode(w)
            }
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(Self::CapacityExceeded),
            1 => Ok(Self::StorageUnavailable),
            2 => Ok(Self::Unavailable(Codec::decode(r)?)),
            _ => Err(Error::InvalidEncoding),
        }
    }
}
impl Codec for OperationResult {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        match self {
            Self::Created { identity } => {
                0u8.encode(w)?;
                identity.encode(w)
            }
            Self::Existing { identity } => {
                1u8.encode(w)?;
                identity.encode(w)
            }
            Self::Enabled { identity } => {
                2u8.encode(w)?;
                identity.encode(w)
            }
            Self::Paused { identity } => {
                3u8.encode(w)?;
                identity.encode(w)
            }
            Self::Retired { identity } => {
                4u8.encode(w)?;
                identity.encode(w)
            }
            Self::RetirementBlocked { identity, reason } => {
                5u8.encode(w)?;
                identity.encode(w)?;
                reason.encode(w)
            }
            Self::ActivationFailed { identity, reason } => {
                6u8.encode(w)?;
                identity.encode(w)?;
                reason.encode(w)
            }
            Self::Cancelled => 7u8.encode(w),
            Self::ServiceAdded { identity, service } => {
                8u8.encode(w)?;
                identity.encode(w)?;
                service.encode(w)
            }
            Self::ServiceRemoved { identity, service } => {
                9u8.encode(w)?;
                identity.encode(w)?;
                service.encode(w)
            }
            Self::ServiceRemovalBlocked {
                identity,
                service,
                reason,
            } => {
                10u8.encode(w)?;
                identity.encode(w)?;
                service.encode(w)?;
                reason.encode(w)
            }
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(Self::Created {
                identity: Codec::decode(r)?,
            }),
            1 => Ok(Self::Existing {
                identity: Codec::decode(r)?,
            }),
            2 => Ok(Self::Enabled {
                identity: Codec::decode(r)?,
            }),
            3 => Ok(Self::Paused {
                identity: Codec::decode(r)?,
            }),
            4 => Ok(Self::Retired {
                identity: Codec::decode(r)?,
            }),
            5 => Ok(Self::RetirementBlocked {
                identity: Codec::decode(r)?,
                reason: Codec::decode(r)?,
            }),
            6 => Ok(Self::ActivationFailed {
                identity: Codec::decode(r)?,
                reason: Codec::decode(r)?,
            }),
            7 => Ok(Self::Cancelled),
            8 => Ok(Self::ServiceAdded {
                identity: Codec::decode(r)?,
                service: Codec::decode(r)?,
            }),
            9 => Ok(Self::ServiceRemoved {
                identity: Codec::decode(r)?,
                service: Codec::decode(r)?,
            }),
            10 => Ok(Self::ServiceRemovalBlocked {
                identity: Codec::decode(r)?,
                service: Codec::decode(r)?,
                reason: Codec::decode(r)?,
            }),
            _ => Err(Error::InvalidEncoding),
        }
    }
}
impl Codec for OperationState {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        match self {
            Self::Reserved => 0u8.encode(w),
            Self::AwaitingKey => 1u8.encode(w),
            Self::Transitioning => 2u8.encode(w),
            Self::Completed(v) => {
                3u8.encode(w)?;
                v.encode(w)
            }
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(Self::Reserved),
            1 => Ok(Self::AwaitingKey),
            2 => Ok(Self::Transitioning),
            3 => Ok(Self::Completed(Codec::decode(r)?)),
            _ => Err(Error::InvalidEncoding),
        }
    }
}
impl Codec for TransitionPhase {
    fn encode(&self, w: &mut Writer<'_>) -> Result<(), Error> {
        match self {
            Self::Activating => 0u8.encode(w),
            Self::InitializingService => 3u8.encode(w),
            Self::Quiescing => 1u8.encode(w),
            Self::ActivationCleanup(v) => {
                2u8.encode(w)?;
                v.encode(w)
            }
        }
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        match u8::decode(r)? {
            0 => Ok(Self::Activating),
            1 => Ok(Self::Quiescing),
            2 => Ok(Self::ActivationCleanup(Codec::decode(r)?)),
            3 => Ok(Self::InitializingService),
            _ => Err(Error::InvalidEncoding),
        }
    }
}

#[cfg(test)]
mod tests;
