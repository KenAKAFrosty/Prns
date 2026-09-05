use super::super::Frequency;
use super::clock::ClockWindow;
use super::frame::EncodedDatagram;
use super::profile::TurboPhyProfile;

pub const TURBO_CHANNEL_COUNT: usize = 51;
pub const TURBO_SLOT_US: u64 = 400_000;
pub const TURBO_CYCLE_US: u64 = TURBO_SLOT_US * TURBO_CHANNEL_COUNT as u64;
pub const TURBO_SUPERCYCLE_SLOTS: u64 = TURBO_CHANNEL_COUNT as u64 * TURBO_CHANNEL_COUNT as u64;
pub const TURBO_SUPERCYCLE_US: u64 = TURBO_SLOT_US * TURBO_SUPERCYCLE_SLOTS;
pub const TURBO_OCCUPANCY_LIMIT_US: u64 = 390_000;
pub const TURBO_BOOT_QUARANTINE_US: u64 = 10_000_000;
pub const TURBO_SCAN_STRIDE: usize = 7;
pub const TURBO_SCAN_DWELL_US: u64 = 341_000;

pub const US915_TURBO_CHANNELS: [Frequency; TURBO_CHANNEL_COUNT] = channels();

pub const TURBO_CHANNEL_ORDER: [u8; TURBO_CHANNEL_COUNT] = [
    23, 35, 4, 16, 32, 45, 7, 19, 43, 31, 18, 0, 48, 28, 2, 15, 30, 42, 9, 21, 49, 34, 3, 22, 37,
    8, 20, 39, 1, 27, 14, 41, 29, 17, 47, 5, 33, 46, 13, 25, 44, 12, 24, 40, 11, 26, 38, 10, 50,
    36, 6,
];

const fn channels() -> [Frequency; TURBO_CHANNEL_COUNT] {
    let mut frequencies = [Frequency::new(902_500_000); TURBO_CHANNEL_COUNT];
    let mut index = 0;
    while index < TURBO_CHANNEL_COUNT {
        frequencies[index] = Frequency::new(902_500_000 + index as u32 * 500_000);
        index += 1;
    }
    frequencies
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupercycleCycle(u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupercycleCycleError {
    OutsideRange { cycle: u8 },
}

impl SupercycleCycle {
    pub const fn new(cycle: u8) -> Result<Self, SupercycleCycleError> {
        if cycle as usize >= TURBO_CHANNEL_COUNT {
            return Err(SupercycleCycleError::OutsideRange { cycle });
        }
        Ok(Self(cycle))
    }

    pub const fn index(self) -> u8 {
        self.0
    }

    pub(crate) const fn from_base_cycle(base_cycle: u64) -> Self {
        Self((base_cycle % TURBO_CHANNEL_COUNT as u64) as u8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransmissionTimingBudget {
    enter_us: u64,
    interframe_us: u64,
    exit_us: u64,
    scheduling_jitter_us: u64,
    final_clear_validity_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransmissionTimingBudgetError {
    EmptyEntryBudget,
    EmptyExitBudget,
    EmptyJitterBudget,
    EmptyFinalClearValidity,
    GuardConsumesSlot,
}

impl TransmissionTimingBudget {
    pub const fn new(
        enter_us: u64,
        interframe_us: u64,
        exit_us: u64,
        scheduling_jitter_us: u64,
        final_clear_validity_us: u64,
    ) -> Result<Self, TransmissionTimingBudgetError> {
        if enter_us == 0 {
            return Err(TransmissionTimingBudgetError::EmptyEntryBudget);
        }
        if exit_us == 0 {
            return Err(TransmissionTimingBudgetError::EmptyExitBudget);
        }
        if scheduling_jitter_us == 0 {
            return Err(TransmissionTimingBudgetError::EmptyJitterBudget);
        }
        if final_clear_validity_us == 0 {
            return Err(TransmissionTimingBudgetError::EmptyFinalClearValidity);
        }
        if enter_us
            .saturating_add(exit_us)
            .saturating_add(scheduling_jitter_us.saturating_mul(2))
            >= TURBO_SLOT_US
        {
            return Err(TransmissionTimingBudgetError::GuardConsumesSlot);
        }
        Ok(Self {
            enter_us,
            interframe_us,
            exit_us,
            scheduling_jitter_us,
            final_clear_validity_us,
        })
    }

    pub const fn interframe_us(self) -> u64 {
        self.interframe_us
    }

    pub const fn final_clear_validity_us(self) -> u64 {
        self.final_clear_validity_us
    }

    const fn entry_guard_us(self) -> u64 {
        self.enter_us.saturating_add(self.scheduling_jitter_us)
    }

    const fn exit_guard_us(self) -> u64 {
        self.exit_us.saturating_add(self.scheduling_jitter_us)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboOpportunity {
    channel_index: usize,
    frequency: Frequency,
    global_slot: u64,
    cycle: SupercycleCycle,
    transmit_must_end_by_schedule_us: u64,
}

impl TurboOpportunity {
    pub const fn channel_index(self) -> usize {
        self.channel_index
    }

    pub const fn frequency(self) -> Frequency {
        self.frequency
    }

    pub const fn global_slot(self) -> u64 {
        self.global_slot
    }

    pub const fn cycle(self) -> SupercycleCycle {
        self.cycle
    }

    pub const fn transmit_must_end_by_schedule_us(self) -> u64 {
        self.transmit_must_end_by_schedule_us
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpportunityRejection {
    ClockRangeOverflow,
    ClockWindowCrossesChannelBoundary,
    EntryGuard,
    PacketCrossesGuardedBoundary,
}

pub const fn global_slot_at(schedule_us: u64) -> u64 {
    schedule_us / TURBO_SLOT_US
}

pub const fn supercycle_cycle_at(schedule_us: u64) -> SupercycleCycle {
    let global_slot = global_slot_at(schedule_us);
    SupercycleCycle((global_slot / TURBO_CHANNEL_COUNT as u64 % TURBO_CHANNEL_COUNT as u64) as u8)
}

pub const fn channel_index_at(schedule_us: u64) -> usize {
    channel_index_for_global_slot(global_slot_at(schedule_us))
}

pub const fn channel_index_for_global_slot(global_slot: u64) -> usize {
    let position = global_slot % TURBO_CHANNEL_COUNT as u64;
    let cycle = global_slot / TURBO_CHANNEL_COUNT as u64 % TURBO_CHANNEL_COUNT as u64;
    TURBO_CHANNEL_ORDER[((position + cycle) % TURBO_CHANNEL_COUNT as u64) as usize] as usize
}

pub const fn slot_position_for_channel(
    cycle: SupercycleCycle,
    channel_index: usize,
) -> Result<usize, ChannelLookupError> {
    if channel_index >= TURBO_CHANNEL_COUNT {
        return Err(ChannelLookupError::OutsideHopSet { channel_index });
    }
    let mut position = 0;
    while position < TURBO_CHANNEL_COUNT {
        let order_position = (position + cycle.index() as usize) % TURBO_CHANNEL_COUNT;
        if TURBO_CHANNEL_ORDER[order_position] as usize == channel_index {
            return Ok(position);
        }
        position += 1;
    }
    Err(ChannelLookupError::OutsideHopSet { channel_index })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLookupError {
    OutsideHopSet { channel_index: usize },
}

pub(crate) fn opportunity_for(
    clock: ClockWindow,
    profile: TurboPhyProfile,
    datagram: &EncodedDatagram,
    timing: TransmissionTimingBudget,
) -> Result<TurboOpportunity, OpportunityRejection> {
    let earliest = clock.earliest_schedule_us();
    let latest = clock.latest_schedule_us();
    let earliest_slot = global_slot_at(earliest);
    let latest_slot = global_slot_at(latest);
    if earliest_slot != latest_slot {
        return Err(OpportunityRejection::ClockWindowCrossesChannelBoundary);
    }
    let slot_begins_at_schedule_us = earliest_slot
        .checked_mul(TURBO_SLOT_US)
        .ok_or(OpportunityRejection::ClockRangeOverflow)?;
    let transmit_not_before_schedule_us = slot_begins_at_schedule_us
        .checked_add(timing.entry_guard_us())
        .ok_or(OpportunityRejection::ClockRangeOverflow)?;
    if earliest < transmit_not_before_schedule_us {
        return Err(OpportunityRejection::EntryGuard);
    }
    let slot_ends_at_schedule_us = slot_begins_at_schedule_us
        .checked_add(TURBO_SLOT_US)
        .ok_or(OpportunityRejection::ClockRangeOverflow)?;
    let transmit_must_end_by_schedule_us = slot_ends_at_schedule_us
        .checked_sub(timing.exit_guard_us())
        .ok_or(OpportunityRejection::ClockRangeOverflow)?;
    let elapsed_us =
        datagram
            .keyed_airtime_us(profile)
            .saturating_add(if datagram.frame_count() == 2 {
                timing.interframe_us()
            } else {
                0
            });
    let projected_latest_end = latest
        .checked_add(elapsed_us)
        .ok_or(OpportunityRejection::ClockRangeOverflow)?;
    if projected_latest_end > transmit_must_end_by_schedule_us {
        return Err(OpportunityRejection::PacketCrossesGuardedBoundary);
    }
    let channel_index = channel_index_for_global_slot(earliest_slot);
    Ok(TurboOpportunity {
        channel_index,
        frequency: US915_TURBO_CHANNELS[channel_index],
        global_slot: earliest_slot,
        cycle: supercycle_cycle_at(earliest),
        transmit_must_end_by_schedule_us,
    })
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn every_schedule_time_selects_a_valid_turbo_channel() {
        let schedule_us: u64 = kani::any();
        assert!(channel_index_at(schedule_us) < TURBO_CHANNEL_COUNT);
    }

    #[kani::proof]
    fn every_cycle_and_position_selects_a_valid_channel() {
        let cycle: u8 = kani::any();
        let position: u8 = kani::any();
        kani::assume(cycle < TURBO_CHANNEL_COUNT as u8);
        kani::assume(position < TURBO_CHANNEL_COUNT as u8);
        let global_slot = cycle as u64 * TURBO_CHANNEL_COUNT as u64 + position as u64;
        assert!(channel_index_for_global_slot(global_slot) < TURBO_CHANNEL_COUNT);
    }
}
