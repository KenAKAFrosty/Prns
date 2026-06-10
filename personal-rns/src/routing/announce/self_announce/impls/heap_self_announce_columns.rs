use alloc::vec::Vec;

use crate::engine::InstantMillis;
use crate::routing::announce::self_announce::{
    ReannounceSchedule, ScheduleAnnounceError, SelfAnnounceAppData, SelfAnnounceColumns,
};
use crate::wire::DestinationHash;

#[derive(Debug, Default)]
pub struct HeapSelfAnnounceColumns {
    destinations: Vec<DestinationHash>,
    schedules: Vec<ReannounceSchedule>,
    last_announced: Vec<Option<InstantMillis>>,
    app_data: Vec<SelfAnnounceAppData>,
}

impl SelfAnnounceColumns for HeapSelfAnnounceColumns {
    fn capacity(&self) -> usize {
        usize::MAX
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
        let i = self.destinations.len();
        self.destinations.push(destination);
        self.schedules.push(schedule);
        self.last_announced.push(None);
        self.app_data.push(app_data);
        Ok(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_past_any_fixed_ceiling() {
        let mut columns = HeapSelfAnnounceColumns::default();
        assert_eq!(columns.capacity(), usize::MAX);

        for n in 0..100u8 {
            let pushed = columns.push(
                DestinationHash::new([n; 16]),
                ReannounceSchedule::every(u64::from(n) + 1),
                SelfAnnounceAppData::from_slice(&[n]).unwrap(),
            );
            assert_eq!(pushed, Ok(n as usize));
        }
        assert_eq!(columns.len(), 100);
        assert_eq!(columns.destinations().len(), 100);
        assert_eq!(columns.app_data_at(99), Some([99u8].as_slice()));
        assert_eq!(columns.app_data_at(100), None);
    }
}
