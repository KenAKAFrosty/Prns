#[derive(Clone, Copy)]
struct SoftApLease {
    mac: [u8; 6],
    last_seen_ms: u64,
}

/// A tiny deterministic DHCP lease index for the embedded SoftAP.
///
/// Known stations keep their slot, new stations take a vacant slot, and a station arriving after
/// the table is full replaces the least recently seen entry. The caller maps the returned index to
/// its subnet's host address.
pub struct SoftApLeaseTable<const N: usize> {
    leases: [Option<SoftApLease>; N],
}

impl<const N: usize> SoftApLeaseTable<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self { leases: [None; N] }
    }

    pub fn assign(&mut self, mac: [u8; 6], now_ms: u64) -> Option<usize> {
        let index = self
            .leases
            .iter()
            .position(|lease| lease.is_some_and(|lease| lease.mac == mac))
            .or_else(|| self.leases.iter().position(Option::is_none))
            // A new association can replace a departed station whose DHCP release was lost. If
            // every station is still active, the AP association ceiling prevents this path.
            .or_else(|| {
                self.leases
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, lease)| lease.map_or(0, |lease| lease.last_seen_ms))
                    .map(|(index, _)| index)
            })?;
        self.leases[index] = Some(SoftApLease {
            mac,
            last_seen_ms: now_ms,
        });
        Some(index)
    }
}

impl<const N: usize> Default for SoftApLeaseTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(last: u8) -> [u8; 6] {
        [2, 0, 0, 0, 0, last]
    }

    #[test]
    fn distinct_stations_receive_distinct_stable_slots() {
        let mut leases = SoftApLeaseTable::<4>::new();

        assert_eq!(leases.assign(mac(1), 10), Some(0));
        assert_eq!(leases.assign(mac(2), 20), Some(1));
        assert_eq!(leases.assign(mac(3), 30), Some(2));
        assert_eq!(leases.assign(mac(4), 40), Some(3));
        assert_eq!(leases.assign(mac(2), 50), Some(1));
    }

    #[test]
    fn a_replacement_reuses_the_least_recently_seen_slot() {
        let mut leases = SoftApLeaseTable::<2>::new();

        assert_eq!(leases.assign(mac(1), 10), Some(0));
        assert_eq!(leases.assign(mac(2), 20), Some(1));
        assert_eq!(leases.assign(mac(1), 30), Some(0));
        assert_eq!(leases.assign(mac(3), 40), Some(1));
    }

    #[test]
    fn an_empty_pool_rejects_assignments() {
        assert_eq!(SoftApLeaseTable::<0>::new().assign(mac(1), 1), None);
    }
}
