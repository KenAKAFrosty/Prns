#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OledAutoOff {
    Enabled,
    Disabled,
}

impl OledAutoOff {
    const fn toggled(self) -> Self {
        match self {
            Self::Enabled => Self::Disabled,
            Self::Disabled => Self::Enabled,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OledDarkReason {
    /// Only the panel is dark. The first button press wakes it and must not reach the UI.
    DisplayOnly,
    /// The interfaces and UI are sleeping too. A button press must reach the UI's wake action.
    SystemSleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OledPowerCommand {
    None,
    TurnOn,
    TurnOff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OledButtonOutcome {
    ForwardToUi,
    WakeAndConsume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OledPowerState {
    Unavailable,
    LitIndefinitely,
    LitUntilAutoOff {
        at_ms: u64,
    },
    LitUntilDark {
        at_ms: u64,
        reason: OledDarkReason,
        auto_off: OledAutoOff,
    },
    Dark {
        reason: OledDarkReason,
        auto_off: OledAutoOff,
    },
}

impl OledPowerState {
    #[must_use]
    pub const fn new(available: bool, now_ms: u64, auto_off_after_ms: u64) -> Self {
        if available {
            Self::lit(OledAutoOff::Enabled, now_ms, auto_off_after_ms)
        } else {
            Self::Unavailable
        }
    }

    #[must_use]
    pub const fn is_lit(self) -> bool {
        matches!(
            self,
            Self::LitIndefinitely | Self::LitUntilAutoOff { .. } | Self::LitUntilDark { .. }
        )
    }

    #[must_use]
    pub const fn auto_off(self) -> Option<OledAutoOff> {
        match self {
            Self::Unavailable => None,
            Self::LitIndefinitely => Some(OledAutoOff::Disabled),
            Self::LitUntilAutoOff { .. } => Some(OledAutoOff::Enabled),
            Self::LitUntilDark { auto_off, .. } | Self::Dark { auto_off, .. } => Some(auto_off),
        }
    }

    /// Advance a reached power deadline and report the hardware operation it requires.
    pub fn tick(&mut self, now_ms: u64) -> OledPowerCommand {
        match *self {
            Self::LitUntilAutoOff { at_ms } if now_ms >= at_ms => {
                *self = Self::Dark {
                    reason: OledDarkReason::DisplayOnly,
                    auto_off: OledAutoOff::Enabled,
                };
                OledPowerCommand::TurnOff
            }
            Self::LitUntilDark {
                at_ms,
                reason,
                auto_off,
            } if now_ms >= at_ms => {
                *self = Self::Dark { reason, auto_off };
                OledPowerCommand::TurnOff
            }
            Self::Unavailable
            | Self::LitIndefinitely
            | Self::LitUntilAutoOff { .. }
            | Self::LitUntilDark { .. }
            | Self::Dark { .. } => OledPowerCommand::None,
        }
    }

    /// Apply display-local button behavior before forwarding an input to [`UiState`](super::UiState).
    pub fn button_pressed(&mut self, now_ms: u64, auto_off_after_ms: u64) -> OledButtonOutcome {
        match *self {
            Self::Dark {
                reason: OledDarkReason::DisplayOnly,
                auto_off,
            } => {
                *self = Self::lit(auto_off, now_ms, auto_off_after_ms);
                OledButtonOutcome::WakeAndConsume
            }
            Self::LitUntilDark { auto_off, .. } => {
                *self = Self::lit(auto_off, now_ms, auto_off_after_ms);
                OledButtonOutcome::ForwardToUi
            }
            Self::LitUntilAutoOff { .. } => {
                *self = Self::lit(OledAutoOff::Enabled, now_ms, auto_off_after_ms);
                OledButtonOutcome::ForwardToUi
            }
            Self::Unavailable
            | Self::LitIndefinitely
            | Self::Dark {
                reason: OledDarkReason::SystemSleep,
                ..
            } => OledButtonOutcome::ForwardToUi,
        }
    }

    pub fn schedule_display_off(&mut self, at_ms: u64) {
        self.schedule_dark(at_ms, OledDarkReason::DisplayOnly);
    }

    pub fn schedule_system_sleep(&mut self, at_ms: u64) {
        self.schedule_dark(at_ms, OledDarkReason::SystemSleep);
    }

    pub fn toggle_auto_off(&mut self, now_ms: u64, auto_off_after_ms: u64) -> Option<OledAutoOff> {
        let auto_off = self.auto_off()?.toggled();
        *self = match *self {
            Self::Unavailable => return None,
            Self::LitIndefinitely | Self::LitUntilAutoOff { .. } => {
                Self::lit(auto_off, now_ms, auto_off_after_ms)
            }
            Self::LitUntilDark { at_ms, reason, .. } => Self::LitUntilDark {
                at_ms,
                reason,
                auto_off,
            },
            Self::Dark { reason, .. } => Self::Dark { reason, auto_off },
        };
        Some(auto_off)
    }

    /// Return from system sleep, reporting whether the physical panel must be powered on.
    pub fn wake(&mut self, now_ms: u64, auto_off_after_ms: u64) -> OledPowerCommand {
        let Some(auto_off) = self.auto_off() else {
            return OledPowerCommand::None;
        };
        let command = if matches!(self, Self::Dark { .. }) {
            OledPowerCommand::TurnOn
        } else {
            OledPowerCommand::None
        };
        *self = Self::lit(auto_off, now_ms, auto_off_after_ms);
        command
    }

    const fn lit(auto_off: OledAutoOff, now_ms: u64, auto_off_after_ms: u64) -> Self {
        match auto_off {
            OledAutoOff::Enabled => Self::LitUntilAutoOff {
                at_ms: now_ms.saturating_add(auto_off_after_ms),
            },
            OledAutoOff::Disabled => Self::LitIndefinitely,
        }
    }

    fn schedule_dark(&mut self, at_ms: u64, reason: OledDarkReason) {
        let auto_off = match *self {
            Self::LitIndefinitely => OledAutoOff::Disabled,
            Self::LitUntilAutoOff { .. } => OledAutoOff::Enabled,
            Self::LitUntilDark { auto_off, .. } => auto_off,
            Self::Unavailable | Self::Dark { .. } => return,
        };
        *self = Self::LitUntilDark {
            at_ms,
            reason,
            auto_off,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTO_OFF_MS: u64 = 60;

    #[test]
    fn available_display_starts_with_auto_off_armed() {
        assert_eq!(
            OledPowerState::new(true, 10, AUTO_OFF_MS),
            OledPowerState::LitUntilAutoOff { at_ms: 70 }
        );
        assert_eq!(
            OledPowerState::new(false, 10, AUTO_OFF_MS),
            OledPowerState::Unavailable
        );
    }

    #[test]
    fn auto_off_becomes_display_only_dark_and_consumes_exactly_the_wake_press() {
        let mut state = OledPowerState::new(true, 0, AUTO_OFF_MS);
        assert_eq!(state.tick(59), OledPowerCommand::None);
        assert_eq!(state.tick(60), OledPowerCommand::TurnOff);
        assert_eq!(
            state,
            OledPowerState::Dark {
                reason: OledDarkReason::DisplayOnly,
                auto_off: OledAutoOff::Enabled,
            }
        );

        assert_eq!(
            state.button_pressed(75, AUTO_OFF_MS),
            OledButtonOutcome::WakeAndConsume
        );
        assert_eq!(state, OledPowerState::LitUntilAutoOff { at_ms: 135 });
        assert_eq!(
            state.button_pressed(80, AUTO_OFF_MS),
            OledButtonOutcome::ForwardToUi
        );
    }

    #[test]
    fn toggling_auto_off_has_no_deadline_when_disabled_and_rearms_when_enabled() {
        let mut state = OledPowerState::new(true, 0, AUTO_OFF_MS);
        assert_eq!(
            state.toggle_auto_off(5, AUTO_OFF_MS),
            Some(OledAutoOff::Disabled)
        );
        assert_eq!(state, OledPowerState::LitIndefinitely);
        assert_eq!(state.tick(u64::MAX), OledPowerCommand::None);

        assert_eq!(
            state.toggle_auto_off(10, AUTO_OFF_MS),
            Some(OledAutoOff::Enabled)
        );
        assert_eq!(state, OledPowerState::LitUntilAutoOff { at_ms: 70 });
    }

    #[test]
    fn pending_display_off_is_cancelled_by_a_forwarded_press() {
        let mut state = OledPowerState::new(true, 0, AUTO_OFF_MS);
        state.schedule_display_off(5);
        assert_eq!(
            state,
            OledPowerState::LitUntilDark {
                at_ms: 5,
                reason: OledDarkReason::DisplayOnly,
                auto_off: OledAutoOff::Enabled,
            }
        );
        assert_eq!(
            state.button_pressed(3, AUTO_OFF_MS),
            OledButtonOutcome::ForwardToUi
        );
        assert_eq!(state, OledPowerState::LitUntilAutoOff { at_ms: 63 });
    }

    #[test]
    fn system_sleep_darkness_forwards_the_press_then_wake_powers_the_panel() {
        let mut state = OledPowerState::new(true, 0, AUTO_OFF_MS);
        state.schedule_system_sleep(5);
        assert_eq!(state.tick(5), OledPowerCommand::TurnOff);
        assert_eq!(
            state,
            OledPowerState::Dark {
                reason: OledDarkReason::SystemSleep,
                auto_off: OledAutoOff::Enabled,
            }
        );
        assert_eq!(
            state.button_pressed(10, AUTO_OFF_MS),
            OledButtonOutcome::ForwardToUi
        );
        assert_eq!(state.wake(10, AUTO_OFF_MS), OledPowerCommand::TurnOn);
        assert_eq!(state, OledPowerState::LitUntilAutoOff { at_ms: 70 });
    }

    #[test]
    fn waking_before_the_sleep_deadline_does_not_power_cycle_the_lit_panel() {
        let mut state = OledPowerState::new(true, 0, AUTO_OFF_MS);
        state.schedule_system_sleep(10);
        assert_eq!(
            state.button_pressed(5, AUTO_OFF_MS),
            OledButtonOutcome::ForwardToUi
        );
        assert_eq!(state.wake(5, AUTO_OFF_MS), OledPowerCommand::None);
        assert!(state.is_lit());
    }
}
