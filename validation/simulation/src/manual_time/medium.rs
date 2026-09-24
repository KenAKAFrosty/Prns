use super::ManualTimeError;
use crate::ble::{BleAdvanceReport, VirtualBleLab};
use crate::{AdvanceReport, MediumSchedule, SimulationTick, VirtualMedium};

pub enum ManualMedium {
    Frames(VirtualMedium),
    HaLow(crate::halow::VirtualHaLowMedium),
    Ble(VirtualBleLab),
    /// Both media must start at the same tick. Neither may be advanced outside
    /// the driver; same-tick effects settle before any actor is polled again.
    FramesAndBle {
        frames: VirtualMedium,
        ble: VirtualBleLab,
    },
}

impl ManualMedium {
    pub(super) fn check_advance(&self, target: SimulationTick) -> Result<(), ManualTimeError> {
        let now = self.now()?;
        if target < now {
            return Err(ManualTimeError::BeforeCurrent {
                current: now,
                requested: target,
            });
        }
        match self {
            Self::Frames(_) | Self::HaLow(_) => Ok(()),
            Self::Ble(ble) | Self::FramesAndBle { ble, .. } => {
                ble.check_advance(target).map_err(ManualTimeError::Ble)
            }
        }
    }

    pub(super) fn now(&self) -> Result<SimulationTick, ManualTimeError> {
        match self {
            Self::Frames(medium) => Ok(medium.now()),
            Self::HaLow(medium) => Ok(medium.now()),
            Self::Ble(lab) => Ok(lab.now()),
            Self::FramesAndBle { frames, ble } => {
                let frames = frames.now();
                let ble = ble.now();
                if frames != ble {
                    return Err(ManualTimeError::MediaDisagree { frames, ble });
                }
                Ok(frames)
            }
        }
    }

    pub(super) fn schedule(&self) -> Result<MediumSchedule, ManualTimeError> {
        match self {
            Self::Frames(medium) => Ok(medium.schedule()),
            Self::HaLow(medium) => Ok(medium.schedule()),
            Self::Ble(lab) => Ok(lab.schedule()),
            Self::FramesAndBle { frames, ble } => {
                let frames = frames.schedule();
                let ble = ble.schedule();
                if frames.now != ble.now {
                    return Err(ManualTimeError::MediaDisagree {
                        frames: frames.now,
                        ble: ble.now,
                    });
                }
                Ok(MediumSchedule {
                    now: frames.now,
                    next_event_at: frames
                        .next_event_at
                        .into_iter()
                        .chain(ble.next_event_at)
                        .min(),
                })
            }
        }
    }

    pub(super) fn advance(
        &self,
        not_after: SimulationTick,
    ) -> Result<ManualAdvance, ManualTimeError> {
        match self {
            Self::HaLow(medium) => medium
                .advance_to_next_event(not_after)
                .map(ManualAdvance::HaLow)
                .map_err(ManualTimeError::HaLow),
            Self::Frames(medium) => medium
                .advance_to_next_event(not_after)
                .map(ManualAdvance::Frames)
                .map_err(ManualTimeError::Frames),
            Self::Ble(lab) => lab
                .advance_to_next_event(not_after)
                .map(ManualAdvance::Ble)
                .map_err(ManualTimeError::Ble),
            Self::FramesAndBle { frames, ble } => {
                let schedule = self.schedule()?;
                if not_after < schedule.now {
                    return Err(ManualTimeError::BeforeCurrent {
                        current: schedule.now,
                        requested: not_after,
                    });
                }
                let target = schedule.target_not_after(not_after);
                // BLE validates its emission budget before mutation. After that
                // succeeds, frame advance can only refuse backwards time, which
                // the common schedule already excluded. Concurrent medium
                // mutation is forbidden by the driver's ownership contract.
                let ble = ble.advance_to(target).map_err(ManualTimeError::Ble)?;
                let frames = frames.advance_to(target).map_err(ManualTimeError::Frames)?;
                Ok(ManualAdvance::FramesAndBle { frames, ble })
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManualAdvance {
    Frames(AdvanceReport),
    HaLow(AdvanceReport),
    Ble(BleAdvanceReport),
    FramesAndBle {
        frames: AdvanceReport,
        ble: BleAdvanceReport,
    },
}

impl ManualAdvance {
    pub(super) fn bounds(&self) -> (SimulationTick, SimulationTick) {
        match self {
            Self::Frames(report) | Self::HaLow(report) => (report.from, report.to),
            Self::Ble(report) => (report.from, report.to),
            Self::FramesAndBle { frames, .. } => (frames.from, frames.to),
        }
    }
}
