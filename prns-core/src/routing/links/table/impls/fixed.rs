use crate::engine::InstantMillis;
use crate::routing::links::table::{LinkPhase, LinkTable, TrackLinkError};
use crate::routing::links::LinkId;

#[derive(Debug)]
pub struct FixedLinkTable<const MAX_LINKS: usize> {
    len: usize,
    link_ids: [LinkId; MAX_LINKS],
    timeout_ats: [InstantMillis; MAX_LINKS],
    has_timeout: [bool; MAX_LINKS],
    phases: [LinkPhase; MAX_LINKS],
}

impl<const MAX_LINKS: usize> Default for FixedLinkTable<MAX_LINKS> {
    fn default() -> Self {
        Self {
            len: 0,
            link_ids: [LinkId::new([0u8; 16]); MAX_LINKS],
            timeout_ats: [InstantMillis(0); MAX_LINKS],
            has_timeout: [false; MAX_LINKS],
            phases: core::array::from_fn(|_| LinkPhase::vacant()),
        }
    }
}

impl<const MAX_LINKS: usize> LinkTable for FixedLinkTable<MAX_LINKS> {
    fn capacity(&self) -> usize {
        MAX_LINKS
    }
    fn len(&self) -> usize {
        self.len
    }

    fn link_ids(&self) -> &[LinkId] {
        &self.link_ids[..self.len]
    }
    fn timeout_at(&self, index: usize) -> Option<InstantMillis> {
        self.has_timeout[..self.len][index].then_some(self.timeout_ats[index])
    }

    fn phases(&self) -> &[LinkPhase] {
        &self.phases[..self.len]
    }

    fn phase_mut(&mut self, index: usize) -> &mut LinkPhase {
        &mut self.phases[index]
    }

    fn set_timeout_at(&mut self, index: usize, timeout_at: Option<InstantMillis>) {
        self.has_timeout[index] = timeout_at.is_some();
        if let Some(at) = timeout_at {
            self.timeout_ats[index] = at;
        }
    }

    fn push(
        &mut self,
        link_id: LinkId,
        phase: LinkPhase,
        timeout_at: Option<InstantMillis>,
    ) -> Result<usize, TrackLinkError> {
        if self.len >= MAX_LINKS {
            return Err(TrackLinkError::TableFull);
        }
        let i = self.len;
        self.link_ids[i] = link_id;
        self.set_timeout_at(i, timeout_at);
        self.phases[i] = phase;
        self.len += 1;
        Ok(i)
    }

    fn swap_remove(&mut self, index: usize) {
        let last = self.len - 1;
        self.link_ids.swap(index, last);
        self.timeout_ats.swap(index, last);
        self.has_timeout.swap(index, last);
        self.phases.swap(index, last);
        self.phases[last] = LinkPhase::vacant();
        self.len = last;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot<const N: usize>(
        table: &FixedLinkTable<N>,
    ) -> std::vec::Vec<(LinkId, Option<InstantMillis>)> {
        (0..table.len())
            .map(|i| (table.link_ids()[i], table.timeout_at(i)))
            .collect()
    }

    #[test]
    fn optional_deadlines_preserve_zero_maximum_and_vacant_slot_reuse() {
        let mut table = FixedLinkTable::<3>::default();
        let a = LinkId::new([1; 16]);
        let b = LinkId::new([2; 16]);
        let c = LinkId::new([3; 16]);
        table
            .push(a, LinkPhase::vacant(), Some(InstantMillis(0)))
            .unwrap();
        table
            .push(b, LinkPhase::vacant(), Some(InstantMillis(u64::MAX)))
            .unwrap();
        table.push(c, LinkPhase::vacant(), None).unwrap();
        assert_eq!(
            snapshot(&table),
            [
                (a, Some(InstantMillis(0))),
                (b, Some(InstantMillis(u64::MAX))),
                (c, None)
            ]
        );
        assert_eq!(
            table.first_due_timeout_matching(InstantMillis(0), |_, _| true),
            Some(0)
        );
        table.set_timeout_at(0, None);
        assert_eq!(
            table.earliest_indexed_timeout(),
            Some(InstantMillis(u64::MAX))
        );
        assert_eq!(
            table.first_due_timeout_matching(InstantMillis(u64::MAX - 1), |_, _| true),
            None
        );
        assert_eq!(
            table.first_due_timeout_matching(InstantMillis(u64::MAX), |_, _| true),
            Some(1)
        );
        table.swap_remove(0);
        assert_eq!(
            snapshot(&table),
            [(c, None), (b, Some(InstantMillis(u64::MAX)))]
        );
        table.swap_remove(1);
        table.push(a, LinkPhase::vacant(), None).unwrap();
        assert_eq!(snapshot(&table), [(c, None), (a, None)]);
        assert_eq!(table.earliest_indexed_timeout(), None);
        table.set_timeout_at(1, Some(InstantMillis(0)));
        assert_eq!(snapshot(&table), [(c, None), (a, Some(InstantMillis(0)))]);
    }

    #[test]
    fn fixed_deadlines_do_not_pay_per_row_option_alignment() {
        type PreviousColumns = (
            usize,
            [LinkId; 32],
            [Option<InstantMillis>; 32],
            [LinkPhase; 32],
        );
        let per_row_saving = core::mem::size_of::<Option<InstantMillis>>()
            - core::mem::size_of::<InstantMillis>()
            - core::mem::size_of::<bool>();
        let alignment_slack = core::mem::align_of::<PreviousColumns>() - 1;
        let minimum_saving = (32 * per_row_saving).saturating_sub(alignment_slack);
        assert!(minimum_saving > 0);
        assert!(
            core::mem::size_of::<FixedLinkTable<32>>() + minimum_saving
                <= core::mem::size_of::<PreviousColumns>()
        );
    }

    proptest::proptest! {
        #[test]
        fn deadline_columns_match_whole_rows_across_updates_and_removal(
            operations in proptest::collection::vec((0u8..4, proptest::num::u64::ANY), 0..128)
        ) {
            let mut table = FixedLinkTable::<4>::default();
            let mut model = std::vec::Vec::new();
            for (step, (operation, value)) in operations.into_iter().enumerate() {
                let deadline = Some(InstantMillis(value));
                match operation {
                    0 => {
                        let link = LinkId::new([step as u8; 16]);
                        let expected = if model.len() < 4 {
                            model.push((link, deadline));
                            Ok(model.len() - 1)
                        } else {
                            Err(TrackLinkError::TableFull)
                        };
                        proptest::prop_assert_eq!(table.push(link, LinkPhase::vacant(), deadline), expected);
                    }
                    1 | 2 if !model.is_empty() => {
                        let index = (value % model.len() as u64) as usize;
                        let replacement = if operation == 1 { deadline } else { None };
                        table.set_timeout_at(index, replacement);
                        model[index].1 = replacement;
                    }
                    3 if !model.is_empty() => {
                        let index = (value % model.len() as u64) as usize;
                        table.swap_remove(index);
                        model.swap_remove(index);
                    }
                    _ => {}
                }
                proptest::prop_assert_eq!(snapshot(&table), model.clone());
                proptest::prop_assert_eq!(table.earliest_indexed_timeout(), model.iter().filter_map(|row| row.1).min());
                let expected = model.iter().enumerate().position(|(index, row)| index % 2 == 0 && row.1.is_some_and(|at| at.0 <= value));
                proptest::prop_assert_eq!(table.first_due_timeout_matching(InstantMillis(value), |index, _| index % 2 == 0), expected);
            }
        }
    }
}
