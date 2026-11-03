//! Provisional service record and receipt encoding.
//! Authentication and binding to the MLS sender, group and control are external.

use alloc::vec::Vec;

use prns_groups_core::{
    AuthorizationEra, ControlId, ControlReference, DurableReceipt, Endpoint, EndpointId,
    EndpointIncarnation, GroupId, NamespaceId, PublicationMetadata, PublicationValidity, RecordId,
    SchemaId, Timestamp,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{Fault, Limits, Publication, Record};

const RECORD_MAGIC: &[u8; 4] = b"PGS\x01";
const RECEIPT_MAGIC: &[u8; 4] = b"PGS\x02";
const RECORD: u8 = 1;
const RECEIPT: u8 = 2;
const ENDPOINT_BYTES: usize = 40;
const RECORD_FIXED_BYTES: usize = 205;
const RECEIPT_BYTES: usize = 117;
const RECORD_DOMAIN: &[u8] = b"prns.groups.service.record.v1\0";

/// Exact plaintext size of a canonical record, without allocating or encoding.
///
/// This checks the wire representation's count and length fields. Target order,
/// validity and selected service quotas are checked when encoding the record.
pub fn encoded_record_len(target_count: usize, body_bytes: usize) -> Result<usize, Fault> {
    if target_count == 0 {
        return Err(Fault::InvalidTargets);
    }
    u32::try_from(target_count).map_err(|_| Fault::Capacity)?;
    u32::try_from(body_bytes).map_err(|_| Fault::Capacity)?;
    target_count
        .checked_mul(ENDPOINT_BYTES)
        .and_then(|targets| targets.checked_add(RECORD_FIXED_BYTES))
        .and_then(|header| header.checked_add(body_bytes))
        .ok_or(Fault::Capacity)
}

pub(crate) enum Packet {
    Record(Record),
    Receipt {
        receipt: DurableReceipt,
        deadline: Timestamp,
    },
}

pub(crate) fn encode_record(
    group: GroupId,
    control: ControlReference,
    author: Endpoint,
    request: &Publication<'_>,
    limits: Limits,
) -> Result<Zeroizing<Vec<u8>>, Fault> {
    validate_limits(&limits)?;
    if request.targets.is_empty() || request.targets.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Fault::InvalidTargets);
    }
    if request.targets.len() > limits.max_targets || request.body.len() > limits.max_body_bytes {
        return Err(Fault::Capacity);
    }
    PublicationValidity::new(
        request.validity.created_at(),
        request.validity.deadline(),
        limits.maximum_window,
    )
    .map_err(|_| Fault::InvalidValidity)?;
    let capacity = encoded_record_len(request.targets.len(), request.body.len())?;
    let target_count = u32::try_from(request.targets.len()).map_err(|_| Fault::Capacity)?;
    let body_length = u32::try_from(request.body.len()).map_err(|_| Fault::Capacity)?;
    let mut encoded = Zeroizing::new(Vec::new());
    encoded
        .try_reserve_exact(capacity)
        .map_err(|_| Fault::Allocation)?;
    encoded.extend_from_slice(RECORD_MAGIC);
    encoded.push(RECORD);
    encoded.extend_from_slice(&group.0);
    append_endpoint(&mut encoded, author);
    encoded.extend_from_slice(&request.namespace.0.to_be_bytes());
    encoded.extend_from_slice(&request.schema.0.to_be_bytes());
    encoded.extend_from_slice(&control.head.0);
    encoded.extend_from_slice(&control.era.0 .0);
    encoded.extend_from_slice(&request.validity.created_at().0.to_be_bytes());
    encoded.extend_from_slice(&request.validity.deadline().0.to_be_bytes());
    encoded.extend_from_slice(&request.nonce);
    encoded.extend_from_slice(&target_count.to_be_bytes());
    for target in request.targets {
        append_endpoint(&mut encoded, *target);
    }
    encoded.extend_from_slice(&body_length.to_be_bytes());
    encoded.extend_from_slice(request.body);
    Ok(encoded)
}

pub(crate) fn encode_receipt(
    receipt: &DurableReceipt,
    deadline: Timestamp,
) -> Result<Zeroizing<Vec<u8>>, Fault> {
    let mut encoded = Zeroizing::new(Vec::new());
    encoded
        .try_reserve_exact(RECEIPT_BYTES)
        .map_err(|_| Fault::Allocation)?;
    encoded.extend_from_slice(RECEIPT_MAGIC);
    encoded.push(RECEIPT);
    encoded.extend_from_slice(&receipt.group.0);
    encoded.extend_from_slice(&receipt.record.0);
    append_endpoint(&mut encoded, receipt.target);
    encoded.extend_from_slice(&deadline.0.to_be_bytes());
    Ok(encoded)
}

pub(crate) fn decode(bytes: &[u8], limits: Limits) -> Result<Packet, Fault> {
    let mut cursor = Cursor { bytes, position: 0 };
    let magic = cursor.array::<4>()?;
    match (magic, cursor.array::<1>()?[0]) {
        (magic, RECORD) if magic == *RECORD_MAGIC => decode_record(cursor, bytes, limits),
        (magic, RECEIPT) if magic == *RECEIPT_MAGIC => {
            let receipt = DurableReceipt {
                group: GroupId(cursor.array()?),
                record: RecordId(cursor.array()?),
                target: cursor.endpoint()?,
            };
            let deadline = Timestamp(cursor.u64()?);
            cursor.finish()?;
            Ok(Packet::Receipt { receipt, deadline })
        }
        _ => Err(Fault::InvalidEncoding),
    }
}

fn decode_record(mut cursor: Cursor<'_>, encoded: &[u8], limits: Limits) -> Result<Packet, Fault> {
    validate_limits(&limits)?;
    let group = GroupId(cursor.array()?);
    let author = cursor.endpoint()?;
    let namespace = NamespaceId(cursor.u32()?);
    let schema = SchemaId(cursor.u32()?);
    let original_control = ControlReference {
        head: ControlId(cursor.array()?),
        era: AuthorizationEra(ControlId(cursor.array()?)),
    };
    let validity = PublicationValidity::new(
        Timestamp(cursor.u64()?),
        Timestamp(cursor.u64()?),
        limits.maximum_window,
    )
    .map_err(|_| Fault::InvalidValidity)?;
    let nonce = cursor.array()?;
    let count = usize::try_from(cursor.u32()?).map_err(|_| Fault::Capacity)?;
    if count == 0 {
        return Err(Fault::InvalidTargets);
    }
    if count > limits.max_targets {
        return Err(Fault::Capacity);
    }
    let target_bytes = cursor.slice(count.checked_mul(ENDPOINT_BYTES).ok_or(Fault::Capacity)?)?;
    let body_length = usize::try_from(cursor.u32()?).map_err(|_| Fault::Capacity)?;
    if body_length > limits.max_body_bytes {
        return Err(Fault::Capacity);
    }
    let body_bytes = cursor.slice(body_length)?;
    cursor.finish()?;

    let mut targets = Vec::new();
    targets
        .try_reserve_exact(count)
        .map_err(|_| Fault::Allocation)?;
    let mut target_cursor = Cursor {
        bytes: target_bytes,
        position: 0,
    };
    for _ in 0..count {
        let target = target_cursor.endpoint()?;
        if targets.last().is_some_and(|previous| *previous >= target) {
            return Err(Fault::InvalidTargets);
        }
        targets.push(target);
    }
    let mut body = Zeroizing::new(Vec::new());
    body.try_reserve_exact(body_length)
        .map_err(|_| Fault::Allocation)?;
    body.extend_from_slice(body_bytes);
    let mut digest = Sha256::new();
    digest.update(RECORD_DOMAIN);
    digest.update(encoded);
    Ok(Packet::Record(Record {
        metadata: PublicationMetadata {
            group,
            record: RecordId(digest.finalize().into()),
            author,
            namespace,
            schema,
            validity,
            original_control,
        },
        nonce,
        targets,
        body,
    }))
}

fn validate_limits(limits: &Limits) -> Result<(), Fault> {
    if limits.max_targets == 0
        || u32::try_from(limits.max_targets).is_err()
        || u32::try_from(limits.max_body_bytes).is_err()
        || limits.maximum_window.0 == 0
    {
        return Err(Fault::Configuration);
    }
    Ok(())
}

fn append_endpoint(encoded: &mut Vec<u8>, endpoint: Endpoint) {
    encoded.extend_from_slice(&endpoint.id.0);
    encoded.extend_from_slice(&endpoint.incarnation.get().to_be_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn slice(&mut self, length: usize) -> Result<&'a [u8], Fault> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(Fault::InvalidEncoding)?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or(Fault::InvalidEncoding)?;
        self.position = end;
        Ok(result)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Fault> {
        self.slice(N)?
            .try_into()
            .map_err(|_| Fault::InvalidEncoding)
    }

    fn u32(&mut self) -> Result<u32, Fault> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, Fault> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn endpoint(&mut self) -> Result<Endpoint, Fault> {
        Ok(Endpoint {
            id: EndpointId(self.array()?),
            incarnation: EndpointIncarnation::new(self.u64()?)
                .map_err(|_| Fault::InvalidEncoding)?,
        })
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
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use prns_groups_core::Seconds;

    fn limits() -> Limits {
        Limits {
            max_records: 8,
            max_targets: 4,
            max_body_bytes: 64,
            max_changes: 8,
            max_envelopes: 16,
            max_retained_wire_bytes: 65_536,
            maximum_window: Seconds(100),
            future_skew: Seconds(0),
        }
    }

    fn endpoint(value: u8, incarnation: u64) -> Endpoint {
        Endpoint {
            id: EndpointId([value; 32]),
            incarnation: EndpointIncarnation::new(incarnation).unwrap(),
        }
    }

    fn control() -> ControlReference {
        ControlReference {
            head: ControlId([5; 32]),
            era: AuthorizationEra(ControlId([6; 32])),
        }
    }

    fn publication<'a>(targets: &'a [Endpoint], body: &'a [u8]) -> Publication<'a> {
        Publication {
            nonce: [7; 32],
            namespace: NamespaceId(8),
            schema: SchemaId(9),
            validity: PublicationValidity::new(Timestamp(100), Timestamp(200), Seconds(100))
                .unwrap(),
            targets,
            body,
        }
    }

    fn encoded_record() -> Zeroizing<Vec<u8>> {
        encode_record(
            GroupId([1; 32]),
            control(),
            endpoint(2, 3),
            &publication(&[endpoint(3, 1), endpoint(4, 1)], b"hello"),
            limits(),
        )
        .unwrap()
    }

    #[test]
    fn size_preflight_matches_real_records_across_member_counts() {
        for count in [1usize, 8, 256] {
            let targets = (0..count)
                .map(|index| endpoint(3, u64::try_from(index).unwrap() + 1))
                .collect::<Vec<_>>();
            for body_bytes in [0usize, 64, 2054] {
                let body = alloc::vec![42; body_bytes];
                let selected = Limits {
                    max_targets: count,
                    max_body_bytes: body_bytes,
                    ..limits()
                };
                let encoded = encode_record(
                    GroupId([1; 32]),
                    control(),
                    endpoint(2, 3),
                    &publication(&targets, &body),
                    selected,
                )
                .unwrap();
                assert_eq!(
                    encoded_record_len(count, body_bytes).unwrap(),
                    encoded.len()
                );
                let Packet::Record(record) = decode(&encoded, selected).unwrap() else {
                    panic!("record decoded as receipt");
                };
                assert_eq!(record.targets, targets);
                assert_eq!(record.body.as_slice(), body.as_slice());
            }
        }
        assert_eq!(encoded_record_len(0, 0), Err(Fault::InvalidTargets));
        assert_eq!(encoded_record_len(usize::MAX, 0), Err(Fault::Capacity));
        assert_eq!(encoded_record_len(1, usize::MAX), Err(Fault::Capacity));
    }

    #[test]
    fn canonical_record_round_trip_binds_every_byte_to_its_identity() {
        let encoded = encoded_record();
        assert_eq!(encoded.len(), RECORD_FIXED_BYTES + 2 * ENDPOINT_BYTES + 5);
        let Packet::Record(record) = decode(&encoded, limits()).unwrap() else {
            panic!("record decoded as receipt");
        };
        assert_eq!(record.metadata.group, GroupId([1; 32]));
        assert_eq!(
            record.metadata.record,
            RecordId([
                96, 207, 122, 28, 130, 68, 162, 170, 127, 254, 217, 83, 73, 214, 117, 90, 180, 106,
                151, 186, 222, 84, 140, 61, 131, 121, 233, 3, 211, 167, 8, 166,
            ])
        );
        assert_eq!(record.metadata.author, endpoint(2, 3));
        assert_eq!(record.metadata.namespace, NamespaceId(8));
        assert_eq!(record.metadata.schema, SchemaId(9));
        assert_eq!(record.metadata.original_control, control());
        assert_eq!(record.metadata.validity.created_at(), Timestamp(100));
        assert_eq!(record.metadata.validity.deadline(), Timestamp(200));
        assert_eq!(record.nonce, [7; 32]);
        assert_eq!(record.targets, [endpoint(3, 1), endpoint(4, 1)]);
        assert_eq!(record.body.as_slice(), b"hello");
        let request = Publication {
            nonce: record.nonce,
            namespace: record.metadata.namespace,
            schema: record.metadata.schema,
            validity: record.metadata.validity,
            targets: &record.targets,
            body: &record.body,
        };
        assert_eq!(
            encode_record(
                record.metadata.group,
                record.metadata.original_control,
                record.metadata.author,
                &request,
                limits()
            )
            .unwrap(),
            encoded
        );
        let mut changed = encoded;
        *changed.last_mut().unwrap() ^= 1;
        let Packet::Record(changed) = decode(&changed, limits()).unwrap() else {
            panic!("record decoded as receipt");
        };
        assert_ne!(changed.metadata.record, record.metadata.record);
    }

    #[test]
    fn receipt_round_trip_is_exact_and_preserves_incarnation() {
        let receipt = DurableReceipt {
            group: GroupId([1; 32]),
            record: RecordId([2; 32]),
            target: endpoint(3, u64::MAX),
        };
        let mut encoded = encode_receipt(&receipt, Timestamp(300)).unwrap();
        assert_eq!(encoded.len(), RECEIPT_BYTES);
        let Packet::Receipt {
            receipt: decoded,
            deadline,
        } = decode(&encoded, limits()).unwrap()
        else {
            panic!("receipt decoded as record");
        };
        assert_eq!(decoded, receipt);
        assert_eq!(deadline, Timestamp(300));
        let mut unsupported = encoded[..RECEIPT_BYTES - 8].to_vec();
        unsupported[3] = 1;
        assert!(matches!(
            decode(&unsupported, limits()),
            Err(Fault::InvalidEncoding)
        ));
        encoded.push(0);
        assert!(matches!(
            decode(&encoded, limits()),
            Err(Fault::InvalidEncoding)
        ));
    }

    #[test]
    fn every_truncation_unknown_header_and_trailing_byte_is_rejected() {
        let encoded = encoded_record();
        for length in 0..encoded.len() {
            assert!(decode(&encoded[..length], limits()).is_err());
        }
        for (offset, value) in [(0, b'X'), (3, 2), (4, 99)] {
            let mut invalid = encoded.clone();
            invalid[offset] = value;
            assert!(matches!(
                decode(&invalid, limits()),
                Err(Fault::InvalidEncoding)
            ));
        }
        let mut invalid = encoded;
        invalid.push(0);
        assert!(matches!(
            decode(&invalid, limits()),
            Err(Fault::InvalidEncoding)
        ));
    }

    #[test]
    fn targets_must_be_nonempty_unique_and_canonically_ordered() {
        for targets in [
            &[][..],
            &[endpoint(3, 1), endpoint(3, 1)][..],
            &[endpoint(4, 1), endpoint(3, 1)][..],
        ] {
            assert!(matches!(
                encode_record(
                    GroupId([1; 32]),
                    control(),
                    endpoint(2, 3),
                    &publication(targets, b"hello"),
                    limits(),
                ),
                Err(Fault::InvalidTargets)
            ));
        }
        let mut duplicate = encoded_record();
        duplicate.copy_within(201..241, 241);
        assert!(matches!(
            decode(&duplicate, limits()),
            Err(Fault::InvalidTargets)
        ));
        let mut reversed = encoded_record();
        reversed[201] = 5;
        assert!(matches!(
            decode(&reversed, limits()),
            Err(Fault::InvalidTargets)
        ));
        let mut zero_incarnation = encoded_record();
        zero_incarnation[233..241].fill(0);
        assert!(matches!(
            decode(&zero_incarnation, limits()),
            Err(Fault::InvalidEncoding)
        ));
    }

    #[test]
    fn untrusted_lengths_and_validity_are_checked_before_body_allocation() {
        let mut large_count = encoded_record();
        large_count[197..201].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            decode(&large_count, limits()),
            Err(Fault::Capacity)
        ));
        let mut large_body = encoded_record();
        large_body[281..285].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            decode(&large_body, limits()),
            Err(Fault::Capacity)
        ));
        let encoded = encoded_record();
        assert!(matches!(
            decode(
                &encoded,
                Limits {
                    maximum_window: Seconds(99),
                    ..limits()
                }
            ),
            Err(Fault::InvalidValidity)
        ));
        assert!(matches!(
            encode_record(
                GroupId([1; 32]),
                control(),
                endpoint(2, 3),
                &publication(&[endpoint(3, 1)], &[0; 65]),
                limits(),
            ),
            Err(Fault::Capacity)
        ));
    }
}
