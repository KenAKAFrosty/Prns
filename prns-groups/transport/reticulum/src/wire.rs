use crate::Limits;
use prns_groups_service::{
    EnvelopeId, EnvelopeInfo, GroupId, InventoryCursor, InventoryKind, InventoryPage,
    InventorySnapshot, RecordId,
};

const MAGIC: &[u8; 4] = b"PGT\x01";
const SNAPSHOT: u8 = 1;
const INVENTORY: u8 = 2;
const FETCH: u8 = 3;
const CAPABILITIES: u8 = 4;
const SUBMIT: u8 = 5;
pub(crate) const SUBMIT_OVERHEAD: usize = 77;
pub(crate) const SUBMITTED_OVERHEAD: usize = 42;
const FAILURE: u8 = 0;
const ITEM_BYTES: usize = 77;
#[cfg(test)]
const MAX_REQUEST_BYTES: usize = 90;

/// Route a complete, well-framed sync request to its local group handler without
/// allocating. This checks syntax only; the selected handler still authenticates
/// the requester and applies its configured page/work budgets.
pub fn request_group(bytes: &[u8]) -> Result<GroupId, ProtocolError> {
    Request::decode_bounded(
        bytes,
        prns_core::routing::links::resources::MAX_EFFICIENT_SIZE,
        usize::MAX,
    )
    .map(|request| request.group())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Allocation,
    Oversized,
    InvalidEncoding,
    WrongEnvelope,
    InvalidInventory,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RemoteError {
    Unauthorized = 1,
    UnknownGroup = 2,
    InvalidRequest = 3,
    RestartInventory = 4,
    Unavailable = 5,
    NotFound = 6,
    OverBudget = 7,
}

pub(crate) enum Request<'a> {
    Capabilities {
        group: GroupId,
    },
    Submit {
        group: GroupId,
        id: EnvelopeId,
        bytes: &'a [u8],
        receipt_bytes: usize,
    },
    Snapshot {
        group: GroupId,
    },
    Inventory {
        group: GroupId,
        snapshot: InventorySnapshot,
        after: InventoryCursor,
        kind: InventoryKind,
        scan_limit: usize,
    },
    Fetch {
        group: GroupId,
        id: EnvelopeId,
    },
}
impl<'a> Request<'a> {
    pub(crate) fn group(&self) -> GroupId {
        match self {
            Self::Capabilities { group }
            | Self::Submit { group, .. }
            | Self::Snapshot { group }
            | Self::Inventory { group, .. }
            | Self::Fetch { group, .. } => *group,
        }
    }
    pub(crate) fn encode(&self, limits: Limits) -> Result<Vec<u8>, ProtocolError> {
        let mut out = Encoder::new(limits);
        match self {
            Self::Capabilities { group } => {
                out.header(CAPABILITIES)?;
                out.put(&group.0)?;
            }
            Self::Submit {
                group,
                id,
                bytes,
                receipt_bytes,
            } => {
                if bytes.is_empty() {
                    return Err(ProtocolError::InvalidEncoding);
                }
                out.header(SUBMIT)?;
                out.put(&group.0)?;
                out.put(&id.0)?;
                out.usize(bytes.len())?;
                out.usize(*receipt_bytes)?;
                out.put(bytes)?;
            }
            Self::Snapshot { group } => {
                out.header(SNAPSHOT)?;
                out.put(&group.0)?;
            }
            Self::Inventory {
                group,
                snapshot,
                after,
                kind,
                scan_limit,
            } => {
                if *scan_limit == 0 || *scan_limit > limits.max_page_items {
                    return Err(ProtocolError::Oversized);
                }
                out.header(INVENTORY)?;
                out.put(&group.0)?;
                out.snapshot(*snapshot)?;
                out.u64(after.0)?;
                out.put(&[kind_byte(*kind)])?;
                out.usize(*scan_limit)?;
            }
            Self::Fetch { group, id } => {
                out.header(FETCH)?;
                out.put(&group.0)?;
                out.put(&id.0)?;
            }
        }
        Ok(out.bytes)
    }
    pub(crate) fn decode(bytes: &'a [u8], limits: Limits) -> Result<Self, ProtocolError> {
        Self::decode_bounded(bytes, limits.max_frame_bytes, limits.max_page_items)
    }

    fn decode_bounded(
        bytes: &'a [u8],
        max_frame_bytes: usize,
        max_page_items: usize,
    ) -> Result<Self, ProtocolError> {
        let (tag, mut input) = Decoder::bounded(bytes, max_frame_bytes)?;
        let group = GroupId(input.array()?);
        let request = match tag {
            CAPABILITIES => Self::Capabilities { group },
            SUBMIT => {
                let id = EnvelopeId(input.array()?);
                let size = input.usize()?;
                let receipt_bytes = input.usize()?;
                if size == 0 || receipt_bytes > max_frame_bytes.saturating_sub(SUBMITTED_OVERHEAD) {
                    return Err(ProtocolError::Oversized);
                }
                Self::Submit {
                    group,
                    id,
                    bytes: input.take(size)?,
                    receipt_bytes,
                }
            }
            SNAPSHOT => Self::Snapshot { group },
            INVENTORY => {
                let snapshot = input.snapshot()?;
                let after = InventoryCursor(input.u64()?);
                let kind = input.kind()?;
                let scan_limit = input.usize()?;
                if scan_limit == 0 || scan_limit > max_page_items {
                    return Err(ProtocolError::Oversized);
                }
                Self::Inventory {
                    group,
                    snapshot,
                    after,
                    kind,
                    scan_limit,
                }
            }
            FETCH => Self::Fetch {
                group,
                id: EnvelopeId(input.array()?),
            },
            _ => return Err(ProtocolError::InvalidEncoding),
        };
        input.finish()?;
        Ok(request)
    }
}

pub(crate) enum Response<'a> {
    Capabilities {
        snapshot: Option<InventorySnapshot>,
        max_submit_bytes: usize,
    },
    Submitted {
        id: EnvelopeId,
        accepted: bool,
        receipt: &'a [u8],
    },
    Snapshot(Option<InventorySnapshot>),
    Inventory(InventoryPage),
    Envelope {
        id: EnvelopeId,
        bytes: &'a [u8],
    },
    Error(RemoteError),
}
impl<'a> Response<'a> {
    pub(crate) fn encode(&self, limits: Limits) -> Result<Vec<u8>, ProtocolError> {
        let mut out = Encoder::new(limits);
        match self {
            Self::Capabilities {
                snapshot,
                max_submit_bytes,
            } => {
                out.header(CAPABILITIES)?;
                out.usize(*max_submit_bytes)?;
                match snapshot {
                    Some(s) => {
                        out.put(&[1])?;
                        out.snapshot(*s)?;
                    }
                    None => out.put(&[0])?,
                }
            }
            Self::Submitted {
                id,
                accepted,
                receipt,
            } => {
                out.header(SUBMIT)?;
                out.put(&id.0)?;
                out.put(&[u8::from(*accepted)])?;
                out.usize(receipt.len())?;
                out.put(receipt)?;
            }
            Self::Error(error) => {
                out.header(FAILURE)?;
                out.put(&[*error as u8])?;
            }
            Self::Snapshot(snapshot) => {
                out.header(SNAPSHOT)?;
                match snapshot {
                    Some(s) => {
                        out.put(&[1])?;
                        out.snapshot(*s)?;
                    }
                    None => out.put(&[0])?,
                }
            }
            Self::Inventory(page) => {
                if page.items.len() > limits.max_page_items {
                    return Err(ProtocolError::Oversized);
                }
                out.header(INVENTORY)?;
                out.snapshot(page.snapshot)?;
                out.u64(page.next.0)?;
                out.usize(page.items.len())?;
                for item in &page.items {
                    out.u64(item.position.0)?;
                    out.put(&item.id.0)?;
                    out.put(&[kind_byte(item.kind)])?;
                    out.put(&item.record.0)?;
                    out.usize(item.bytes)?;
                }
            }
            Self::Envelope { id, bytes } => {
                out.header(FETCH)?;
                out.put(&id.0)?;
                out.usize(bytes.len())?;
                out.put(bytes)?;
            }
        }
        Ok(out.bytes)
    }
    pub(crate) fn decode(bytes: &'a [u8], limits: Limits) -> Result<Self, ProtocolError> {
        let (tag, mut input) = Decoder::new(bytes, limits)?;
        let response = match tag {
            CAPABILITIES => {
                let max_submit_bytes = input.usize()?;
                let snapshot = match input.byte()? {
                    0 => None,
                    1 => Some(input.snapshot()?),
                    _ => return Err(ProtocolError::InvalidEncoding),
                };
                Self::Capabilities {
                    snapshot,
                    max_submit_bytes,
                }
            }
            SUBMIT => {
                let id = EnvelopeId(input.array()?);
                let accepted = match input.byte()? {
                    0 => false,
                    1 => true,
                    _ => return Err(ProtocolError::InvalidEncoding),
                };
                let size = input.usize()?;
                if !accepted && size != 0 {
                    return Err(ProtocolError::InvalidEncoding);
                }
                Self::Submitted {
                    id,
                    accepted,
                    receipt: input.take(size)?,
                }
            }
            FAILURE => Self::Error(match input.byte()? {
                1 => RemoteError::Unauthorized,
                2 => RemoteError::UnknownGroup,
                3 => RemoteError::InvalidRequest,
                4 => RemoteError::RestartInventory,
                5 => RemoteError::Unavailable,
                6 => RemoteError::NotFound,
                7 => RemoteError::OverBudget,
                _ => return Err(ProtocolError::InvalidEncoding),
            }),
            SNAPSHOT => Self::Snapshot(match input.byte()? {
                0 => None,
                1 => Some(input.snapshot()?),
                _ => return Err(ProtocolError::InvalidEncoding),
            }),
            INVENTORY => {
                let snapshot = input.snapshot()?;
                let next = InventoryCursor(input.u64()?);
                let count = input.usize()?;
                if count > limits.max_page_items
                    || count.checked_mul(ITEM_BYTES) != Some(input.bytes.len())
                {
                    return Err(ProtocolError::InvalidEncoding);
                }
                let mut items = Vec::new();
                items
                    .try_reserve_exact(count)
                    .map_err(|_| ProtocolError::Allocation)?;
                for _ in 0..count {
                    items.push(EnvelopeInfo {
                        position: InventoryCursor(input.u64()?),
                        id: EnvelopeId(input.array()?),
                        kind: input.kind()?,
                        record: RecordId(input.array()?),
                        bytes: input.usize()?,
                    });
                }
                Self::Inventory(InventoryPage {
                    snapshot,
                    next,
                    items,
                })
            }
            FETCH => {
                let id = EnvelopeId(input.array()?);
                let size = input.usize()?;
                Self::Envelope {
                    id,
                    bytes: input.take(size)?,
                }
            }
            _ => return Err(ProtocolError::InvalidEncoding),
        };
        input.finish()?;
        Ok(response)
    }
}
fn kind_byte(kind: InventoryKind) -> u8 {
    match kind {
        InventoryKind::Record => 1,
        InventoryKind::Receipt => 2,
    }
}
struct Encoder {
    bytes: Vec<u8>,
    limit: usize,
}
impl Encoder {
    fn new(limits: Limits) -> Self {
        Self {
            bytes: Vec::new(),
            limit: limits.max_frame_bytes,
        }
    }
    fn put(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or(ProtocolError::Oversized)?
            > self.limit
        {
            return Err(ProtocolError::Oversized);
        }
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| ProtocolError::Allocation)?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn header(&mut self, tag: u8) -> Result<(), ProtocolError> {
        self.put(MAGIC)?;
        self.put(&[tag])
    }
    fn u64(&mut self, value: u64) -> Result<(), ProtocolError> {
        self.put(&value.to_be_bytes())
    }
    fn usize(&mut self, value: usize) -> Result<(), ProtocolError> {
        self.put(
            &u32::try_from(value)
                .map_err(|_| ProtocolError::Oversized)?
                .to_be_bytes(),
        )
    }
    fn snapshot(&mut self, s: InventorySnapshot) -> Result<(), ProtocolError> {
        self.put(&s.generation)?;
        self.u64(s.head.0)
    }
}
struct Decoder<'a> {
    bytes: &'a [u8],
}
impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8], limits: Limits) -> Result<(u8, Self), ProtocolError> {
        Self::bounded(bytes, limits.max_frame_bytes)
    }
    fn bounded(bytes: &'a [u8], max_frame_bytes: usize) -> Result<(u8, Self), ProtocolError> {
        if bytes.len() > max_frame_bytes {
            return Err(ProtocolError::Oversized);
        }
        let mut input = Self { bytes };
        if input.array::<4>()? != *MAGIC {
            return Err(ProtocolError::InvalidEncoding);
        }
        Ok((input.byte()?, input))
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], ProtocolError> {
        if count > self.bytes.len() {
            return Err(ProtocolError::InvalidEncoding);
        }
        let (bytes, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        self.take(N)?
            .try_into()
            .map_err(|_| ProtocolError::InvalidEncoding)
    }
    fn byte(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.array::<1>()?[0])
    }
    fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    fn usize(&mut self) -> Result<usize, ProtocolError> {
        usize::try_from(u32::from_be_bytes(self.array()?)).map_err(|_| ProtocolError::Oversized)
    }
    fn kind(&mut self) -> Result<InventoryKind, ProtocolError> {
        match self.byte()? {
            1 => Ok(InventoryKind::Record),
            2 => Ok(InventoryKind::Receipt),
            _ => Err(ProtocolError::InvalidEncoding),
        }
    }
    fn snapshot(&mut self) -> Result<InventorySnapshot, ProtocolError> {
        Ok(InventorySnapshot {
            generation: self.array()?,
            head: InventoryCursor(self.u64()?),
        })
    }
    fn finish(self) -> Result<(), ProtocolError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError::InvalidEncoding)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn limits() -> Limits {
        Limits {
            max_frame_bytes: 4096,
            max_page_items: 8,
            max_peers: 4,
            inbound_jobs: 8,
            exchange_timeout: std::time::Duration::from_secs(1),
        }
    }
    #[cfg(feature = "network")]
    #[test]
    fn projected_envelope_bound_fits_actual_submit_and_fetch_wrappers() {
        let limits = limits();
        let maximum = limits.max_envelope_bytes().unwrap();
        let bytes = vec![3; maximum];
        let request = Request::Submit {
            group: GroupId([1; 32]),
            id: EnvelopeId([2; 32]),
            bytes: &bytes,
            receipt_bytes: maximum,
        };
        let encoded = request.encode(limits).unwrap();
        assert_eq!(encoded.len(), limits.max_frame_bytes);
        assert!(Request::decode(&encoded, limits).is_ok());
        assert!(Response::Envelope {
            id: EnvelopeId([2; 32]),
            bytes: &bytes
        }
        .encode(limits)
        .is_ok());
        let larger = vec![3; maximum + 1];
        assert_eq!(
            Request::Submit {
                group: GroupId([1; 32]),
                id: EnvelopeId([2; 32]),
                bytes: &larger,
                receipt_bytes: maximum,
            }
            .encode(limits),
            Err(ProtocolError::Oversized)
        );
        assert_eq!(
            Limits {
                max_frame_bytes: usize::MAX,
                ..limits
            }
            .max_envelope_bytes(),
            Err(crate::NetworkError::InvalidLimits)
        );
    }
    #[test]
    fn request_and_response_reject_truncation_unknown_versions_and_extra_bytes() {
        let request = Request::Inventory {
            group: GroupId([1; 32]),
            snapshot: InventorySnapshot {
                generation: [2; 32],
                head: InventoryCursor(7),
            },
            after: InventoryCursor(3),
            kind: InventoryKind::Receipt,
            scan_limit: 8,
        };
        let bytes = request.encode(limits()).unwrap();
        assert!(Request::decode(&bytes, limits()).is_ok());
        for size in 0..bytes.len() {
            assert!(Request::decode(&bytes[..size], limits()).is_err());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Request::decode(&extra, limits()).is_err());
        let mut version = bytes;
        version[3] = 2;
        assert!(Request::decode(&version, limits()).is_err());
        let response = Response::Envelope {
            id: EnvelopeId([3; 32]),
            bytes: b"ciphertext",
        }
        .encode(limits())
        .unwrap();
        for size in 0..response.len() {
            assert!(Response::decode(&response[..size], limits()).is_err());
        }
        let mut extra = response.clone();
        extra.push(0);
        assert!(Response::decode(&extra, limits()).is_err());
        assert!(matches!(
            Response::decode(&response, limits()).unwrap(),
            Response::Envelope {
                bytes: b"ciphertext",
                ..
            }
        ));
    }
    #[test]
    fn forged_inventory_count_cannot_allocate() {
        let page = InventoryPage {
            snapshot: InventorySnapshot {
                generation: [4; 32],
                head: InventoryCursor(0),
            },
            next: InventoryCursor(0),
            items: Vec::new(),
        };
        let mut bytes = Response::Inventory(page).encode(limits()).unwrap();
        bytes[53..57].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            Response::decode(&bytes, limits()),
            Err(ProtocolError::InvalidEncoding)
        ));
    }

    #[test]
    fn group_routing_requires_a_complete_valid_request() {
        let group = GroupId([9; 32]);
        let requests = [
            Request::Snapshot { group },
            Request::Fetch {
                group,
                id: EnvelopeId([1; 32]),
            },
            Request::Inventory {
                group,
                snapshot: InventorySnapshot {
                    generation: [2; 32],
                    head: InventoryCursor(7),
                },
                after: InventoryCursor(3),
                kind: InventoryKind::Receipt,
                scan_limit: 8,
            },
        ];
        for request in requests {
            let bytes = request.encode(limits()).unwrap();
            assert_eq!(request_group(&bytes), Ok(group));
            for size in 0..bytes.len() {
                assert!(request_group(&bytes[..size]).is_err());
            }
            let mut extra = bytes.clone();
            extra.push(0);
            assert!(request_group(&extra).is_err());
            let mut version = bytes.clone();
            version[3] = 2;
            assert!(request_group(&version).is_err());
            let mut tag = bytes;
            tag[4] = 255;
            assert!(request_group(&tag).is_err());
        }
        let mut inventory = Request::Inventory {
            group,
            snapshot: InventorySnapshot {
                generation: [2; 32],
                head: InventoryCursor(7),
            },
            after: InventoryCursor(3),
            kind: InventoryKind::Receipt,
            scan_limit: 8,
        }
        .encode(limits())
        .unwrap();
        assert_eq!(inventory.len(), MAX_REQUEST_BYTES);
        inventory[86..90].copy_from_slice(&0_u32.to_be_bytes());
        assert_eq!(request_group(&inventory), Err(ProtocolError::Oversized));
        inventory[86..90].copy_from_slice(&8_u32.to_be_bytes());
        inventory[85] = 255;
        assert_eq!(
            request_group(&inventory),
            Err(ProtocolError::InvalidEncoding)
        );
        assert_eq!(
            request_group(&[0; MAX_REQUEST_BYTES + 1]),
            Err(ProtocolError::InvalidEncoding)
        );
    }
    #[test]
    fn submit_and_capability_frames_are_strict_borrowed_and_bounded() {
        let group = GroupId([7; 32]);
        let body = vec![3; 900];
        let request = Request::Submit {
            group,
            id: EnvelopeId([8; 32]),
            bytes: &body,
            receipt_bytes: 400,
        };
        let wire = request.encode(limits()).unwrap();
        assert_eq!(wire.len(), SUBMIT_OVERHEAD + body.len());
        assert_eq!(request_group(&wire), Ok(group));
        assert!(matches!(Request::decode(&wire, limits()).unwrap(),
            Request::Submit { bytes, receipt_bytes: 400, .. } if bytes == body));
        for end in 0..wire.len() {
            assert!(request_group(&wire[..end]).is_err());
        }
        let mut extra = wire.clone();
        extra.push(0);
        assert!(request_group(&extra).is_err());
        let mut forged = wire.clone();
        forged[69..73].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(request_group(&forged).is_err());
        assert!(matches!(
            Request::decode(
                &wire,
                Limits {
                    max_frame_bytes: wire.len() - 1,
                    ..limits()
                }
            ),
            Err(ProtocolError::Oversized)
        ));
        let mut receipt_budget = wire.clone();
        receipt_budget[73..77].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(Request::decode(&receipt_budget, limits()).is_err());
        let cap = Request::Capabilities { group }.encode(limits()).unwrap();
        assert_eq!(request_group(&cap), Ok(group));
        let response = Response::Capabilities {
            snapshot: None,
            max_submit_bytes: 100,
        }
        .encode(limits())
        .unwrap();
        assert!(matches!(
            Response::decode(&response, limits()).unwrap(),
            Response::Capabilities {
                max_submit_bytes: 100,
                snapshot: None
            }
        ));
        let receipt = Response::Submitted {
            id: EnvelopeId([8; 32]),
            accepted: true,
            receipt: b"exact receipt",
        }
        .encode(limits())
        .unwrap();
        for end in 0..receipt.len() {
            assert!(Response::decode(&receipt[..end], limits()).is_err());
        }
        let mut contradicted = receipt.clone();
        contradicted[37] = 0;
        assert!(
            Response::decode(&contradicted, limits()).is_err(),
            "rejection cannot carry acceptance evidence"
        );
        let mut unknown = receipt;
        unknown[37] = 2;
        assert!(Response::decode(&unknown, limits()).is_err());
    }
    #[test]
    fn fetch_ceiling_can_exceed_submit_ceiling_without_rewriting_ciphertext() {
        let bytes = vec![7; limits().max_frame_bytes - 41];
        let id = EnvelopeId([9; 32]);
        assert_eq!(
            Response::Envelope { id, bytes: &bytes }
                .encode(limits())
                .unwrap()
                .len(),
            limits().max_frame_bytes
        );
        assert_eq!(
            Request::Submit {
                group: GroupId([1; 32]),
                id,
                bytes: &bytes,
                receipt_bytes: 0
            }
            .encode(limits()),
            Err(ProtocolError::Oversized)
        );
    }
}
