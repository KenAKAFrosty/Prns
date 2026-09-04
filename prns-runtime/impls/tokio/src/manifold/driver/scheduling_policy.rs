#[cfg(any(feature = "scheduler-tuning", test))]
use core::fmt;
#[cfg(any(feature = "scheduler-tuning", test))]
use core::num::NonZeroUsize;

pub(super) const MAX_INTERACTIVE_CRYPTO_BATCH: usize = 8;

#[cfg(any(feature = "scheduler-tuning", test))]
#[derive(Debug, PartialEq, Eq)]
pub struct SchedulerPolicyInput {
    pub turn_work: NonZeroUsize,
    pub completion_batch: NonZeroUsize,
    pub inbound_total: NonZeroUsize,
    pub inbound_per_lane: NonZeroUsize,
    pub command_batch: NonZeroUsize,
    pub owed_work_batch: NonZeroUsize,
    pub hot_turns: usize,
    pub worker_hot_idle_turns: usize,
    pub worker_spin_idle_turns: usize,
    pub interactive_batch: NonZeroUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerPolicy {
    turn_work: usize,
    completion_batch: usize,
    inbound_total: usize,
    inbound_per_lane: usize,
    command_batch: usize,
    owed_work_batch: usize,
    hot_turns: usize,
    worker_hot_idle_turns: usize,
    worker_spin_idle_turns: usize,
    interactive_batch: usize,
}

#[cfg(any(feature = "scheduler-tuning", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerPolicyError {
    CompletionBatchExceedsTurnWork {
        completion_batch: usize,
        turn_work: usize,
    },
    InboundTotalExceedsTurnWork {
        inbound_total: usize,
        turn_work: usize,
    },
    InboundPerLaneExceedsInboundTotal {
        inbound_per_lane: usize,
        inbound_total: usize,
    },
    CommandBatchExceedsTurnWork {
        command_batch: usize,
        turn_work: usize,
    },
    OwedWorkBatchExceedsTurnWork {
        owed_work_batch: usize,
        turn_work: usize,
    },
    InteractiveBatchExceedsCapacity {
        interactive_batch: usize,
        capacity: usize,
    },
}

impl SchedulerPolicy {
    const PRODUCTION: Self = Self {
        turn_work: 64,
        completion_batch: 16,
        inbound_total: 24,
        inbound_per_lane: 8,
        command_batch: 16,
        owed_work_batch: 8,
        hot_turns: 16,
        worker_hot_idle_turns: 2,
        worker_spin_idle_turns: 8_192,
        interactive_batch: 8,
    };

    #[must_use]
    pub const fn production() -> Self {
        Self::PRODUCTION
    }

    #[cfg(any(feature = "scheduler-tuning", test))]
    pub fn new(input: SchedulerPolicyInput) -> Result<Self, SchedulerPolicyError> {
        let policy = Self {
            turn_work: input.turn_work.get(),
            completion_batch: input.completion_batch.get(),
            inbound_total: input.inbound_total.get(),
            inbound_per_lane: input.inbound_per_lane.get(),
            command_batch: input.command_batch.get(),
            owed_work_batch: input.owed_work_batch.get(),
            hot_turns: input.hot_turns,
            worker_hot_idle_turns: input.worker_hot_idle_turns,
            worker_spin_idle_turns: input.worker_spin_idle_turns,
            interactive_batch: input.interactive_batch.get(),
        };
        policy.validate()?;
        Ok(policy)
    }

    pub const fn turn_work(self) -> usize {
        self.turn_work
    }

    #[cfg(any(feature = "scheduler-tuning", test))]
    pub const fn completion_batch(self) -> usize {
        self.completion_batch
    }

    #[cfg(any(feature = "scheduler-tuning", test))]
    pub const fn inbound_total(self) -> usize {
        self.inbound_total
    }

    pub const fn inbound_per_lane(self) -> usize {
        self.inbound_per_lane
    }

    #[cfg(any(feature = "scheduler-tuning", test))]
    pub const fn command_batch(self) -> usize {
        self.command_batch
    }

    #[cfg(any(feature = "scheduler-tuning", test))]
    pub const fn owed_work_batch(self) -> usize {
        self.owed_work_batch
    }

    pub const fn hot_turns(self) -> usize {
        self.hot_turns
    }

    pub const fn worker_hot_idle_turns(self) -> usize {
        self.worker_hot_idle_turns
    }

    pub const fn worker_spin_idle_turns(self) -> usize {
        self.worker_spin_idle_turns
    }

    pub const fn interactive_batch(self) -> usize {
        self.interactive_batch
    }

    pub(crate) const fn completion_turn_budget(self, work_remaining: usize) -> usize {
        liveness_preserving_budget(work_remaining, self.completion_batch)
    }

    pub(crate) const fn inbound_turn_budget(self, work_remaining: usize) -> usize {
        liveness_preserving_budget(work_remaining, self.inbound_total)
    }

    pub(crate) const fn command_turn_budget(self, work_remaining: usize) -> usize {
        liveness_preserving_budget(work_remaining, self.command_batch)
    }

    pub(crate) const fn owed_work_turn_budget(self, work_remaining: usize) -> usize {
        liveness_preserving_budget(work_remaining, self.owed_work_batch)
    }

    #[cfg(any(feature = "scheduler-tuning", test))]
    fn validate(self) -> Result<(), SchedulerPolicyError> {
        if self.completion_batch > self.turn_work {
            return Err(SchedulerPolicyError::CompletionBatchExceedsTurnWork {
                completion_batch: self.completion_batch,
                turn_work: self.turn_work,
            });
        }
        if self.inbound_total > self.turn_work {
            return Err(SchedulerPolicyError::InboundTotalExceedsTurnWork {
                inbound_total: self.inbound_total,
                turn_work: self.turn_work,
            });
        }
        if self.inbound_per_lane > self.inbound_total {
            return Err(SchedulerPolicyError::InboundPerLaneExceedsInboundTotal {
                inbound_per_lane: self.inbound_per_lane,
                inbound_total: self.inbound_total,
            });
        }
        if self.command_batch > self.turn_work {
            return Err(SchedulerPolicyError::CommandBatchExceedsTurnWork {
                command_batch: self.command_batch,
                turn_work: self.turn_work,
            });
        }
        if self.owed_work_batch > self.turn_work {
            return Err(SchedulerPolicyError::OwedWorkBatchExceedsTurnWork {
                owed_work_batch: self.owed_work_batch,
                turn_work: self.turn_work,
            });
        }
        if self.interactive_batch > MAX_INTERACTIVE_CRYPTO_BATCH {
            return Err(SchedulerPolicyError::InteractiveBatchExceedsCapacity {
                interactive_batch: self.interactive_batch,
                capacity: MAX_INTERACTIVE_CRYPTO_BATCH,
            });
        }
        Ok(())
    }
}

const fn liveness_preserving_budget(work_remaining: usize, batch_limit: usize) -> usize {
    let available = if work_remaining < batch_limit {
        work_remaining
    } else {
        batch_limit
    };
    if available == 0 {
        1
    } else {
        available
    }
}

#[cfg(any(feature = "scheduler-tuning", test))]
impl fmt::Display for SchedulerPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CompletionBatchExceedsTurnWork {
                completion_batch,
                turn_work,
            } => write!(
                formatter,
                "completion batch {completion_batch} exceeds turn work {turn_work}"
            ),
            Self::InboundTotalExceedsTurnWork {
                inbound_total,
                turn_work,
            } => write!(
                formatter,
                "inbound total {inbound_total} exceeds turn work {turn_work}"
            ),
            Self::InboundPerLaneExceedsInboundTotal {
                inbound_per_lane,
                inbound_total,
            } => write!(
                formatter,
                "inbound per lane {inbound_per_lane} exceeds inbound total {inbound_total}"
            ),
            Self::CommandBatchExceedsTurnWork {
                command_batch,
                turn_work,
            } => write!(
                formatter,
                "command batch {command_batch} exceeds turn work {turn_work}"
            ),
            Self::OwedWorkBatchExceedsTurnWork {
                owed_work_batch,
                turn_work,
            } => write!(
                formatter,
                "owed-work batch {owed_work_batch} exceeds turn work {turn_work}"
            ),
            Self::InteractiveBatchExceedsCapacity {
                interactive_batch,
                capacity,
            } => write!(
                formatter,
                "interactive batch {interactive_batch} exceeds worker capacity {capacity}"
            ),
        }
    }
}

#[cfg(any(feature = "scheduler-tuning", test))]
impl std::error::Error for SchedulerPolicyError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn nonzero(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).expect("test input is nonzero")
    }

    fn production_input() -> SchedulerPolicyInput {
        let policy = SchedulerPolicy::production();
        SchedulerPolicyInput {
            turn_work: nonzero(policy.turn_work()),
            completion_batch: nonzero(policy.completion_batch()),
            inbound_total: nonzero(policy.inbound_total()),
            inbound_per_lane: nonzero(policy.inbound_per_lane()),
            command_batch: nonzero(policy.command_batch()),
            owed_work_batch: nonzero(policy.owed_work_batch()),
            hot_turns: policy.hot_turns(),
            worker_hot_idle_turns: policy.worker_hot_idle_turns(),
            worker_spin_idle_turns: policy.worker_spin_idle_turns(),
            interactive_batch: nonzero(policy.interactive_batch()),
        }
    }

    #[test]
    fn production_policy_satisfies_all_invariants() {
        assert_eq!(
            SchedulerPolicy::new(production_input()),
            Ok(SchedulerPolicy::production())
        );
    }

    #[test]
    fn exhausted_turns_retain_one_slot_for_every_downstream_lane() {
        let policy = SchedulerPolicy::production();

        assert_eq!(policy.completion_turn_budget(0), 1);
        assert_eq!(policy.inbound_turn_budget(0), 1);
        assert_eq!(policy.command_turn_budget(0), 1);
        assert_eq!(policy.owed_work_turn_budget(0), 1);
    }

    #[test]
    fn live_turn_budgets_remain_bounded_by_each_lane_policy() {
        let policy = SchedulerPolicy::production();

        assert_eq!(policy.completion_turn_budget(usize::MAX), 16);
        assert_eq!(policy.inbound_turn_budget(usize::MAX), 24);
        assert_eq!(policy.command_turn_budget(usize::MAX), 16);
        assert_eq!(policy.owed_work_turn_budget(usize::MAX), 8);
    }

    #[test]
    fn validation_rejects_each_cross_field_violation() {
        let mut input = production_input();
        input.completion_batch = nonzero(input.turn_work.get() + 1);
        assert!(matches!(
            SchedulerPolicy::new(input),
            Err(SchedulerPolicyError::CompletionBatchExceedsTurnWork { .. })
        ));

        let mut input = production_input();
        input.inbound_total = nonzero(input.turn_work.get() + 1);
        assert!(matches!(
            SchedulerPolicy::new(input),
            Err(SchedulerPolicyError::InboundTotalExceedsTurnWork { .. })
        ));

        let mut input = production_input();
        input.inbound_per_lane = nonzero(input.inbound_total.get() + 1);
        assert!(matches!(
            SchedulerPolicy::new(input),
            Err(SchedulerPolicyError::InboundPerLaneExceedsInboundTotal { .. })
        ));

        let mut input = production_input();
        input.command_batch = nonzero(input.turn_work.get() + 1);
        assert!(matches!(
            SchedulerPolicy::new(input),
            Err(SchedulerPolicyError::CommandBatchExceedsTurnWork { .. })
        ));

        let mut input = production_input();
        input.owed_work_batch = nonzero(input.turn_work.get() + 1);
        assert!(matches!(
            SchedulerPolicy::new(input),
            Err(SchedulerPolicyError::OwedWorkBatchExceedsTurnWork { .. })
        ));

        let mut input = production_input();
        input.interactive_batch = nonzero(MAX_INTERACTIVE_CRYPTO_BATCH + 1);
        assert!(matches!(
            SchedulerPolicy::new(input),
            Err(SchedulerPolicyError::InteractiveBatchExceedsCapacity { .. })
        ));
    }
}
