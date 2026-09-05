use super::super::MonotonicMicros;
use super::HoppingRegion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HopTransmission {
    channel_index: usize,
    started_at: MonotonicMicros,
    airtime_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopTransmissionError {
    EmptyAirtime,
    TimeRangeOverflow,
}

impl HopTransmission {
    pub const fn new(
        channel_index: usize,
        started_at: MonotonicMicros,
        airtime_us: u64,
    ) -> Result<Self, HopTransmissionError> {
        if airtime_us == 0 {
            return Err(HopTransmissionError::EmptyAirtime);
        }
        if started_at.micros().checked_add(airtime_us).is_none() {
            return Err(HopTransmissionError::TimeRangeOverflow);
        }
        Ok(Self {
            channel_index,
            started_at,
            airtime_us,
        })
    }

    pub const fn channel_index(self) -> usize {
        self.channel_index
    }

    pub const fn started_at(self) -> MonotonicMicros {
        self.started_at
    }

    pub const fn airtime_us(self) -> u64 {
        self.airtime_us
    }

    pub const fn ends_at(self) -> MonotonicMicros {
        MonotonicMicros::new(self.started_at.micros() + self.airtime_us)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrequencyOccupancySummary {
    transmission_count: usize,
    total_airtime_us: u64,
    maximum_channel_occupancy_us: u64,
}

impl FrequencyOccupancySummary {
    pub const fn transmission_count(self) -> usize {
        self.transmission_count
    }

    pub const fn total_airtime_us(self) -> u64 {
        self.total_airtime_us
    }

    pub const fn maximum_channel_occupancy_us(self) -> u64 {
        self.maximum_channel_occupancy_us
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrequencyOccupancyError {
    ChannelIndexOutsideHopSet {
        transmission_index: usize,
        channel_index: usize,
        channel_count: usize,
    },
    TransmissionsOverlap {
        earlier_index: usize,
        later_index: usize,
    },
    ChannelOccupancyExceeded {
        channel_index: usize,
        window_end_us: u64,
        occupied_us: u64,
        maximum_us: u64,
    },
}

pub fn audit_frequency_occupancy<R, const N: usize>(
    region: &R,
    transmissions: &[HopTransmission],
) -> Result<FrequencyOccupancySummary, FrequencyOccupancyError>
where
    R: HoppingRegion<N>,
{
    validate_trace::<N>(transmissions)?;
    let observation_window_us = region.observation_window().micros();
    let maximum_us = region.channel_occupancy_limit().micros();
    let mut total_airtime_us = 0u64;
    let mut maximum_channel_occupancy_us = 0u64;
    for transmission in transmissions {
        total_airtime_us = total_airtime_us.saturating_add(transmission.airtime_us());
        for window_end_us in [
            transmission.ends_at().micros(),
            transmission
                .started_at()
                .micros()
                .saturating_add(observation_window_us),
        ] {
            let occupied_us = channel_occupancy_in_window(
                transmissions,
                transmission.channel_index(),
                window_end_us,
                observation_window_us,
            );
            maximum_channel_occupancy_us = maximum_channel_occupancy_us.max(occupied_us);
            if occupied_us > maximum_us {
                return Err(FrequencyOccupancyError::ChannelOccupancyExceeded {
                    channel_index: transmission.channel_index(),
                    window_end_us,
                    occupied_us,
                    maximum_us,
                });
            }
        }
    }
    Ok(FrequencyOccupancySummary {
        transmission_count: transmissions.len(),
        total_airtime_us,
        maximum_channel_occupancy_us,
    })
}

fn validate_trace<const N: usize>(
    transmissions: &[HopTransmission],
) -> Result<(), FrequencyOccupancyError> {
    for (index, transmission) in transmissions.iter().enumerate() {
        if transmission.channel_index() >= N {
            return Err(FrequencyOccupancyError::ChannelIndexOutsideHopSet {
                transmission_index: index,
                channel_index: transmission.channel_index(),
                channel_count: N,
            });
        }
        if index > 0 && transmission.started_at() < transmissions[index - 1].ends_at() {
            return Err(FrequencyOccupancyError::TransmissionsOverlap {
                earlier_index: index - 1,
                later_index: index,
            });
        }
    }
    Ok(())
}

fn channel_occupancy_in_window(
    transmissions: &[HopTransmission],
    channel_index: usize,
    window_end_us: u64,
    window_duration_us: u64,
) -> u64 {
    transmissions
        .iter()
        .filter(|transmission| transmission.channel_index() == channel_index)
        .map(|transmission| transmission_overlap(*transmission, window_end_us, window_duration_us))
        .fold(0u64, u64::saturating_add)
}

fn transmission_overlap(
    transmission: HopTransmission,
    window_end_us: u64,
    window_duration_us: u64,
) -> u64 {
    let window_start_us = window_end_us.saturating_sub(window_duration_us);
    let overlap_start_us = transmission.started_at().micros().max(window_start_us);
    let overlap_end_us = transmission.ends_at().micros().min(window_end_us);
    overlap_end_us.saturating_sub(overlap_start_us)
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn overlap_never_exceeds_transmission_airtime() {
        let started_at: u32 = kani::any();
        let airtime: u16 = kani::any();
        let window_end: u32 = kani::any();
        let window_duration: u32 = kani::any();
        kani::assume(airtime > 0);
        let transmission =
            HopTransmission::new(0, MonotonicMicros::new(started_at as u64), airtime as u64)
                .unwrap();
        assert!(
            transmission_overlap(transmission, window_end as u64, window_duration as u64,)
                <= airtime as u64
        );
    }
}
