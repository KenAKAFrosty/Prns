use crate::SimulationTick;

/// One atomic view of a medium's time and earliest scheduled event.
/// Already queued receptions and runnable runtime tasks are not scheduled medium events.
#[derive(Debug, PartialEq, Eq)]
pub struct MediumSchedule {
    pub now: SimulationTick,
    pub next_event_at: Option<SimulationTick>,
}

impl MediumSchedule {
    pub(crate) fn target_not_after(&self, limit: SimulationTick) -> SimulationTick {
        self.next_event_at.map_or(limit, |at| at.min(limit))
    }
}

#[cfg(test)]
mod ble_tests;
#[cfg(test)]
mod frame_tests;
