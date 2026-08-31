//! The routing-table region: every route row with the announce record that vouches for it and its replay ring.
//! The codec carries rows verbatim and verifies nothing — seeding re-derives the address binding and re-checks the Ed25519 signature, so storage never has to be trusted.

use super::envelope::{
    open_snapshot, seal_snapshot_in_place, SnapshotSealError, SNAPSHOT_HEADER_LEN,
    SNAPSHOT_OVERHEAD_LEN,
};
use super::{SnapshotReadError, SnapshotRegion};
use crate::crypto::{Ed25519PublicKey, Ed25519Signature, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentitySigningPublicKey};
use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use crate::routing::announce::{
    AnnounceId, DottedNameHash, IdentityPublicKeys, RatchetKey, ANNOUNCE_ID_WIRE_LEN,
};
use crate::routing::routes::RouteEntry;
use crate::routing::{AnnounceIdRing, NextHop, PersistedRouteRow, RouteResponsiveness};
use crate::units::InstantMillis;
use crate::wire::{DestinationHash, TransportId};
use crate::wire::{
    DOTTED_NAME_HASH_BYTE_LEN, RATCHET_BYTE_LEN, SIGNATURE_BYTE_LEN, TRUNCATED_HASH_BYTE_LEN,
};

const ROW_COUNT_LEN: usize = 4;
const INSTANT_LEN: usize = 8;
const TAG_LEN: usize = 1;
const HOPS_LEN: usize = 1;
const APP_DATA_LEN_PREFIX_LEN: usize = 2;
const RING_COUNT_PREFIX_LEN: usize = 1;
const MIN_PERSISTED_ROUTE_ROW_WIRE_LEN: usize = TRUNCATED_HASH_BYTE_LEN
    + HOPS_LEN
    + INSTANT_LEN
    + INSTANT_LEN
    + TAG_LEN
    + INTERFACE_ID_LEN
    + TAG_LEN
    + X25519PublicKey::LEN
    + Ed25519PublicKey::LEN
    + DOTTED_NAME_HASH_BYTE_LEN
    + ANNOUNCE_ID_WIRE_LEN
    + TAG_LEN
    + SIGNATURE_BYTE_LEN
    + APP_DATA_LEN_PREFIX_LEN
    + RING_COUNT_PREFIX_LEN;

const RESPONSIVENESS_UNKNOWN_TAG: u8 = 0;
const RESPONSIVENESS_RESPONSIVE_TAG: u8 = 1;
const RESPONSIVENESS_UNRESPONSIVE_TAG: u8 = 2;
const NEXT_HOP_DIRECT_TAG: u8 = 0;
const NEXT_HOP_VIA_TAG: u8 = 1;
const RATCHET_ABSENT_TAG: u8 = 0;
const RATCHET_PRESENT_TAG: u8 = 1;

/// The ring's count prefix is one byte, so a ring deeper than 255 keeps only its newest 255 ids.
const RING_MAX_PERSISTED_IDS: usize = u8::MAX as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingTableSnapshotWriteError {
    BufferTooShort,
    AppDataOutgrewLengthPrefix,
}

impl From<SnapshotSealError> for RoutingTableSnapshotWriteError {
    fn from(SnapshotSealError::BufferTooShort: SnapshotSealError) -> Self {
        RoutingTableSnapshotWriteError::BufferTooShort
    }
}

pub fn persisted_route_row_wire_len(row: &PersistedRouteRow<'_>) -> usize {
    MIN_PERSISTED_ROUTE_ROW_WIRE_LEN
        + match row.entry.next_hop {
            NextHop::Direct => 0,
            NextHop::Via(_) => TRUNCATED_HASH_BYTE_LEN,
        }
        + match row.ratchet {
            None => 0,
            Some(_) => RATCHET_BYTE_LEN,
        }
        + row.app_data.len()
        + row.announce_id_ring.len().min(RING_MAX_PERSISTED_IDS) * ANNOUNCE_ID_WIRE_LEN
}

#[must_use]
pub const fn maximum_persisted_route_row_wire_len(
    app_data_len: usize,
    announce_history_depth: usize,
) -> usize {
    let persisted_history_depth = if announce_history_depth < RING_MAX_PERSISTED_IDS {
        announce_history_depth
    } else {
        RING_MAX_PERSISTED_IDS
    };
    MIN_PERSISTED_ROUTE_ROW_WIRE_LEN
        + TRUNCATED_HASH_BYTE_LEN
        + RATCHET_BYTE_LEN
        + app_data_len
        + persisted_history_depth * ANNOUNCE_ID_WIRE_LEN
}

#[must_use]
pub const fn maximum_route_upsert_payload_len(
    app_data_len: usize,
    announce_history_depth: usize,
) -> usize {
    SNAPSHOT_OVERHEAD_LEN
        + ROW_COUNT_LEN
        + maximum_persisted_route_row_wire_len(app_data_len, announce_history_depth)
}

pub fn routing_table_snapshot_len<'a>(rows: impl Iterator<Item = PersistedRouteRow<'a>>) -> usize {
    SNAPSHOT_OVERHEAD_LEN
        + ROW_COUNT_LEN
        + rows
            .map(|row| persisted_route_row_wire_len(&row))
            .sum::<usize>()
}

pub fn write_routing_table_snapshot<'a>(
    rows: impl Iterator<Item = PersistedRouteRow<'a>>,
    out: &mut [u8],
) -> Result<usize, RoutingTableSnapshotWriteError> {
    #[cfg(feature = "parallel-persistence")]
    if super::should_parallelize_persistence(out.len()) {
        return write_routing_table_snapshot_parallel(rows.collect(), out);
    }
    write_routing_table_snapshot_serial(rows, out)
}

fn write_routing_table_snapshot_serial<'a>(
    rows: impl Iterator<Item = PersistedRouteRow<'a>>,
    out: &mut [u8],
) -> Result<usize, RoutingTableSnapshotWriteError> {
    let payload_start = SNAPSHOT_HEADER_LEN + ROW_COUNT_LEN;
    if out.len() < payload_start {
        return Err(RoutingTableSnapshotWriteError::BufferTooShort);
    }
    let mut at = payload_start;
    let mut row_count: u32 = 0;
    for row in rows {
        if row.app_data.len() > u16::MAX as usize {
            return Err(RoutingTableSnapshotWriteError::AppDataOutgrewLengthPrefix);
        }
        let row_len = persisted_route_row_wire_len(&row);
        if out.len() < at + row_len {
            return Err(RoutingTableSnapshotWriteError::BufferTooShort);
        }
        at += write_row(&row, &mut out[at..at + row_len]);
        row_count += 1;
    }
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&row_count.to_le_bytes());
    let payload_len = at - SNAPSHOT_HEADER_LEN;
    Ok(seal_snapshot_in_place(
        SnapshotRegion::RoutingTable,
        payload_len,
        out,
    )?)
}

#[cfg(feature = "parallel-persistence")]
fn write_routing_table_snapshot_parallel(
    rows: std::vec::Vec<PersistedRouteRow<'_>>,
    out: &mut [u8],
) -> Result<usize, RoutingTableSnapshotWriteError> {
    use rayon::prelude::*;

    let payload_start = SNAPSHOT_HEADER_LEN + ROW_COUNT_LEN;
    let mut at = payload_start;
    let mut row_lengths = std::vec::Vec::with_capacity(rows.len());
    for row in &rows {
        if row.app_data.len() > u16::MAX as usize {
            return Err(RoutingTableSnapshotWriteError::AppDataOutgrewLengthPrefix);
        }
        let row_len = persisted_route_row_wire_len(row);
        at = at
            .checked_add(row_len)
            .ok_or(RoutingTableSnapshotWriteError::BufferTooShort)?;
        row_lengths.push(row_len);
    }
    if out.len() < at {
        return Err(RoutingTableSnapshotWriteError::BufferTooShort);
    }
    let row_count =
        u32::try_from(rows.len()).map_err(|_| RoutingTableSnapshotWriteError::BufferTooShort)?;
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&row_count.to_le_bytes());
    let mut rest = &mut out[payload_start..at];
    let mut jobs = std::vec::Vec::with_capacity(rows.len());
    for (row, row_len) in rows.iter().zip(row_lengths) {
        let current = core::mem::take(&mut rest);
        let (row_out, tail) = current.split_at_mut(row_len);
        rest = tail;
        jobs.push((row, row_out));
    }
    jobs.into_par_iter().for_each(|(row, row_out)| {
        let _ = write_row(row, row_out);
    });
    Ok(seal_snapshot_in_place(
        SnapshotRegion::RoutingTable,
        at - SNAPSHOT_HEADER_LEN,
        out,
    )?)
}

fn write_row(row: &PersistedRouteRow<'_>, buf: &mut [u8]) -> usize {
    let mut at = 0;
    let mut put = |bytes: &[u8], at: &mut usize| {
        buf[*at..*at + bytes.len()].copy_from_slice(bytes);
        *at += bytes.len();
    };
    put(row.destination.as_bytes(), &mut at);
    put(&[row.entry.hops], &mut at);
    put(&row.entry.learned_at.0.to_le_bytes(), &mut at);
    put(&row.entry.last_route_activity_at.0.to_le_bytes(), &mut at);
    let responsiveness_tag = match row.entry.responsiveness {
        RouteResponsiveness::Unknown => RESPONSIVENESS_UNKNOWN_TAG,
        RouteResponsiveness::Responsive => RESPONSIVENESS_RESPONSIVE_TAG,
        RouteResponsiveness::Unresponsive => RESPONSIVENESS_UNRESPONSIVE_TAG,
    };
    put(&[responsiveness_tag], &mut at);
    put(row.entry.receiving_interface.as_bytes(), &mut at);
    match row.entry.next_hop {
        NextHop::Direct => put(&[NEXT_HOP_DIRECT_TAG], &mut at),
        NextHop::Via(transport_id) => {
            put(&[NEXT_HOP_VIA_TAG], &mut at);
            put(transport_id.as_bytes(), &mut at);
        }
    }
    put(row.public_keys.encryption.as_bytes(), &mut at);
    put(row.public_keys.signing.as_bytes(), &mut at);
    put(row.dotted_name_hash.as_bytes(), &mut at);
    put(&row.announce_id.to_wire_bytes(), &mut at);
    match &row.ratchet {
        None => put(&[RATCHET_ABSENT_TAG], &mut at),
        Some(ratchet) => {
            put(&[RATCHET_PRESENT_TAG], &mut at);
            put(ratchet.as_bytes(), &mut at);
        }
    }
    put(&row.signature.0, &mut at);
    put(&(row.app_data.len() as u16).to_le_bytes(), &mut at);
    put(row.app_data, &mut at);
    let ring_len = row.announce_id_ring.len();
    let persisted_ids = ring_len.min(RING_MAX_PERSISTED_IDS);
    put(&[persisted_ids as u8], &mut at);
    for id in row.announce_id_ring.ids().skip(ring_len - persisted_ids) {
        put(&id.to_wire_bytes(), &mut at);
    }
    at
}

pub fn read_routing_table_snapshot(
    bytes: &[u8],
) -> Result<PersistedRouteRows<'_>, SnapshotReadError> {
    let payload =
        open_snapshot(SnapshotRegion::RoutingTable, bytes).map_err(SnapshotReadError::Envelope)?;
    let Some((row_count_bytes, rows)) = payload.split_first_chunk::<ROW_COUNT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    let remaining_rows = u32::from_le_bytes(*row_count_bytes);
    let remaining_rows_usize =
        usize::try_from(remaining_rows).map_err(|_| SnapshotReadError::MalformedPayload)?;
    if remaining_rows_usize > rows.len() / MIN_PERSISTED_ROUTE_ROW_WIRE_LEN {
        return Err(SnapshotReadError::MalformedPayload);
    }
    Ok(PersistedRouteRows {
        rest: rows,
        remaining_rows,
        poisoned: false,
    })
}

/// Yields rows in stored order; the first malformed row poisons the iterator, and payload bytes past the declared row count are refused rather than ignored.
#[derive(Debug, Clone)]
pub struct PersistedRouteRows<'a> {
    rest: &'a [u8],
    remaining_rows: u32,
    poisoned: bool,
}

impl PersistedRouteRows<'_> {
    pub fn remaining_row_count(&self) -> u32 {
        self.remaining_rows
    }
}

impl<'a> Iterator for PersistedRouteRows<'a> {
    type Item = Result<PersistedRouteRow<'a>, SnapshotReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.poisoned {
            return None;
        }
        if self.remaining_rows == 0 {
            if self.rest.is_empty() {
                return None;
            }
            self.poisoned = true;
            return Some(Err(SnapshotReadError::MalformedPayload));
        }
        match parse_row(self.rest) {
            Some((row, rest)) => {
                self.rest = rest;
                self.remaining_rows -= 1;
                Some(Ok(row))
            }
            None => {
                self.poisoned = true;
                self.remaining_rows = 0;
                Some(Err(SnapshotReadError::MalformedPayload))
            }
        }
    }
}

fn parse_row(bytes: &[u8]) -> Option<(PersistedRouteRow<'_>, &[u8])> {
    let (destination, rest) = bytes.split_first_chunk::<TRUNCATED_HASH_BYTE_LEN>()?;
    let (&[hops], rest) = rest.split_first_chunk::<HOPS_LEN>()?;
    let (learned_at, rest) = rest.split_first_chunk::<INSTANT_LEN>()?;
    let (last_route_activity_at, rest) = rest.split_first_chunk::<INSTANT_LEN>()?;
    let (&[responsiveness_tag], rest) = rest.split_first_chunk::<TAG_LEN>()?;
    let responsiveness = match responsiveness_tag {
        RESPONSIVENESS_UNKNOWN_TAG => RouteResponsiveness::Unknown,
        RESPONSIVENESS_RESPONSIVE_TAG => RouteResponsiveness::Responsive,
        RESPONSIVENESS_UNRESPONSIVE_TAG => RouteResponsiveness::Unresponsive,
        _ => return None,
    };
    let (receiving_interface, rest) = rest.split_first_chunk::<INTERFACE_ID_LEN>()?;
    let (&[next_hop_tag], rest) = rest.split_first_chunk::<TAG_LEN>()?;
    let (next_hop, rest) = match next_hop_tag {
        NEXT_HOP_DIRECT_TAG => (NextHop::Direct, rest),
        NEXT_HOP_VIA_TAG => {
            let (transport_id, rest) = rest.split_first_chunk::<TRUNCATED_HASH_BYTE_LEN>()?;
            (NextHop::Via(TransportId::new(*transport_id)), rest)
        }
        _ => return None,
    };
    let (encryption, rest) = rest.split_first_chunk::<{ X25519PublicKey::LEN }>()?;
    let (signing, rest) = rest.split_first_chunk::<{ Ed25519PublicKey::LEN }>()?;
    let (dotted_name_hash, rest) = rest.split_first_chunk::<DOTTED_NAME_HASH_BYTE_LEN>()?;
    let (announce_id, rest) = rest.split_first_chunk::<ANNOUNCE_ID_WIRE_LEN>()?;
    let (&[ratchet_tag], rest) = rest.split_first_chunk::<TAG_LEN>()?;
    let (ratchet, rest) = match ratchet_tag {
        RATCHET_ABSENT_TAG => (None, rest),
        RATCHET_PRESENT_TAG => {
            let (ratchet, rest) = rest.split_first_chunk::<RATCHET_BYTE_LEN>()?;
            (Some(RatchetKey::new(*ratchet)), rest)
        }
        _ => return None,
    };
    let (signature, rest) = rest.split_first_chunk::<SIGNATURE_BYTE_LEN>()?;
    let (app_data_bytes, rest) = rest.split_first_chunk::<APP_DATA_LEN_PREFIX_LEN>()?;
    let app_data_bytes = u16::from_le_bytes(*app_data_bytes) as usize;
    if rest.len() < app_data_bytes {
        return None;
    }
    let (app_data, rest) = rest.split_at(app_data_bytes);
    let (&[ring_count], rest) = rest.split_first_chunk::<RING_COUNT_PREFIX_LEN>()?;
    let ring_bytes_len = ring_count as usize * ANNOUNCE_ID_WIRE_LEN;
    if rest.len() < ring_bytes_len {
        return None;
    }
    let (ring_bytes, rest) = rest.split_at(ring_bytes_len);
    let row = PersistedRouteRow {
        destination: DestinationHash::new(*destination),
        entry: RouteEntry {
            hops,
            learned_at: InstantMillis(u64::from_le_bytes(*learned_at)),
            last_route_activity_at: InstantMillis(u64::from_le_bytes(*last_route_activity_at)),
            responsiveness,
            receiving_interface: InterfaceId::new(*receiving_interface),
            next_hop,
            retention: crate::routing::RouteRetention::Network,
        },
        public_keys: IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey(*encryption)),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey(*signing)),
        },
        dotted_name_hash: DottedNameHash::new(*dotted_name_hash),
        announce_id: AnnounceId::from_wire(*announce_id),
        ratchet,
        signature: Ed25519Signature(*signature),
        app_data,
        announce_id_ring: AnnounceIdRing::Wire(ring_bytes),
    };
    Some((row, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn ring_ids(tags: &[u8]) -> Vec<AnnounceId> {
        tags.iter()
            .map(|&tag| AnnounceId::from_wire([tag; ANNOUNCE_ID_WIRE_LEN]))
            .collect()
    }

    fn row<'a>(
        seed: u8,
        ratchet: Option<RatchetKey>,
        app_data: &'a [u8],
        ring: &'a [AnnounceId],
    ) -> PersistedRouteRow<'a> {
        PersistedRouteRow {
            destination: DestinationHash::new([seed; 16]),
            entry: RouteEntry {
                hops: seed,
                learned_at: InstantMillis(1_000 + u64::from(seed)),
                last_route_activity_at: InstantMillis(2_000 + u64::from(seed)),
                responsiveness: RouteResponsiveness::Responsive,
                receiving_interface: InterfaceId::new([seed; INTERFACE_ID_LEN]),
                next_hop: NextHop::Via(TransportId::new([seed.wrapping_add(1); 16])),
                retention: crate::routing::RouteRetention::Network,
            },
            public_keys: IdentityPublicKeys {
                encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([seed; 32])),
                signing: IdentitySigningPublicKey::new(Ed25519PublicKey([seed; 32])),
            },
            dotted_name_hash: DottedNameHash::new([seed; DOTTED_NAME_HASH_BYTE_LEN]),
            announce_id: AnnounceId::from_wire([seed; ANNOUNCE_ID_WIRE_LEN]),
            ratchet,
            signature: Ed25519Signature([seed; SIGNATURE_BYTE_LEN]),
            app_data,
            announce_id_ring: AnnounceIdRing::Table(ring),
        }
    }

    fn assert_rows_equal(read: &PersistedRouteRow<'_>, written: &PersistedRouteRow<'_>) {
        assert_eq!(read.destination, written.destination);
        assert_eq!(read.entry, written.entry);
        assert_eq!(read.public_keys, written.public_keys);
        assert_eq!(read.dotted_name_hash, written.dotted_name_hash);
        assert_eq!(read.announce_id, written.announce_id);
        assert_eq!(read.ratchet, written.ratchet);
        assert_eq!(read.signature, written.signature);
        assert_eq!(read.app_data, written.app_data);
        assert_eq!(
            read.announce_id_ring.ids().collect::<Vec<_>>(),
            written.announce_id_ring.ids().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn a_mixed_set_of_rows_round_trips() {
        let ring_a = ring_ids(&[1, 2, 3]);
        let ring_b = ring_ids(&[]);
        let rows = [
            row(
                0xA1,
                Some(RatchetKey::new([0x5E; 32])),
                b"app data",
                &ring_a,
            ),
            row(0xB2, None, b"", &ring_b),
        ];

        let mut out = std::vec![0u8; routing_table_snapshot_len(rows.iter().cloned())];
        let len = write_routing_table_snapshot(rows.iter().cloned(), &mut out).unwrap();
        assert_eq!(len, out.len());

        let read: Vec<_> = read_routing_table_snapshot(&out[..len])
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(read.len(), rows.len());
        for (read, written) in read.iter().zip(rows.iter()) {
            assert_rows_equal(read, written);
        }
    }

    #[cfg(feature = "parallel-persistence")]
    #[test]
    fn parallel_and_serial_writers_are_byte_identical() {
        let ring = ring_ids(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let prototype = row(0x6A, Some(RatchetKey::new([0xA5; 32])), &[0xC3; 96], &ring);
        let rows = core::iter::repeat_n(prototype, 2_048).collect::<Vec<_>>();
        let len = routing_table_snapshot_len(rows.iter().cloned());
        assert!(len >= super::super::PARALLEL_PERSISTENCE_MIN_BYTES);
        let mut parallel = std::vec![0u8; len];
        let mut serial = std::vec![0u8; len];
        let parallel_len =
            write_routing_table_snapshot_parallel(rows.clone(), &mut parallel).unwrap();
        let serial_len =
            write_routing_table_snapshot_serial(rows.iter().cloned(), &mut serial).unwrap();
        assert_eq!(parallel_len, serial_len);
        assert_eq!(parallel, serial);
    }

    #[test]
    fn an_empty_table_round_trips_to_no_rows() {
        let mut out = [0u8; SNAPSHOT_OVERHEAD_LEN + ROW_COUNT_LEN];
        let len = write_routing_table_snapshot(core::iter::empty(), &mut out).unwrap();
        assert_eq!(read_routing_table_snapshot(&out[..len]).unwrap().count(), 0,);
    }

    #[test]
    fn a_ring_deeper_than_the_count_prefix_keeps_the_newest_ids() {
        let deep_ring: Vec<AnnounceId> = (0..300u16)
            .map(|n| {
                let mut bytes = [0u8; ANNOUNCE_ID_WIRE_LEN];
                bytes[..2].copy_from_slice(&n.to_le_bytes());
                AnnounceId::from_wire(bytes)
            })
            .collect();
        let rows = [row(0xC3, None, b"", &deep_ring)];

        let mut out = std::vec![0u8; routing_table_snapshot_len(rows.iter().cloned())];
        let len = write_routing_table_snapshot(rows.iter().cloned(), &mut out).unwrap();
        let read: Vec<_> = read_routing_table_snapshot(&out[..len])
            .unwrap()
            .map(|row| row.unwrap())
            .collect();

        let read_ids: Vec<_> = read[0].announce_id_ring.ids().collect();
        assert_eq!(read_ids.len(), RING_MAX_PERSISTED_IDS);
        assert_eq!(read_ids, deep_ring[300 - RING_MAX_PERSISTED_IDS..]);
    }

    #[test]
    fn a_truncated_row_poisons_the_iterator() {
        let ring = ring_ids(&[7]);
        let rows = [row(0xD4, None, b"tail", &ring), row(0xE5, None, b"", &ring)];
        let mut out = std::vec![0u8; routing_table_snapshot_len(rows.iter().cloned())];
        let len = write_routing_table_snapshot(rows.iter().cloned(), &mut out).unwrap();

        let mut cut = out[..len].to_vec();
        let removed = 12;
        cut.truncate(len - SNAPSHOT_OVERHEAD_LEN + SNAPSHOT_HEADER_LEN - removed);
        let payload_len = (cut.len() - SNAPSHOT_HEADER_LEN) as u32;
        cut[6..10].copy_from_slice(&payload_len.to_le_bytes());
        let resealed_len = {
            let payload = cut[SNAPSHOT_HEADER_LEN..].to_vec();
            let mut resealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
            let n = super::super::envelope::seal_snapshot(
                SnapshotRegion::RoutingTable,
                &payload,
                &mut resealed,
            )
            .unwrap();
            cut = resealed;
            n
        };

        let mut reader = read_routing_table_snapshot(&cut[..resealed_len]).unwrap();
        assert!(reader.next().unwrap().is_ok());
        assert!(matches!(
            reader.next().unwrap(),
            Err(SnapshotReadError::MalformedPayload)
        ));
        assert_eq!(reader.remaining_row_count(), 0);
        assert!(reader.next().is_none(), "a poisoned reader stays silent");
    }

    #[test]
    fn payload_bytes_past_the_declared_row_count_are_refused() {
        let ring = ring_ids(&[]);
        let rows = [row(0xF6, None, b"", &ring)];
        let mut payload = std::vec![0u8; ROW_COUNT_LEN];
        payload[..ROW_COUNT_LEN].copy_from_slice(&0u32.to_le_bytes());
        let mut row_bytes = std::vec![0u8; persisted_route_row_wire_len(&rows[0])];
        write_row(&rows[0], &mut row_bytes);
        payload.extend_from_slice(&row_bytes);

        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::RoutingTable,
            &payload,
            &mut sealed,
        )
        .unwrap();

        let mut reader = read_routing_table_snapshot(&sealed[..len]).unwrap();
        assert!(matches!(
            reader.next().unwrap(),
            Err(SnapshotReadError::MalformedPayload)
        ));
    }

    #[test]
    fn a_declared_row_count_that_cannot_fit_the_payload_is_refused() {
        let mut payload = std::vec![0u8; ROW_COUNT_LEN];
        payload.copy_from_slice(&u32::MAX.to_le_bytes());
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::RoutingTable,
            &payload,
            &mut sealed,
        )
        .unwrap();

        assert!(matches!(
            read_routing_table_snapshot(&sealed[..len]),
            Err(SnapshotReadError::MalformedPayload)
        ));
    }

    #[test]
    fn an_unknown_tag_refuses_the_row() {
        let ring = ring_ids(&[]);
        let rows = [row(0x1B, None, b"", &ring)];
        let mut out = std::vec![0u8; routing_table_snapshot_len(rows.iter().cloned())];
        write_routing_table_snapshot(rows.iter().cloned(), &mut out).unwrap();

        let responsiveness_at = SNAPSHOT_HEADER_LEN
            + ROW_COUNT_LEN
            + TRUNCATED_HASH_BYTE_LEN
            + HOPS_LEN
            + INSTANT_LEN
            + INSTANT_LEN;
        let mut payload = out[SNAPSHOT_HEADER_LEN..].to_vec();
        payload.truncate(payload.len() - super::super::envelope::SNAPSHOT_CHECKSUM_LEN);
        payload[responsiveness_at - SNAPSHOT_HEADER_LEN] = 0x7F;
        let mut resealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::RoutingTable,
            &payload,
            &mut resealed,
        )
        .unwrap();

        let mut reader = read_routing_table_snapshot(&resealed[..len]).unwrap();
        assert!(matches!(
            reader.next().unwrap(),
            Err(SnapshotReadError::MalformedPayload)
        ));
    }

    #[test]
    fn a_short_buffer_is_refused() {
        let ring = ring_ids(&[1]);
        let rows = [row(0x2C, None, b"payload", &ring)];
        let exact = routing_table_snapshot_len(rows.iter().cloned());
        let mut short = std::vec![0u8; exact - 1];
        assert_eq!(
            write_routing_table_snapshot(rows.iter().cloned(), &mut short),
            Err(RoutingTableSnapshotWriteError::BufferTooShort),
        );
    }
}
