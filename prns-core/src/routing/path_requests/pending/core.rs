use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::wire::DestinationHash;

/// The ordinary RNS path-request lifetime. Bitrate-aware callers retain this as a floor.
pub const PATH_REQUEST_TIMEOUT_MS: u64 = crate::routing::timing::PATH_REQUEST_TIMEOUT_FLOOR_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingPathRequest {
    pub destination: DestinationHash,
    pub command_id: CommandId,
    pub timeout_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettledPathRequest {
    pub command_id: CommandId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpiredPathRequest {
    pub command_id: CommandId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CulledPathRequest {
    pub command_id: CommandId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackPathRequestError {
    TableFull,
}

pub trait PendingPathRequestTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn command_ids(&self) -> &[CommandId];
    fn timeout_ats(&self) -> &[InstantMillis];

    fn push(&mut self, request: PendingPathRequest) -> Result<usize, TrackPathRequestError>;
    fn swap_remove(&mut self, index: usize);

    fn index_of(&self, destination: &DestinationHash) -> Option<usize> {
        self.destinations()
            .iter()
            .position(|candidate| candidate == destination)
    }

    fn earliest_indexed_timeout(&mut self) -> Option<InstantMillis> {
        self.timeout_ats().iter().min().copied()
    }

    fn first_expired(&mut self, now: InstantMillis) -> Option<usize> {
        self.timeout_ats()
            .iter()
            .position(|timeout_at| *timeout_at <= now)
    }
}

#[derive(Debug, Default)]
pub struct PendingPathRequests<C: PendingPathRequestTable> {
    table: C,
    earliest_timeout: Option<InstantMillis>,
}

impl<C: PendingPathRequestTable> PendingPathRequests<C> {
    /// A full table evicts its soonest-expiring row, always favoring the newer request; the dropped one still settles, typed, through the returned cull. At capacity zero the new request is itself the cull.
    pub fn track(&mut self, request: PendingPathRequest) -> Option<CulledPathRequest> {
        let mut culled = None;
        if self.table.len() >= self.table.capacity() {
            culled = self.cull_soonest_expiring();
        }
        let pushed = self.table.push(request);
        self.refresh_earliest_timeout();
        match pushed {
            Ok(_) => culled,
            Err(TrackPathRequestError::TableFull) => Some(CulledPathRequest {
                command_id: request.command_id,
            }),
        }
    }

    fn cull_soonest_expiring(&mut self) -> Option<CulledPathRequest> {
        let index = self
            .table
            .timeout_ats()
            .iter()
            .enumerate()
            .min_by_key(|(_, timeout_at)| **timeout_at)
            .map(|(index, _)| index)?;
        let culled = CulledPathRequest {
            command_id: *self.table.command_ids().get(index)?,
        };
        self.table.swap_remove(index);
        Some(culled)
    }

    fn refresh_earliest_timeout(&mut self) {
        self.earliest_timeout = self.table.earliest_indexed_timeout();
    }

    pub fn earliest_timeout_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_timeout,
            self.table.timeout_ats().iter().min().copied(),
            "earliest_timeout cache desynced from the timeout_ats column"
        );
        self.earliest_timeout
    }

    /// Whether RNS 1.4.2 `Transport.inbound` would exempt `destination` from announce ingress limiting because a request for it is active.
    pub fn contains(&self, destination: &DestinationHash) -> bool {
        self.table.index_of(destination).is_some()
    }

    pub fn pop_settled_for(&mut self, destination: &DestinationHash) -> Option<SettledPathRequest> {
        let index = self.table.index_of(destination)?;
        let settled = SettledPathRequest {
            command_id: *self.table.command_ids().get(index)?,
        };
        self.table.swap_remove(index);
        self.refresh_earliest_timeout();
        Some(settled)
    }

    /// Pop one request whose timeout has passed. Call repeatedly until `None` to fully drain.
    pub fn pop_expired(&mut self, now: InstantMillis) -> Option<ExpiredPathRequest> {
        let index = self.table.first_expired(now)?;
        let expired = ExpiredPathRequest {
            command_id: *self.table.command_ids().get(index)?,
        };
        self.table.swap_remove(index);
        self.refresh_earliest_timeout();
        Some(expired)
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn pending(destination: u8, command_id: u64, timeout_at: u64) -> PendingPathRequest {
        PendingPathRequest {
            destination: dest(destination),
            command_id: CommandId(command_id),
            timeout_at: InstantMillis(timeout_at),
        }
    }

    #[test]
    fn a_learned_route_settles_its_pending_request_exactly_once() {
        let mut table: PendingPathRequests<FixedPendingPathRequestTable<4>> =
            PendingPathRequests::default();
        assert_eq!(table.track(pending(1, 7, 15_000)), None);

        assert_eq!(
            table.pop_settled_for(&dest(1)),
            Some(SettledPathRequest {
                command_id: CommandId(7),
            }),
        );
        assert_eq!(table.pop_settled_for(&dest(1)), None);
        assert!(table.is_empty());
    }

    #[test]
    fn every_request_waiting_on_a_destination_settles_when_it_arrives() {
        let mut table: PendingPathRequests<FixedPendingPathRequestTable<4>> =
            PendingPathRequests::default();
        assert_eq!(table.track(pending(1, 7, 15_000)), None);
        assert_eq!(table.track(pending(1, 8, 16_000)), None);

        let mut settled = std::vec::Vec::new();
        while let Some(s) = table.pop_settled_for(&dest(1)) {
            settled.push(s.command_id);
        }
        settled.sort_unstable_by_key(|id| id.0);
        assert_eq!(settled, std::vec![CommandId(7), CommandId(8)]);
        assert!(table.is_empty());
    }

    #[test]
    fn an_expired_request_pops_for_its_timeout() {
        let mut table: PendingPathRequests<FixedPendingPathRequestTable<4>> =
            PendingPathRequests::default();
        assert_eq!(table.track(pending(1, 7, 15_000)), None);

        assert_eq!(table.pop_expired(InstantMillis(14_999)), None);
        assert_eq!(
            table.pop_expired(InstantMillis(15_000)),
            Some(ExpiredPathRequest {
                command_id: CommandId(7),
            }),
        );
        assert_eq!(table.pop_expired(InstantMillis(15_000)), None);
    }

    #[test]
    fn the_earliest_timeout_drives_the_wakeup() {
        let mut table: PendingPathRequests<FixedPendingPathRequestTable<4>> =
            PendingPathRequests::default();
        assert_eq!(table.earliest_timeout_at(), None);
        assert_eq!(table.track(pending(1, 7, 18_000)), None);
        assert_eq!(table.track(pending(2, 8, 15_000)), None);
        assert_eq!(table.earliest_timeout_at(), Some(InstantMillis(15_000)));
    }

    #[test]
    fn a_full_table_culls_its_soonest_expiring_request_for_the_new_one() {
        let mut table: PendingPathRequests<FixedPendingPathRequestTable<2>> =
            PendingPathRequests::default();
        assert_eq!(table.track(pending(1, 1, 30_000)), None);
        assert_eq!(table.track(pending(2, 2, 20_000)), None);

        assert_eq!(
            table.track(pending(3, 3, 40_000)),
            Some(CulledPathRequest {
                command_id: CommandId(2),
            }),
            "the soonest-expiring request is culled, not the newest",
        );
        assert_eq!(table.len(), 2);
        assert_eq!(table.pop_settled_for(&dest(2)), None);
        assert!(table.pop_settled_for(&dest(1)).is_some());
        assert!(table.pop_settled_for(&dest(3)).is_some());
    }

    #[test]
    fn at_capacity_zero_the_new_request_is_the_cull() {
        let mut table: PendingPathRequests<FixedPendingPathRequestTable<0>> =
            PendingPathRequests::default();
        assert_eq!(
            table.track(pending(1, 9, 15_000)),
            Some(CulledPathRequest {
                command_id: CommandId(9),
            }),
        );
        assert!(table.is_empty());
    }

    #[test]
    fn heap_columns_grow_past_any_fixed_ceiling() {
        let mut table: PendingPathRequests<HeapPendingPathRequestTable> =
            PendingPathRequests::default();
        for n in 0..64u8 {
            assert_eq!(table.track(pending(n, u64::from(n), 100_000)), None);
        }
        assert_eq!(table.len(), 64);
        assert!(table.pop_settled_for(&dest(17)).is_some());
        assert_eq!(table.len(), 63);
    }

    #[cfg(feature = "std")]
    #[test]
    fn heap_indexes_duplicate_destinations_and_exact_timeouts_after_row_moves() {
        let mut table: PendingPathRequests<HeapPendingPathRequestTable> =
            PendingPathRequests::default();
        table.track(pending(1, 1, 30_000));
        table.track(pending(2, 2, 10_000));
        table.track(pending(1, 3, 20_000));

        assert_eq!(table.earliest_timeout_at(), Some(InstantMillis(10_000)));
        assert!(table.pop_settled_for(&dest(1)).is_some());
        assert!(table.pop_settled_for(&dest(1)).is_some());
        assert_eq!(table.pop_expired(InstantMillis(9_999)), None);
        assert_eq!(
            table.pop_expired(InstantMillis(10_000)),
            Some(ExpiredPathRequest {
                command_id: CommandId(2),
            })
        );
        assert!(table.is_empty());
    }
}
