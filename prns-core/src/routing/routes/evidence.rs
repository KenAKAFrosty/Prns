use super::RouteEvidenceId;

/// Snapshot of the occupied evidence-id space at and ahead of one issuance candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RouteEvidenceScan {
    current_in_use: bool,
    next_in_use: Option<RouteEvidenceId>,
}

impl RouteEvidenceScan {
    pub(crate) fn over(
        candidate: RouteEvidenceId,
        ids: impl IntoIterator<Item = RouteEvidenceId>,
    ) -> Self {
        let mut current_in_use = false;
        let mut next_in_use = None;
        for id in ids {
            match id.cmp(&candidate) {
                core::cmp::Ordering::Less => {}
                core::cmp::Ordering::Equal => current_in_use = true,
                core::cmp::Ordering::Greater => {
                    next_in_use =
                        Some(next_in_use.map_or(id, |next: RouteEvidenceId| next.min(id)));
                }
            }
        }
        Self {
            current_in_use,
            next_in_use,
        }
    }
}

/// Monotonic ID issuer with a guarded reuse frontier after `u32` wrap.
///
/// Before wrap, minting is an increment. Afterwards, the optional barrier names the next id that might still be live. Reaching that barrier will invoke the caller's bounded holder scan (to re-balance the barrier and issuance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RouteEvidenceIdIssuer {
    next: RouteEvidenceId,
    reuse_barrier: Option<RouteEvidenceId>,
}

const _: () = assert!(core::mem::size_of::<RouteEvidenceIdIssuer>() == 8);

impl Default for RouteEvidenceIdIssuer {
    fn default() -> Self {
        Self {
            next: RouteEvidenceId::FIRST,
            reuse_barrier: None,
        }
    }
}

impl RouteEvidenceIdIssuer {
    pub(crate) fn issue(
        &mut self,
        mut scan: impl FnMut(RouteEvidenceId) -> RouteEvidenceScan,
    ) -> RouteEvidenceId {
        loop {
            if self.reuse_barrier == Some(self.next) {
                let occupied = scan(self.next);
                if occupied.current_in_use {
                    let wrapped = self.advance();
                    self.reuse_barrier = if wrapped {
                        Some(self.next)
                    } else {
                        occupied.next_in_use
                    };
                    continue;
                }
                self.reuse_barrier = occupied.next_in_use;
            }

            let issued = self.next;
            if self.advance() {
                // Force a fresh view at the beginning of every lap. Newly issued ids are behind
                // the cursor until then, so no other issuance needs a holder scan.
                self.reuse_barrier = Some(self.next);
            }
            return issued;
        }
    }

    fn advance(&mut self) -> bool {
        match RouteEvidenceId::new(self.next.get().wrapping_add(1)) {
            Some(next) => {
                self.next = next;
                false
            }
            None => {
                self.next = RouteEvidenceId::FIRST;
                true
            }
        }
    }

    #[cfg(test)]
    pub(crate) const fn with_next(next: RouteEvidenceId) -> Self {
        Self {
            next,
            reuse_barrier: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u32) -> RouteEvidenceId {
        RouteEvidenceId::new(value).unwrap()
    }

    #[test]
    fn normal_issuance_never_scans() {
        let mut issuer = RouteEvidenceIdIssuer::default();
        let mut scans = 0;
        assert_eq!(
            issuer.issue(|_| {
                scans += 1;
                unreachable!()
            }),
            id(1)
        );
        assert_eq!(issuer.issue(|_| unreachable!()), id(2));
        assert_eq!(scans, 0);
    }

    #[test]
    fn wrap_skips_zero_and_only_scans_at_live_barriers() {
        use core::cell::Cell;

        let mut issuer = RouteEvidenceIdIssuer::with_next(id(u32::MAX));
        let mut live = [id(1), id(3), id(u32::MAX)];
        let scans = Cell::new(0);
        let issue = |issuer: &mut RouteEvidenceIdIssuer, live: &[RouteEvidenceId]| {
            issuer.issue(|candidate| {
                scans.set(scans.get() + 1);
                RouteEvidenceScan::over(candidate, live.iter().copied())
            })
        };

        assert_eq!(issue(&mut issuer, &live), id(u32::MAX));
        assert_eq!(issue(&mut issuer, &live), id(2));
        assert_eq!(scans.get(), 1, "one wrap scan skips the occupied id 1");

        live[1] = id(30);
        assert_eq!(issue(&mut issuer, &live), id(3));
        assert_eq!(scans.get(), 2, "the dead barrier is rescanned exactly once");
        assert_eq!(issue(&mut issuer, &live), id(4));
        assert_eq!(
            scans.get(),
            2,
            "the open interval returns to increment-only issuance"
        );
    }

    #[test]
    fn contiguous_live_ids_are_skipped_without_reuse() {
        let mut issuer = RouteEvidenceIdIssuer::with_next(id(u32::MAX));
        let live = [id(1), id(2), id(3)];
        let mut scan = |candidate| RouteEvidenceScan::over(candidate, live.iter().copied());

        assert_eq!(issuer.issue(&mut scan), id(u32::MAX));
        assert_eq!(issuer.issue(&mut scan), id(4));
    }
}
