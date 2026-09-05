use super::clock::{ClockWindow, ScheduleMicros};
use super::frame::EncodedDatagram;
use super::profile::TurboPhyProfile;
use super::spec::{Us915TurboSpec, TURBO_CHANNEL_COUNT, US915_TURBO_SPEC};
use crate::interfaces::subghz::Frequency;

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
pub struct TurboGlobalSlot(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboGlobalSlotError {
    OutsideScheduleRange { index: u64, maximum_index: u64 },
}

impl TurboGlobalSlot {
    pub const fn new(index: u64) -> Result<Self, TurboGlobalSlotError> {
        let maximum_index = u64::MAX / US915_TURBO_SPEC.slot_us();
        if index > maximum_index {
            return Err(TurboGlobalSlotError::OutsideScheduleRange {
                index,
                maximum_index,
            });
        }
        Ok(Self(index))
    }

    pub const fn index(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboSlotPosition(u8);

impl TurboSlotPosition {
    pub const fn index(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboChannelIndex(u8);

impl TurboChannelIndex {
    pub const fn new(index: usize) -> Result<Self, ChannelLookupError> {
        if index >= TURBO_CHANNEL_COUNT {
            return Err(ChannelLookupError::OutsideHopSet {
                channel_index: index,
            });
        }
        Ok(Self(index as u8))
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurboScheduleSlot {
    global_slot: TurboGlobalSlot,
    supercycle: u64,
    cycle: SupercycleCycle,
    position: TurboSlotPosition,
    channel_index: TurboChannelIndex,
    frequency: Frequency,
    starts_at: ScheduleMicros,
    offset_us: u64,
    remaining_us: u64,
}

impl TurboScheduleSlot {
    pub const fn global_slot(self) -> TurboGlobalSlot {
        self.global_slot
    }

    pub const fn supercycle(self) -> u64 {
        self.supercycle
    }

    pub const fn cycle(self) -> SupercycleCycle {
        self.cycle
    }

    pub const fn position(self) -> TurboSlotPosition {
        self.position
    }

    pub const fn channel_index(self) -> TurboChannelIndex {
        self.channel_index
    }

    pub const fn frequency(self) -> Frequency {
        self.frequency
    }

    pub const fn starts_at(self) -> ScheduleMicros {
        self.starts_at
    }

    pub const fn offset_us(self) -> u64 {
        self.offset_us
    }

    pub const fn remaining_us(self) -> u64 {
        self.remaining_us
    }
}

impl Us915TurboSpec {
    pub const fn slot_at(&self, schedule: ScheduleMicros) -> TurboScheduleSlot {
        let global_slot = TurboGlobalSlot(schedule.micros() / self.slot_us());
        let offset_us = schedule.micros() % self.slot_us();
        self.build_slot(global_slot, offset_us)
    }

    pub const fn slot_for_global_slot(&self, global_slot: TurboGlobalSlot) -> TurboScheduleSlot {
        self.build_slot(global_slot, 0)
    }

    pub const fn slot_position_for_channel(
        &self,
        cycle: SupercycleCycle,
        channel_index: TurboChannelIndex,
    ) -> TurboSlotPosition {
        let order_position = self.channel_order_position(channel_index.index()) as usize;
        let position =
            (order_position + TURBO_CHANNEL_COUNT - cycle.index() as usize) % TURBO_CHANNEL_COUNT;
        TurboSlotPosition(position as u8)
    }

    const fn build_slot(&self, global_slot: TurboGlobalSlot, offset_us: u64) -> TurboScheduleSlot {
        let base_cycle = global_slot.index() / TURBO_CHANNEL_COUNT as u64;
        let position = global_slot.index() % TURBO_CHANNEL_COUNT as u64;
        let cycle = SupercycleCycle::from_base_cycle(base_cycle);
        let order_position = (position + cycle.index() as u64) % TURBO_CHANNEL_COUNT as u64;
        let channel_index = TurboChannelIndex(self.channel_order_at(order_position as usize));
        TurboScheduleSlot {
            global_slot,
            supercycle: base_cycle / TURBO_CHANNEL_COUNT as u64,
            cycle,
            position: TurboSlotPosition(position as u8),
            channel_index,
            frequency: self.channels()[channel_index.index()],
            starts_at: ScheduleMicros::new(global_slot.index() * self.slot_us()),
            offset_us,
            remaining_us: self.slot_us() - offset_us,
        }
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
            >= US915_TURBO_SPEC.slot_us()
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
    channel_index: TurboChannelIndex,
    frequency: Frequency,
    global_slot: TurboGlobalSlot,
    cycle: SupercycleCycle,
    transmit_must_end_by_schedule_us: u64,
}

impl TurboOpportunity {
    pub const fn channel_index(self) -> TurboChannelIndex {
        self.channel_index
    }

    pub const fn frequency(self) -> Frequency {
        self.frequency
    }

    pub const fn global_slot(self) -> TurboGlobalSlot {
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
    let earliest_slot = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(earliest));
    let latest_slot = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(latest));
    if earliest_slot.global_slot() != latest_slot.global_slot() {
        return Err(OpportunityRejection::ClockWindowCrossesChannelBoundary);
    }
    let slot_begins_at_schedule_us = earliest_slot.starts_at().micros();
    let transmit_not_before_schedule_us = slot_begins_at_schedule_us
        .checked_add(timing.entry_guard_us())
        .ok_or(OpportunityRejection::ClockRangeOverflow)?;
    if earliest < transmit_not_before_schedule_us {
        return Err(OpportunityRejection::EntryGuard);
    }
    let slot_ends_at_schedule_us = slot_begins_at_schedule_us
        .checked_add(US915_TURBO_SPEC.slot_us())
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
    Ok(TurboOpportunity {
        channel_index: earliest_slot.channel_index(),
        frequency: earliest_slot.frequency(),
        global_slot: earliest_slot.global_slot(),
        cycle: earliest_slot.cycle(),
        transmit_must_end_by_schedule_us,
    })
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn every_schedule_time_selects_a_valid_turbo_channel() {
        let schedule_us: u64 = kani::any();
        let slot = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(schedule_us));
        assert!(slot.channel_index().index() < TURBO_CHANNEL_COUNT);
    }

    #[kani::proof]
    fn global_slot_construction_matches_representable_start_times() {
        let index: u64 = kani::any();
        let maximum_index = u64::MAX / US915_TURBO_SPEC.slot_us();
        match TurboGlobalSlot::new(index) {
            Ok(global_slot) => {
                assert!(global_slot.index() <= maximum_index);
                assert!(global_slot
                    .index()
                    .checked_mul(US915_TURBO_SPEC.slot_us())
                    .is_some());
            }
            Err(TurboGlobalSlotError::OutsideScheduleRange {
                index,
                maximum_index: rejected_maximum,
            }) => {
                assert_eq!(rejected_maximum, maximum_index);
                assert!(index > maximum_index);
            }
        }
    }

    #[kani::proof]
    fn schedule_slot_reconstructs_bounded_time() {
        let schedule_us: u32 = kani::any();
        let schedule_us = schedule_us as u64;
        let slot = US915_TURBO_SPEC.slot_at(ScheduleMicros::new(schedule_us));
        assert!(slot.offset_us() < US915_TURBO_SPEC.slot_us());
        assert!(slot.starts_at().micros() <= schedule_us);
        assert_eq!(schedule_us - slot.starts_at().micros(), slot.offset_us());
        assert_eq!(
            slot.offset_us() + slot.remaining_us(),
            US915_TURBO_SPEC.slot_us()
        );
    }

    #[kani::proof]
    fn every_cycle_and_position_selects_a_valid_channel() {
        let cycle: u8 = kani::any();
        let position: u8 = kani::any();
        kani::assume(cycle < TURBO_CHANNEL_COUNT as u8);
        kani::assume(position < TURBO_CHANNEL_COUNT as u8);
        let global_slot =
            TurboGlobalSlot(cycle as u64 * TURBO_CHANNEL_COUNT as u64 + position as u64);
        assert!(
            US915_TURBO_SPEC
                .slot_for_global_slot(global_slot)
                .channel_index()
                .index()
                < TURBO_CHANNEL_COUNT
        );
    }

    #[kani::proof]
    fn schedule_repeats_after_one_complete_supercycle() {
        let global_slot: u16 = kani::any();
        let global_slot = TurboGlobalSlot(global_slot as u64);
        let repeated_global_slot =
            TurboGlobalSlot(global_slot.index() + US915_TURBO_SPEC.supercycle_slots());
        let current = US915_TURBO_SPEC.slot_for_global_slot(global_slot);
        let repeated = US915_TURBO_SPEC.slot_for_global_slot(repeated_global_slot);
        assert_eq!(current.cycle(), repeated.cycle());
        assert_eq!(current.position(), repeated.position());
        assert_eq!(current.channel_index(), repeated.channel_index());
    }

    #[kani::proof]
    fn channel_position_lookup_round_trips() {
        let cycle: u8 = kani::any();
        let channel: u8 = kani::any();
        kani::assume(cycle < TURBO_CHANNEL_COUNT as u8);
        kani::assume(channel < TURBO_CHANNEL_COUNT as u8);
        let cycle = SupercycleCycle(cycle);
        let channel = TurboChannelIndex(channel);
        let position = US915_TURBO_SPEC.slot_position_for_channel(cycle, channel);
        let global_slot = TurboGlobalSlot(
            cycle.index() as u64 * TURBO_CHANNEL_COUNT as u64 + position.index() as u64,
        );
        assert_eq!(
            US915_TURBO_SPEC
                .slot_for_global_slot(global_slot)
                .channel_index(),
            channel
        );
    }
}
