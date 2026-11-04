//! Private durable fact encodings. The storage owner supplies atomicity and
//! confidentiality; these bytes do not authenticate network statements.

use alloc::vec::Vec;

use prns_groups_core::{
    DurableReceipt, Endpoint, EndpointId, EndpointIncarnation, GroupId, RecordId,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    Change, ChangeCursor, ChangeKind, EnvelopeId, Fault, Incoming, OperationId, RejectReason,
};

const FACT_MAGIC: &[u8; 4] = b"PGF\x01";
const INCOMING_MAGIC: &[u8; 4] = b"PGI\x01";
const CHANGE_MAGIC: &[u8; 4] = b"PGC\x01";
const FEED_TAG: u8 = b'F';
const RECEIPT_FIELDS_BYTES: usize = 104;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fact {
    pub fingerprint: [u8; 32],
    pub result: ResultFact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResultFact {
    Published(RecordId),
    ReceiptQueued(RecordId),
    Incoming(Incoming),
}

pub(crate) fn key(tag: u8, id: &[u8; 32]) -> [u8; 33] {
    let mut result = [0; 33];
    result[0] = tag;
    result[1..].copy_from_slice(id);
    result
}

pub(crate) fn record_key(tag: u8, group: GroupId, record: RecordId) -> [u8; 33] {
    let mut digest = Sha256::new();
    digest.update(group.0);
    digest.update(record.0);
    let id: [u8; 32] = digest.finalize().into();
    key(tag, &id)
}

pub(crate) fn feed_key(group: GroupId, cursor: ChangeCursor) -> [u8; 41] {
    let mut result = [0; 41];
    result[0] = FEED_TAG;
    result[1..33].copy_from_slice(&group.0);
    result[33..].copy_from_slice(&cursor.0.to_be_bytes());
    result
}

pub(crate) fn encode_fact(fact: &Fact) -> Result<Zeroizing<Vec<u8>>, Fault> {
    let payload_bytes = match fact.result {
        ResultFact::Published(_) | ResultFact::ReceiptQueued(_) => 32,
        ResultFact::Incoming(incoming) => incoming_bytes(incoming),
    };
    let mut encoded = buffer(FACT_MAGIC, 4 + 32 + 1 + payload_bytes)?;
    encoded.extend_from_slice(&fact.fingerprint);
    match fact.result {
        ResultFact::Published(record) => {
            encoded.push(1);
            encoded.extend_from_slice(&record.0);
        }
        ResultFact::ReceiptQueued(record) => {
            encoded.push(2);
            encoded.extend_from_slice(&record.0);
        }
        ResultFact::Incoming(incoming) => {
            encoded.push(3);
            append_incoming(&mut encoded, incoming);
        }
    }
    Ok(encoded)
}

pub(crate) fn decode_fact(bytes: &[u8]) -> Result<Fact, Fault> {
    let mut cursor = Cursor::new(bytes, FACT_MAGIC)?;
    let fingerprint = cursor.array()?;
    let result = match cursor.byte()? {
        1 => ResultFact::Published(RecordId(cursor.array()?)),
        2 => ResultFact::ReceiptQueued(RecordId(cursor.array()?)),
        3 => ResultFact::Incoming(cursor.incoming()?),
        _ => return Err(Fault::InvalidEncoding),
    };
    cursor.finish()?;
    Ok(Fact {
        fingerprint,
        result,
    })
}

pub(crate) fn encode_incoming(incoming: Incoming) -> Result<Zeroizing<Vec<u8>>, Fault> {
    let mut encoded = buffer(INCOMING_MAGIC, 4 + incoming_bytes(incoming))?;
    append_incoming(&mut encoded, incoming);
    Ok(encoded)
}

pub(crate) fn decode_incoming(bytes: &[u8]) -> Result<Incoming, Fault> {
    let mut cursor = Cursor::new(bytes, INCOMING_MAGIC)?;
    let incoming = cursor.incoming()?;
    cursor.finish()?;
    Ok(incoming)
}

pub(crate) fn encode_change(change: &Change) -> Result<Zeroizing<Vec<u8>>, Fault> {
    let payload_bytes = match change.kind {
        ChangeKind::Published(_) | ChangeKind::Received(_) => 32,
        ChangeKind::Receipt(_) | ChangeKind::ReceiptQueued(_) => RECEIPT_FIELDS_BYTES,
        ChangeKind::Rejected { .. } => 33,
    };
    let mut encoded = buffer(CHANGE_MAGIC, 4 + 8 + 32 + 1 + payload_bytes)?;
    encoded.extend_from_slice(&change.cursor.0.to_be_bytes());
    encoded.extend_from_slice(&change.operation.0);
    match change.kind {
        ChangeKind::Published(record) => {
            encoded.push(1);
            encoded.extend_from_slice(&record.0);
        }
        ChangeKind::Received(record) => {
            encoded.push(2);
            encoded.extend_from_slice(&record.0);
        }
        ChangeKind::Receipt(receipt) => {
            encoded.push(3);
            append_receipt(&mut encoded, receipt);
        }
        ChangeKind::Rejected { envelope, reason } => {
            encoded.push(4);
            encoded.extend_from_slice(&envelope.0);
            encoded.push(reason_byte(reason));
        }
        ChangeKind::ReceiptQueued(receipt) => {
            encoded.push(5);
            append_receipt(&mut encoded, receipt);
        }
    }
    Ok(encoded)
}

pub(crate) fn decode_change(bytes: &[u8]) -> Result<Change, Fault> {
    let mut cursor = Cursor::new(bytes, CHANGE_MAGIC)?;
    let change_cursor = ChangeCursor(u64::from_be_bytes(cursor.array()?));
    let operation = OperationId(cursor.array()?);
    let kind = match cursor.byte()? {
        1 => ChangeKind::Published(RecordId(cursor.array()?)),
        2 => ChangeKind::Received(RecordId(cursor.array()?)),
        3 => ChangeKind::Receipt(cursor.receipt()?),
        4 => ChangeKind::Rejected {
            envelope: EnvelopeId(cursor.array()?),
            reason: cursor.reason()?,
        },
        5 => ChangeKind::ReceiptQueued(cursor.receipt()?),
        _ => return Err(Fault::InvalidEncoding),
    };
    cursor.finish()?;
    Ok(Change {
        cursor: change_cursor,
        operation,
        kind,
    })
}

fn buffer(magic: &[u8; 4], capacity: usize) -> Result<Zeroizing<Vec<u8>>, Fault> {
    let mut encoded = Zeroizing::new(Vec::new());
    encoded
        .try_reserve_exact(capacity)
        .map_err(|_| Fault::Allocation)?;
    encoded.extend_from_slice(magic);
    Ok(encoded)
}

fn incoming_bytes(incoming: Incoming) -> usize {
    1 + match incoming {
        Incoming::Record(_) => 32,
        Incoming::Receipt(_) => RECEIPT_FIELDS_BYTES,
        Incoming::Rejected(_) => 1,
    }
}

fn append_incoming(encoded: &mut Vec<u8>, incoming: Incoming) {
    match incoming {
        Incoming::Record(record) => {
            encoded.push(1);
            encoded.extend_from_slice(&record.0);
        }
        Incoming::Receipt(receipt) => {
            encoded.push(2);
            append_receipt(encoded, receipt);
        }
        Incoming::Rejected(reason) => {
            encoded.push(3);
            encoded.push(reason_byte(reason));
        }
    }
}

fn append_receipt(encoded: &mut Vec<u8>, receipt: DurableReceipt) {
    encoded.extend_from_slice(&receipt.group.0);
    encoded.extend_from_slice(&receipt.record.0);
    encoded.extend_from_slice(&receipt.target.id.0);
    encoded.extend_from_slice(&receipt.target.incarnation.get().to_be_bytes());
}

fn reason_byte(reason: RejectReason) -> u8 {
    match reason {
        RejectReason::InvalidPacket => 1,
        RejectReason::WrongGroup => 2,
        RejectReason::WrongAuthor => 3,
        RejectReason::WrongControl => 4,
        RejectReason::NotTargeted => 5,
        RejectReason::Expired => 6,
        RejectReason::TimeUncertain => 7,
        RejectReason::UnknownRecord => 8,
        RejectReason::InvalidReceipt => 9,
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], magic: &[u8; 4]) -> Result<Self, Fault> {
        let mut result = Self { bytes, position: 0 };
        if result.array::<4>()? != *magic {
            return Err(Fault::InvalidEncoding);
        }
        Ok(result)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Fault> {
        let end = self.position.checked_add(N).ok_or(Fault::InvalidEncoding)?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or(Fault::InvalidEncoding)?
            .try_into()
            .map_err(|_| Fault::InvalidEncoding)?;
        self.position = end;
        Ok(result)
    }

    fn byte(&mut self) -> Result<u8, Fault> {
        Ok(self.array::<1>()?[0])
    }

    fn incoming(&mut self) -> Result<Incoming, Fault> {
        match self.byte()? {
            1 => Ok(Incoming::Record(RecordId(self.array()?))),
            2 => Ok(Incoming::Receipt(self.receipt()?)),
            3 => Ok(Incoming::Rejected(self.reason()?)),
            _ => Err(Fault::InvalidEncoding),
        }
    }

    fn receipt(&mut self) -> Result<DurableReceipt, Fault> {
        Ok(DurableReceipt {
            group: GroupId(self.array()?),
            record: RecordId(self.array()?),
            target: Endpoint {
                id: EndpointId(self.array()?),
                incarnation: EndpointIncarnation::new(u64::from_be_bytes(self.array()?))
                    .map_err(|_| Fault::InvalidEncoding)?,
            },
        })
    }

    fn reason(&mut self) -> Result<RejectReason, Fault> {
        match self.byte()? {
            1 => Ok(RejectReason::InvalidPacket),
            2 => Ok(RejectReason::WrongGroup),
            3 => Ok(RejectReason::WrongAuthor),
            4 => Ok(RejectReason::WrongControl),
            5 => Ok(RejectReason::NotTargeted),
            6 => Ok(RejectReason::Expired),
            7 => Ok(RejectReason::TimeUncertain),
            8 => Ok(RejectReason::UnknownRecord),
            9 => Ok(RejectReason::InvalidReceipt),
            _ => Err(Fault::InvalidEncoding),
        }
    }

    fn finish(self) -> Result<(), Fault> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(Fault::InvalidEncoding)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn receipt() -> DurableReceipt {
        DurableReceipt {
            group: GroupId([1; 32]),
            record: RecordId([2; 32]),
            target: Endpoint {
                id: EndpointId([3; 32]),
                incarnation: EndpointIncarnation::new(u64::MAX).unwrap(),
            },
        }
    }

    #[test]
    fn keys_separate_groups_record_types_and_numerically_ordered_feed_positions() {
        let group = GroupId([1; 32]);
        let record = RecordId([2; 32]);
        let key_a = record_key(b'R', group, record);
        assert_eq!(key_a[0], b'R');
        assert_ne!(key_a, record_key(b'R', GroupId([2; 32]), record));
        assert_ne!(key_a, record_key(b'D', group, record));
        assert_eq!(&key(9, &[7; 32])[1..], &[7; 32]);
        assert!(feed_key(group, ChangeCursor(255)) < feed_key(group, ChangeCursor(256)));
        assert!(feed_key(group, ChangeCursor(256)) < feed_key(group, ChangeCursor(u64::MAX)));
        assert_ne!(
            feed_key(group, ChangeCursor(1)),
            feed_key(GroupId([2; 32]), ChangeCursor(1))
        );
    }

    #[test]
    fn operation_facts_preserve_fingerprint_and_every_result_kind() {
        for result in [
            ResultFact::Published(RecordId([1; 32])),
            ResultFact::ReceiptQueued(RecordId([2; 32])),
            ResultFact::Incoming(Incoming::Record(RecordId([3; 32]))),
            ResultFact::Incoming(Incoming::Receipt(receipt())),
            ResultFact::Incoming(Incoming::Rejected(RejectReason::Expired)),
        ] {
            let fact = Fact {
                fingerprint: [4; 32],
                result,
            };
            let encoded = encode_fact(&fact).unwrap();
            assert_eq!(decode_fact(&encoded), Ok(fact));
            assert!(encoded.len() <= 142);
            for length in 0..encoded.len() {
                assert!(decode_fact(&encoded[..length]).is_err());
            }
            let mut trailing = encoded;
            trailing.push(0);
            assert_eq!(decode_fact(&trailing), Err(Fault::InvalidEncoding));
        }
    }

    #[test]
    fn incoming_outcomes_have_exact_encodings_and_typed_rejections() {
        for incoming in [
            Incoming::Record(RecordId([1; 32])),
            Incoming::Receipt(receipt()),
            Incoming::Rejected(RejectReason::InvalidPacket),
            Incoming::Rejected(RejectReason::WrongGroup),
            Incoming::Rejected(RejectReason::WrongAuthor),
            Incoming::Rejected(RejectReason::WrongControl),
            Incoming::Rejected(RejectReason::NotTargeted),
            Incoming::Rejected(RejectReason::Expired),
            Incoming::Rejected(RejectReason::TimeUncertain),
            Incoming::Rejected(RejectReason::UnknownRecord),
            Incoming::Rejected(RejectReason::InvalidReceipt),
        ] {
            let encoded = encode_incoming(incoming).unwrap();
            assert_eq!(decode_incoming(&encoded), Ok(incoming));
            for length in 0..encoded.len() {
                assert!(decode_incoming(&encoded[..length]).is_err());
            }
            let mut trailing = encoded;
            trailing.push(0);
            assert_eq!(decode_incoming(&trailing), Err(Fault::InvalidEncoding));
        }
        for invalid in [0, 10, u8::MAX] {
            assert_eq!(
                decode_incoming(&[b'P', b'G', b'I', 1, 3, invalid]),
                Err(Fault::InvalidEncoding)
            );
        }
    }

    #[test]
    fn changes_bind_feed_position_operation_and_all_variants() {
        for kind in [
            ChangeKind::Published(RecordId([1; 32])),
            ChangeKind::Received(RecordId([2; 32])),
            ChangeKind::Receipt(receipt()),
            ChangeKind::ReceiptQueued(receipt()),
            ChangeKind::Rejected {
                envelope: EnvelopeId([3; 32]),
                reason: RejectReason::TimeUncertain,
            },
        ] {
            let change = Change {
                cursor: ChangeCursor(u64::MAX),
                operation: OperationId([4; 32]),
                kind,
            };
            let encoded = encode_change(&change).unwrap();
            assert_eq!(decode_change(&encoded), Ok(change));
            assert!(encoded.len() <= 149);
            for length in 0..encoded.len() {
                assert!(decode_change(&encoded[..length]).is_err());
            }
            let mut trailing = encoded;
            trailing.push(0);
            assert_eq!(decode_change(&trailing), Err(Fault::InvalidEncoding));
        }
    }

    #[test]
    fn unknown_versions_tags_and_zero_receipt_incarnations_fail_closed() {
        let mut encoded = encode_incoming(Incoming::Receipt(receipt())).unwrap();
        for (offset, value) in [(0, b'X'), (3, 2), (4, 99)] {
            let mut changed = encoded.clone();
            changed[offset] = value;
            assert_eq!(decode_incoming(&changed), Err(Fault::InvalidEncoding));
        }
        encoded[101..109].fill(0);
        assert_eq!(decode_incoming(&encoded), Err(Fault::InvalidEncoding));
        let mut fact = encode_fact(&Fact {
            fingerprint: [1; 32],
            result: ResultFact::Published(RecordId([2; 32])),
        })
        .unwrap();
        fact[36] = 99;
        assert_eq!(decode_fact(&fact), Err(Fault::InvalidEncoding));
        let mut change = encode_change(&Change {
            cursor: ChangeCursor(1),
            operation: OperationId([1; 32]),
            kind: ChangeKind::Published(RecordId([2; 32])),
        })
        .unwrap();
        change[44] = 99;
        assert_eq!(decode_change(&change), Err(Fault::InvalidEncoding));
    }
}
