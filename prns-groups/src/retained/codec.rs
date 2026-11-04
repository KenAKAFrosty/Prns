//! Bounded row encoding and strict canonical decoding. One decoded row plus at
//! most one encoded row is transient work, separate from a returned page. JSON
//! parser scratch and geometric Vec growth are bounded by the selected encoded
//! row (conservatively 8 * max_row_bytes + size_of::<Operation>); accepted rows
//! additionally pass the actual post-decode owned-capacity guard. This covers
//! these fixed declarations, not arbitrary Deserialize implementations, redb's
//! page cache, allocator bookkeeping or whole-process memory.
use super::*;
use std::io::{self, Write};

// Request and scope remain unchanged while the fixed-size result replaces Pending.
// Every encoded state must fit this headroom; revisit it when state encoding grows.
const STATE_GROWTH_RESERVE_BYTES: u64 = 512;

struct Counter {
    used: usize,
    max: usize,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.used = self
            .used
            .checked_add(bytes.len())
            .filter(|n| *n <= self.max)
            .ok_or_else(|| io::Error::other("retained row bound"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encoded_len(value: &impl Serialize, max: usize) -> Result<usize, Error> {
    let mut count = Counter { used: 0, max };
    serde_json::to_writer(&mut count, value).map_err(|_| Error::StorageFull)?;
    Ok(count.used)
}
struct Buffer(Vec<u8>);
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.0.capacity() - self.0.len() {
            return Err(io::Error::other("retained row bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn encode_bounded(value: &impl Serialize, max: usize) -> Result<Vec<u8>, Error> {
    let size = encoded_len(value, max)?;
    let mut buffer = Buffer(Vec::new());
    buffer
        .0
        .try_reserve_exact(size)
        .map_err(|_| Error::StorageFull)?;
    if buffer.0.capacity() > max {
        return Err(Error::StorageFull);
    }
    serde_json::to_writer(&mut buffer, value).map_err(|_| Error::StorageFull)?;
    Ok(buffer.0)
}
pub(super) fn encode(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    encode_bounded(value, 4096)
}
pub(super) fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::Corrupt)
}
struct Canonical<'a>(&'a [u8]);
impl Write for Canonical<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.0.starts_with(bytes) {
            return Err(io::Error::other("noncanonical retained row"));
        }
        self.0 = self
            .0
            .get(bytes.len()..)
            .ok_or_else(|| io::Error::other("retained row"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn canonical(value: &impl Serialize, bytes: &[u8]) -> Result<(), Error> {
    let mut writer = Canonical(bytes);
    serde_json::to_writer(&mut writer, value).map_err(|_| Error::Corrupt)?;
    if !writer.0.is_empty() {
        return Err(Error::Corrupt);
    }
    Ok(())
}
pub(super) fn owned_bytes(operation: &Operation) -> Result<usize, Error> {
    std::mem::size_of::<Operation>()
        .checked_add(crate::client::intent_heap_bytes(&operation.request.intent)?)
        .ok_or(Error::InvalidInput)
}
// No payload clone or intermediate encoding just to reserve result growth.
pub(super) fn accounted(operation: &Operation, limits: Limits) -> Result<u64, Error> {
    #[derive(Serialize)]
    struct Pending<'a> {
        request: &'a Request,
        state: State,
        scope: OperationScope,
    }
    let value = Pending {
        request: &operation.request,
        state: State::Pending,
        scope: operation.scope,
    };
    let bytes = encoded_len(&value, limits.max_row_bytes as usize)?;
    (bytes as u64)
        .checked_add(STATE_GROWTH_RESERVE_BYTES)
        .filter(|reserved| *reserved <= u64::from(limits.max_row_bytes))
        .ok_or(Error::StorageFull)
}
pub(super) fn decode_operation(
    key: &[u8],
    bytes: &[u8],
    limits: Limits,
) -> Result<Operation, Error> {
    if key.len() != 32 || bytes.len() > limits.max_row_bytes as usize {
        return Err(Error::Corrupt);
    }
    #[cfg(test)]
    super::work::decoded();
    let operation: Operation = decode(bytes)?;
    operation.request.validate().map_err(|_| Error::Corrupt)?;
    crate::RequestToken::parse(operation.request.id.0).map_err(|_| Error::Corrupt)?;
    if key != operation.request.id.0
        || owned_bytes(&operation).map_err(|_| Error::Corrupt)? as u64 > limits.max_operation_bytes
    {
        return Err(Error::Corrupt);
    }
    match (operation.request.intent.group(), operation.scope) {
        (Some(group), OperationScope::Group(scope)) if group == scope => (),
        (None, OperationScope::Create)
            if matches!(operation.request.intent, Intent::Create { .. }) => {}
        (None, OperationScope::Group(_))
            if !matches!(operation.request.intent, Intent::Create { .. }) => {}
        _ => return Err(Error::Corrupt),
    }
    if let State::Completed(outcome) = operation.state {
        let valid = matches!(
            (&operation.request.intent, outcome),
            (Intent::Publish(_), Outcome::Published(_))
                | (Intent::Create { .. }, Outcome::Created(_))
                | (Intent::Invite { .. }, Outcome::Invited(_))
        ) || (matches!(outcome, Outcome::Saved)
            && !matches!(
                operation.request.intent,
                Intent::Publish(_) | Intent::Create { .. } | Intent::Invite { .. }
            ));
        if !valid {
            return Err(Error::Corrupt);
        }
    }
    if bytes.len() as u64 > accounted(&operation, limits).map_err(|_| Error::Corrupt)? {
        return Err(Error::Corrupt);
    }
    canonical(&operation, bytes)?;
    Ok(operation)
}
pub(super) fn encode_operation(operation: &Operation, limits: Limits) -> Result<Vec<u8>, Error> {
    if owned_bytes(operation)? as u64 > limits.max_operation_bytes {
        return Err(Error::StorageFull);
    }
    let bytes = encode_bounded(operation, limits.max_row_bytes as usize)?;
    // Admission must fit the actual persisted decoder's capacity, not just the
    // caller's often smaller Vec capacity. Otherwise accepted work could become
    // impossible to reopen or page after a geometrically growing decoder.
    decode_operation(&operation.request.id.0, &bytes, limits).map_err(|_| Error::StorageFull)?;
    Ok(bytes)
}
