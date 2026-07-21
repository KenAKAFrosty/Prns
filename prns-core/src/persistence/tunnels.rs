//! The tunnels region: tunnel id, the interface it last rode, and its expiry — three fixed-size columns per row.
//! RNS 1.4.0 `Transport.save_tunnel_table` nests every path row under its tunnel; ours nest nothing because routing rows persist their interface themselves — the seeded tunnel only has to warm that interface and catch the peer's next synthesize as a reappearance.
//! The codec carries rows verbatim and verifies nothing: a tunnel row holds no keys, so the worst a hostile store can plant is warmth on a dead interface, bounded by the row's own expiry.

use super::envelope::{
    open_snapshot, seal_snapshot_in_place, SnapshotSealError, SNAPSHOT_HEADER_LEN,
    SNAPSHOT_OVERHEAD_LEN,
};
use super::{SnapshotReadError, SnapshotRegion};
use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use crate::routing::tunnel::{PersistedTunnelRow, TunnelId};
use crate::units::InstantMillis;

const ROW_COUNT_LEN: usize = 4;
const TUNNEL_ID_LEN: usize = 32;
const INSTANT_LEN: usize = 8;

pub const TUNNEL_ROW_WIRE_LEN: usize = TUNNEL_ID_LEN + INTERFACE_ID_LEN + INSTANT_LEN;

pub fn tunnels_snapshot_len(row_count: usize) -> usize {
    SNAPSHOT_OVERHEAD_LEN + ROW_COUNT_LEN + row_count * TUNNEL_ROW_WIRE_LEN
}

pub fn write_tunnels_snapshot(
    rows: impl Iterator<Item = PersistedTunnelRow>,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    #[cfg(feature = "parallel-persistence")]
    if super::should_parallelize_persistence(out.len()) {
        return write_tunnels_snapshot_parallel(rows.collect(), out);
    }
    write_tunnels_snapshot_serial(rows, out)
}

fn write_tunnels_snapshot_serial(
    rows: impl Iterator<Item = PersistedTunnelRow>,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    let payload_start = SNAPSHOT_HEADER_LEN + ROW_COUNT_LEN;
    if out.len() < payload_start {
        return Err(SnapshotSealError::BufferTooShort);
    }
    let mut at = payload_start;
    let mut row_count: u32 = 0;
    for row in rows {
        if out.len() < at + TUNNEL_ROW_WIRE_LEN {
            return Err(SnapshotSealError::BufferTooShort);
        }
        write_tunnel_row(row, &mut out[at..at + TUNNEL_ROW_WIRE_LEN]);
        at += TUNNEL_ROW_WIRE_LEN;
        row_count += 1;
    }
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&row_count.to_le_bytes());
    seal_snapshot_in_place(SnapshotRegion::Tunnels, at - SNAPSHOT_HEADER_LEN, out)
}

fn write_tunnel_row(row: PersistedTunnelRow, out: &mut [u8]) {
    out[..TUNNEL_ID_LEN].copy_from_slice(row.tunnel_id.as_bytes());
    out[TUNNEL_ID_LEN..TUNNEL_ID_LEN + INTERFACE_ID_LEN].copy_from_slice(row.interface.as_bytes());
    out[TUNNEL_ID_LEN + INTERFACE_ID_LEN..TUNNEL_ROW_WIRE_LEN]
        .copy_from_slice(&row.expires_at.0.to_le_bytes());
}

#[cfg(feature = "parallel-persistence")]
fn write_tunnels_snapshot_parallel(
    rows: std::vec::Vec<PersistedTunnelRow>,
    out: &mut [u8],
) -> Result<usize, SnapshotSealError> {
    use rayon::prelude::*;

    let payload_start = SNAPSHOT_HEADER_LEN + ROW_COUNT_LEN;
    let rows_len = rows
        .len()
        .checked_mul(TUNNEL_ROW_WIRE_LEN)
        .ok_or(SnapshotSealError::BufferTooShort)?;
    let total_len = payload_start
        .checked_add(rows_len)
        .ok_or(SnapshotSealError::BufferTooShort)?;
    if out.len() < total_len {
        return Err(SnapshotSealError::BufferTooShort);
    }
    let row_count = u32::try_from(rows.len()).map_err(|_| SnapshotSealError::BufferTooShort)?;
    out[SNAPSHOT_HEADER_LEN..payload_start].copy_from_slice(&row_count.to_le_bytes());
    out[payload_start..total_len]
        .par_chunks_mut(TUNNEL_ROW_WIRE_LEN)
        .zip(rows.into_par_iter())
        .for_each(|(row_out, row)| write_tunnel_row(row, row_out));
    seal_snapshot_in_place(
        SnapshotRegion::Tunnels,
        total_len - SNAPSHOT_HEADER_LEN,
        out,
    )
}

pub fn read_tunnels_snapshot(bytes: &[u8]) -> Result<PersistedTunnelRows<'_>, SnapshotReadError> {
    let payload =
        open_snapshot(SnapshotRegion::Tunnels, bytes).map_err(SnapshotReadError::Envelope)?;
    let Some((row_count_bytes, rows)) = payload.split_first_chunk::<ROW_COUNT_LEN>() else {
        return Err(SnapshotReadError::MalformedPayload);
    };
    let row_count = u64::from(u32::from_le_bytes(*row_count_bytes));
    if rows.len() as u64 != row_count * TUNNEL_ROW_WIRE_LEN as u64 {
        return Err(SnapshotReadError::MalformedPayload);
    }
    Ok(PersistedTunnelRows { rest: rows })
}

/// Fixed-size rows let the whole payload validate at open, so iteration is infallible: a truncated row and bytes past the declared count are both refused before the first row is yielded.
#[derive(Debug, Clone)]
pub struct PersistedTunnelRows<'a> {
    rest: &'a [u8],
}

impl PersistedTunnelRows<'_> {
    pub fn row_count(&self) -> usize {
        self.rest.len() / TUNNEL_ROW_WIRE_LEN
    }
}

impl Iterator for PersistedTunnelRows<'_> {
    type Item = PersistedTunnelRow;

    fn next(&mut self) -> Option<Self::Item> {
        let (tunnel_id, rest) = self.rest.split_first_chunk::<TUNNEL_ID_LEN>()?;
        let (interface, rest) = rest.split_first_chunk::<INTERFACE_ID_LEN>()?;
        let (expires_at, rest) = rest.split_first_chunk::<INSTANT_LEN>()?;
        self.rest = rest;
        Some(PersistedTunnelRow {
            tunnel_id: TunnelId::new(*tunnel_id),
            interface: InterfaceId::new(*interface),
            expires_at: InstantMillis(u64::from_le_bytes(*expires_at)),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.row_count(), Some(self.row_count()))
    }
}

impl ExactSizeIterator for PersistedTunnelRows<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::SnapshotOpenError;
    use std::vec::Vec;

    fn row(seed: u8) -> PersistedTunnelRow {
        PersistedTunnelRow {
            tunnel_id: TunnelId::new([seed; TUNNEL_ID_LEN]),
            interface: InterfaceId::new([seed; INTERFACE_ID_LEN]),
            expires_at: InstantMillis(1_000 + u64::from(seed)),
        }
    }

    #[test]
    fn a_set_of_rows_round_trips() {
        let rows = [row(0xA1), row(0xB2), row(0xC3)];
        let mut out = std::vec![0u8; tunnels_snapshot_len(rows.len())];
        let len = write_tunnels_snapshot(rows.iter().copied(), &mut out).unwrap();
        assert_eq!(len, out.len());

        let reader = read_tunnels_snapshot(&out[..len]).unwrap();
        assert_eq!(reader.row_count(), rows.len());
        assert_eq!(reader.collect::<Vec<_>>(), rows);
    }

    #[cfg(feature = "parallel-persistence")]
    #[test]
    fn parallel_and_serial_writers_are_byte_identical() {
        let rows = (0..16_384)
            .map(|index| row(index as u8))
            .collect::<Vec<_>>();
        let len = tunnels_snapshot_len(rows.len());
        assert!(len >= super::super::PARALLEL_PERSISTENCE_MIN_BYTES);
        let mut parallel = std::vec![0u8; len];
        let mut serial = std::vec![0u8; len];
        let parallel_len = write_tunnels_snapshot_parallel(rows.clone(), &mut parallel).unwrap();
        let serial_len = write_tunnels_snapshot_serial(rows.iter().copied(), &mut serial).unwrap();
        assert_eq!(parallel_len, serial_len);
        assert_eq!(parallel, serial);
    }

    #[test]
    fn an_empty_table_round_trips_to_no_rows() {
        let mut out = [0u8; SNAPSHOT_OVERHEAD_LEN + ROW_COUNT_LEN];
        let len = write_tunnels_snapshot(core::iter::empty(), &mut out).unwrap();
        assert_eq!(read_tunnels_snapshot(&out[..len]).unwrap().count(), 0);
    }

    #[test]
    fn a_truncated_row_refuses_the_whole_snapshot_at_open() {
        let mut payload = std::vec![0u8; ROW_COUNT_LEN + TUNNEL_ROW_WIRE_LEN - 1];
        payload[..ROW_COUNT_LEN].copy_from_slice(&1u32.to_le_bytes());
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len =
            super::super::envelope::seal_snapshot(SnapshotRegion::Tunnels, &payload, &mut sealed)
                .unwrap();
        assert_eq!(
            read_tunnels_snapshot(&sealed[..len]).err(),
            Some(SnapshotReadError::MalformedPayload),
        );
    }

    #[test]
    fn payload_bytes_past_the_declared_row_count_are_refused() {
        let mut payload = std::vec![0u8; ROW_COUNT_LEN + TUNNEL_ROW_WIRE_LEN];
        payload[..ROW_COUNT_LEN].copy_from_slice(&0u32.to_le_bytes());
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len =
            super::super::envelope::seal_snapshot(SnapshotRegion::Tunnels, &payload, &mut sealed)
                .unwrap();
        assert_eq!(
            read_tunnels_snapshot(&sealed[..len]).err(),
            Some(SnapshotReadError::MalformedPayload),
        );
    }

    #[test]
    fn another_regions_snapshot_is_refused_by_name() {
        let payload = 0u32.to_le_bytes();
        let mut sealed = std::vec![0u8; SNAPSHOT_OVERHEAD_LEN + payload.len()];
        let len = super::super::envelope::seal_snapshot(
            SnapshotRegion::RoutingTable,
            &payload,
            &mut sealed,
        )
        .unwrap();
        assert_eq!(
            read_tunnels_snapshot(&sealed[..len]).err(),
            Some(SnapshotReadError::Envelope(
                SnapshotOpenError::WrongRegion {
                    found: SnapshotRegion::RoutingTable.tag(),
                }
            )),
        );
    }

    #[test]
    fn a_short_buffer_is_refused() {
        let rows = [row(0xF6)];
        let mut short = std::vec![0u8; tunnels_snapshot_len(rows.len()) - 1];
        assert_eq!(
            write_tunnels_snapshot(rows.iter().copied(), &mut short),
            Err(SnapshotSealError::BufferTooShort),
        );
    }
}
