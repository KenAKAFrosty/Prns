use crate::engine::InstantMillis;
use crate::routing::announce::self_announce::{
    ReannounceSchedule, ScheduleAnnounceError, SelfAnnounceAppData, SelfAnnounceColumns,
};
use crate::wire::DestinationHash;
use heapless::Vec as HeaplessVec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedSelfAnnounceColumns<const MAX_SELF_ANNOUNCED_DESTINATIONS: usize> {
    destinations: HeaplessVec<DestinationHash, MAX_SELF_ANNOUNCED_DESTINATIONS>,
    schedules: HeaplessVec<ReannounceSchedule, MAX_SELF_ANNOUNCED_DESTINATIONS>,
    last_announced: HeaplessVec<Option<InstantMillis>, MAX_SELF_ANNOUNCED_DESTINATIONS>,
    app_data: HeaplessVec<SelfAnnounceAppData, MAX_SELF_ANNOUNCED_DESTINATIONS>,
}

impl<const MAX_SELF_ANNOUNCED_DESTINATIONS: usize> Default
    for FixedSelfAnnounceColumns<MAX_SELF_ANNOUNCED_DESTINATIONS>
{
    fn default() -> Self {
        Self {
            destinations: HeaplessVec::new(),
            schedules: HeaplessVec::new(),
            last_announced: HeaplessVec::new(),
            app_data: HeaplessVec::new(),
        }
    }
}

impl<const MAX_SELF_ANNOUNCED_DESTINATIONS: usize> SelfAnnounceColumns
    for FixedSelfAnnounceColumns<MAX_SELF_ANNOUNCED_DESTINATIONS>
{
    fn capacity(&self) -> usize {
        MAX_SELF_ANNOUNCED_DESTINATIONS
    }
    fn len(&self) -> usize {
        self.destinations.len()
    }

    fn destinations(&self) -> &[DestinationHash] {
        &self.destinations
    }
    fn schedules(&self) -> &[ReannounceSchedule] {
        &self.schedules
    }
    fn last_announced(&self) -> &[Option<InstantMillis>] {
        &self.last_announced
    }
    fn app_data_at(&self, index: usize) -> Option<&[u8]> {
        self.app_data.get(index).map(|data| data.as_slice())
    }

    fn set_row(
        &mut self,
        index: usize,
        schedule: ReannounceSchedule,
        app_data: SelfAnnounceAppData,
    ) {
        if let Some(slot) = self.schedules.get_mut(index) {
            *slot = schedule;
        }
        if let Some(slot) = self.app_data.get_mut(index) {
            *slot = app_data;
        }
        if let Some(slot) = self.last_announced.get_mut(index) {
            *slot = None;
        }
    }

    fn set_last_announced(&mut self, index: usize, at: InstantMillis) {
        if let Some(slot) = self.last_announced.get_mut(index) {
            *slot = Some(at);
        }
    }

    fn push(
        &mut self,
        destination: DestinationHash,
        schedule: ReannounceSchedule,
        app_data: SelfAnnounceAppData,
    ) -> Result<usize, ScheduleAnnounceError> {
        if self.destinations.is_full() {
            return Err(ScheduleAnnounceError::TableFull);
        }
        let i = self.destinations.len();
        let _ = self.destinations.push(destination);
        let _ = self.schedules.push(schedule);
        let _ = self.last_announced.push(None);
        let _ = self.app_data.push(app_data);
        Ok(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn data(bytes: &[u8]) -> SelfAnnounceAppData {
        SelfAnnounceAppData::from_slice(bytes).unwrap()
    }

    #[test]
    fn exposes_only_pushed_rows_and_reports_a_full_table() {
        let mut columns = FixedSelfAnnounceColumns::<2>::default();
        assert_eq!(columns.capacity(), 2);
        assert!(columns.is_empty());

        assert_eq!(
            columns.push(dest(1), ReannounceSchedule::every(1_000), data(b"one")),
            Ok(0)
        );
        assert_eq!(
            columns.push(dest(2), ReannounceSchedule::every(2_000), data(b"two")),
            Ok(1)
        );
        assert_eq!(
            columns.push(dest(3), ReannounceSchedule::every(3_000), data(b"three")),
            Err(ScheduleAnnounceError::TableFull),
        );

        assert_eq!(columns.len(), 2);
        assert_eq!(columns.destinations(), &[dest(1), dest(2)]);
        assert_eq!(
            columns.schedules(),
            &[
                ReannounceSchedule::every(1_000),
                ReannounceSchedule::every(2_000)
            ]
        );
        assert_eq!(columns.last_announced(), &[None, None]);
        assert_eq!(columns.app_data_at(0), Some(b"one".as_slice()));
        assert_eq!(columns.app_data_at(1), Some(b"two".as_slice()));
        assert_eq!(columns.app_data_at(2), None);
    }

    #[test]
    fn set_row_replaces_config_and_clears_the_cadence_anchor() {
        let mut columns = FixedSelfAnnounceColumns::<2>::default();
        columns
            .push(dest(1), ReannounceSchedule::every(1_000), data(b"old"))
            .unwrap();
        columns.set_last_announced(0, InstantMillis(500));
        assert_eq!(columns.last_announced(), &[Some(InstantMillis(500))]);

        columns.set_row(0, ReannounceSchedule::every(9_000), data(b"new"));
        assert_eq!(columns.schedules(), &[ReannounceSchedule::every(9_000)]);
        assert_eq!(columns.app_data_at(0), Some(b"new".as_slice()));
        assert_eq!(columns.last_announced(), &[None]);
    }
}
