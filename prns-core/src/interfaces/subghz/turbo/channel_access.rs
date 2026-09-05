use super::super::MonotonicMicros;
use super::schedule::TURBO_CHANNEL_COUNT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalPacketTxop {
    One,
    Two,
    Four,
}

pub const TURBO_SELECTED_TXOP: LogicalPacketTxop = LogicalPacketTxop::One;

impl LogicalPacketTxop {
    pub const fn packet_limit(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Four => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentionClass {
    Fresh { recent_airtime_per_mille: u16 },
    Continuation,
}

impl ContentionClass {
    const fn band(self) -> u8 {
        match self {
            Self::Continuation => 0,
            Self::Fresh {
                recent_airtime_per_mille: 0..=70,
            } => 0,
            Self::Fresh {
                recent_airtime_per_mille: 71..=450,
            } => 1,
            Self::Fresh {
                recent_airtime_per_mille: 451..=840,
            } => 2,
            Self::Fresh { .. } => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentionPolicy {
    slot_us: u64,
    difs_slots: u8,
    contention_band_slots: u8,
    pending_ttl_us: u64,
    txop: LogicalPacketTxop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentionPolicyError {
    EmptySlot,
    EmptyDifs,
    EmptyContentionBand,
    EmptyPendingLifetime,
}

impl ContentionPolicy {
    pub const fn new(
        slot_us: u64,
        difs_slots: u8,
        contention_band_slots: u8,
        pending_ttl_us: u64,
        txop: LogicalPacketTxop,
    ) -> Result<Self, ContentionPolicyError> {
        if slot_us == 0 {
            return Err(ContentionPolicyError::EmptySlot);
        }
        if difs_slots == 0 {
            return Err(ContentionPolicyError::EmptyDifs);
        }
        if contention_band_slots == 0 {
            return Err(ContentionPolicyError::EmptyContentionBand);
        }
        if pending_ttl_us == 0 {
            return Err(ContentionPolicyError::EmptyPendingLifetime);
        }
        Ok(Self {
            slot_us,
            difs_slots,
            contention_band_slots,
            pending_ttl_us,
            txop,
        })
    }

    pub const fn turbo() -> Self {
        Self {
            slot_us: 1_000,
            difs_slots: 2,
            contention_band_slots: 15,
            pending_ttl_us: 30_000_000,
            txop: TURBO_SELECTED_TXOP,
        }
    }

    pub const fn txop(self) -> LogicalPacketTxop {
        self.txop
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAccessEvent {
    Clear,
    Busy,
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FinalClearGrant {
    channel_index: usize,
    issued_at: MonotonicMicros,
}

impl FinalClearGrant {
    pub(crate) const fn channel_index(&self) -> usize {
        self.channel_index
    }

    pub(crate) const fn issued_at(&self) -> MonotonicMicros {
        self.issued_at
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChannelAccessAction {
    Wait,
    PerformFinalClear,
    Granted(FinalClearGrant),
    NeedTieBreakEntropy,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAccessError {
    ChannelOutsideHopSet { channel_index: usize },
    MonotonicTimeWentBackward,
    TieBreakEntropyNotExpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAccessState {
    WaitingForClear {
        clear_us: u64,
        remaining_backoff_us: u64,
    },
    Backoff {
        remaining_us: u64,
    },
    FinalCheck,
    AwaitingTieBreakEntropy,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelAccess {
    policy: ContentionPolicy,
    channel_index: usize,
    state: ChannelAccessState,
    last_observed_at: MonotonicMicros,
    expires_at: MonotonicMicros,
}

impl ChannelAccess {
    pub const fn begin(
        policy: ContentionPolicy,
        channel_index: usize,
        class: ContentionClass,
        entropy: u16,
        now: MonotonicMicros,
    ) -> Result<Self, ChannelAccessError> {
        if channel_index >= TURBO_CHANNEL_COUNT {
            return Err(ChannelAccessError::ChannelOutsideHopSet { channel_index });
        }
        let band_width_us = policy
            .slot_us
            .saturating_mul(policy.contention_band_slots as u64);
        let band_start_us = band_width_us.saturating_mul(class.band() as u64);
        let random_us = (entropy as u64).saturating_mul(band_width_us) >> 16;
        Ok(Self {
            policy,
            channel_index,
            state: ChannelAccessState::WaitingForClear {
                clear_us: 0,
                remaining_backoff_us: band_start_us.saturating_add(random_us),
            },
            last_observed_at: now,
            expires_at: MonotonicMicros::new(now.micros().saturating_add(policy.pending_ttl_us)),
        })
    }

    pub const fn state(self) -> ChannelAccessState {
        self.state
    }

    pub fn observe(
        &mut self,
        now: MonotonicMicros,
        event: ChannelAccessEvent,
    ) -> Result<ChannelAccessAction, ChannelAccessError> {
        if now < self.last_observed_at {
            return Err(ChannelAccessError::MonotonicTimeWentBackward);
        }
        if now >= self.expires_at {
            self.state = ChannelAccessState::Complete;
            return Ok(ChannelAccessAction::Expired);
        }
        let elapsed_us = now.micros().saturating_sub(self.last_observed_at.micros());
        self.last_observed_at = now;
        let action = match (self.state, event) {
            (ChannelAccessState::Complete, _) => ChannelAccessAction::Expired,
            (ChannelAccessState::AwaitingTieBreakEntropy, _) => {
                ChannelAccessAction::NeedTieBreakEntropy
            }
            (ChannelAccessState::FinalCheck, ChannelAccessEvent::Clear) => {
                self.state = ChannelAccessState::Complete;
                ChannelAccessAction::Granted(FinalClearGrant {
                    channel_index: self.channel_index,
                    issued_at: now,
                })
            }
            (
                ChannelAccessState::FinalCheck,
                ChannelAccessEvent::Busy | ChannelAccessEvent::Unknown,
            ) => {
                self.state = ChannelAccessState::AwaitingTieBreakEntropy;
                ChannelAccessAction::NeedTieBreakEntropy
            }
            (
                ChannelAccessState::WaitingForClear {
                    remaining_backoff_us,
                    ..
                }
                | ChannelAccessState::Backoff {
                    remaining_us: remaining_backoff_us,
                },
                ChannelAccessEvent::Busy | ChannelAccessEvent::Unknown,
            ) => {
                self.state = ChannelAccessState::WaitingForClear {
                    clear_us: 0,
                    remaining_backoff_us,
                };
                ChannelAccessAction::Wait
            }
            (
                ChannelAccessState::WaitingForClear {
                    clear_us,
                    remaining_backoff_us,
                },
                ChannelAccessEvent::Clear,
            ) => {
                let difs_us = self
                    .policy
                    .slot_us
                    .saturating_mul(self.policy.difs_slots as u64);
                let total_clear_us = clear_us.saturating_add(elapsed_us);
                if total_clear_us < difs_us {
                    self.state = ChannelAccessState::WaitingForClear {
                        clear_us: total_clear_us,
                        remaining_backoff_us,
                    };
                    return Ok(ChannelAccessAction::Wait);
                }
                self.advance_backoff(remaining_backoff_us, total_clear_us.saturating_sub(difs_us))
            }
            (ChannelAccessState::Backoff { remaining_us }, ChannelAccessEvent::Clear) => {
                self.advance_backoff(remaining_us, elapsed_us)
            }
        };
        Ok(action)
    }

    pub fn supply_tie_break_entropy(
        &mut self,
        entropy: u16,
    ) -> Result<ChannelAccessAction, ChannelAccessError> {
        if !matches!(self.state, ChannelAccessState::AwaitingTieBreakEntropy) {
            return Err(ChannelAccessError::TieBreakEntropyNotExpected);
        }
        let band_width_us = self
            .policy
            .slot_us
            .saturating_mul(self.policy.contention_band_slots as u64);
        let random_us = (entropy as u64).saturating_mul(band_width_us) >> 16;
        self.state = ChannelAccessState::WaitingForClear {
            clear_us: 0,
            remaining_backoff_us: random_us,
        };
        Ok(ChannelAccessAction::Wait)
    }

    fn advance_backoff(&mut self, remaining_us: u64, progress_us: u64) -> ChannelAccessAction {
        let remaining_us = remaining_us.saturating_sub(progress_us);
        if remaining_us == 0 {
            self.state = ChannelAccessState::FinalCheck;
            ChannelAccessAction::PerformFinalClear
        } else {
            self.state = ChannelAccessState::Backoff { remaining_us };
            ChannelAccessAction::Wait
        }
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(9)]
    fn channel_access_never_grants_without_a_final_clear() {
        let entropy: u16 = kani::any();
        let mut access = ChannelAccess::begin(
            ContentionPolicy::turbo(),
            0,
            ContentionClass::Fresh {
                recent_airtime_per_mille: kani::any(),
            },
            entropy,
            MonotonicMicros::new(0),
        )
        .unwrap();
        let mut final_check_requested = false;
        let mut now = 0u64;
        for _ in 0..8 {
            now = now.saturating_add(kani::any::<u8>() as u64 * 1_000 + 1);
            let event = match kani::any::<u8>() % 3 {
                0 => ChannelAccessEvent::Clear,
                1 => ChannelAccessEvent::Busy,
                _ => ChannelAccessEvent::Unknown,
            };
            let action = access.observe(MonotonicMicros::new(now), event);
            if matches!(action, Ok(ChannelAccessAction::PerformFinalClear)) {
                final_check_requested = true;
            }
            if matches!(action, Ok(ChannelAccessAction::Granted(_))) {
                assert!(final_check_requested);
                assert_eq!(event, ChannelAccessEvent::Clear);
            }
        }
    }
}
