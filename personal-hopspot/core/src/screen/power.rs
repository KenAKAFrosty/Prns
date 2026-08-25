#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayAutoOffDuration {
    milliseconds: u64,
}

impl DisplayAutoOffDuration {
    #[must_use]
    pub const fn from_millis(milliseconds: u64) -> Self {
        assert!(milliseconds > 0);
        Self { milliseconds }
    }

    #[must_use]
    pub const fn milliseconds(self) -> u64 {
        self.milliseconds
    }
}

pub const DEFAULT_DISPLAY_AUTO_OFF: DisplayAutoOffDuration =
    DisplayAutoOffDuration::from_millis(60_000);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayAutoOff {
    Enabled,
    Disabled,
}

impl DisplayAutoOff {
    const fn toggled(self) -> Self {
        match self {
            Self::Enabled => Self::Disabled,
            Self::Disabled => Self::Enabled,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayDarkReason {
    /// Only the panel is dark. The first button press wakes it and must not reach the UI.
    DisplayOnly,
    /// The interfaces and UI are sleeping too. A button press must reach the UI's wake action.
    SystemSleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayPowerCommand {
    NoChange,
    Wake,
    Darken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayButtonOutcome {
    ForwardToUi,
    WakeAndConsume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayPowerState {
    Unavailable,
    LitIndefinitely,
    LitUntilAutoOff {
        at_ms: u64,
    },
    LitUntilDark {
        at_ms: u64,
        reason: DisplayDarkReason,
        auto_off: DisplayAutoOff,
    },
    Dark {
        reason: DisplayDarkReason,
        auto_off: DisplayAutoOff,
    },
}

impl DisplayPowerState {
    #[must_use]
    pub const fn new(
        control: super::DisplayPowerControl,
        now_ms: u64,
        auto_off_after: DisplayAutoOffDuration,
    ) -> Self {
        match control {
            super::DisplayPowerControl::Available => {
                Self::lit(DisplayAutoOff::Enabled, now_ms, auto_off_after)
            }
            super::DisplayPowerControl::Unavailable => Self::Unavailable,
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
    pub const fn auto_off(self) -> Option<DisplayAutoOff> {
        match self {
            Self::Unavailable => None,
            Self::LitIndefinitely => Some(DisplayAutoOff::Disabled),
            Self::LitUntilAutoOff { .. } => Some(DisplayAutoOff::Enabled),
            Self::LitUntilDark { auto_off, .. } | Self::Dark { auto_off, .. } => Some(auto_off),
        }
    }

    /// Advance a reached power deadline and report the hardware operation it requires.
    pub fn tick(&mut self, now_ms: u64) -> DisplayPowerCommand {
        match *self {
            Self::LitUntilAutoOff { at_ms } if now_ms >= at_ms => {
                *self = Self::Dark {
                    reason: DisplayDarkReason::DisplayOnly,
                    auto_off: DisplayAutoOff::Enabled,
                };
                DisplayPowerCommand::Darken
            }
            Self::LitUntilDark {
                at_ms,
                reason,
                auto_off,
            } if now_ms >= at_ms => {
                *self = Self::Dark { reason, auto_off };
                DisplayPowerCommand::Darken
            }
            Self::Unavailable
            | Self::LitIndefinitely
            | Self::LitUntilAutoOff { .. }
            | Self::LitUntilDark { .. }
            | Self::Dark { .. } => DisplayPowerCommand::NoChange,
        }
    }

    /// Apply display-local button behavior before forwarding an input to [`UiState`](super::UiState).
    pub fn button_pressed(
        &mut self,
        now_ms: u64,
        auto_off_after: DisplayAutoOffDuration,
    ) -> DisplayButtonOutcome {
        match *self {
            Self::Dark {
                reason: DisplayDarkReason::DisplayOnly,
                auto_off,
            } => {
                *self = Self::lit(auto_off, now_ms, auto_off_after);
                DisplayButtonOutcome::WakeAndConsume
            }
            Self::LitUntilDark { auto_off, .. } => {
                *self = Self::lit(auto_off, now_ms, auto_off_after);
                DisplayButtonOutcome::ForwardToUi
            }
            Self::LitUntilAutoOff { .. } => {
                *self = Self::lit(DisplayAutoOff::Enabled, now_ms, auto_off_after);
                DisplayButtonOutcome::ForwardToUi
            }
            Self::Unavailable
            | Self::LitIndefinitely
            | Self::Dark {
                reason: DisplayDarkReason::SystemSleep,
                ..
            } => DisplayButtonOutcome::ForwardToUi,
        }
    }

    pub fn schedule_display_off(&mut self, at_ms: u64) {
        self.schedule_dark(at_ms, DisplayDarkReason::DisplayOnly);
    }

    pub fn schedule_system_sleep(&mut self, at_ms: u64) {
        self.schedule_dark(at_ms, DisplayDarkReason::SystemSleep);
    }

    pub fn mark_unavailable(&mut self) {
        *self = Self::Unavailable;
    }

    pub fn toggle_auto_off(
        &mut self,
        now_ms: u64,
        auto_off_after: DisplayAutoOffDuration,
    ) -> Option<DisplayAutoOff> {
        let auto_off = self.auto_off()?.toggled();
        *self = match *self {
            Self::Unavailable => return None,
            Self::LitIndefinitely | Self::LitUntilAutoOff { .. } => {
                Self::lit(auto_off, now_ms, auto_off_after)
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
    pub fn wake(
        &mut self,
        now_ms: u64,
        auto_off_after: DisplayAutoOffDuration,
    ) -> DisplayPowerCommand {
        let Some(auto_off) = self.auto_off() else {
            return DisplayPowerCommand::NoChange;
        };
        let command = if matches!(self, Self::Dark { .. }) {
            DisplayPowerCommand::Wake
        } else {
            DisplayPowerCommand::NoChange
        };
        *self = Self::lit(auto_off, now_ms, auto_off_after);
        command
    }

    const fn lit(
        auto_off: DisplayAutoOff,
        now_ms: u64,
        auto_off_after: DisplayAutoOffDuration,
    ) -> Self {
        match auto_off {
            DisplayAutoOff::Enabled => Self::LitUntilAutoOff {
                at_ms: now_ms.saturating_add(auto_off_after.milliseconds()),
            },
            DisplayAutoOff::Disabled => Self::LitIndefinitely,
        }
    }

    fn schedule_dark(&mut self, at_ms: u64, reason: DisplayDarkReason) {
        let auto_off = match *self {
            Self::LitIndefinitely => DisplayAutoOff::Disabled,
            Self::LitUntilAutoOff { .. } => DisplayAutoOff::Enabled,
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

    const AUTO_OFF: DisplayAutoOffDuration = DisplayAutoOffDuration::from_millis(60);

    #[test]
    fn available_display_starts_with_auto_off_armed() {
        assert_eq!(
            DisplayPowerState::new(super::super::DisplayPowerControl::Available, 10, AUTO_OFF),
            DisplayPowerState::LitUntilAutoOff { at_ms: 70 }
        );
        assert_eq!(
            DisplayPowerState::new(super::super::DisplayPowerControl::Unavailable, 10, AUTO_OFF),
            DisplayPowerState::Unavailable
        );
    }

    #[test]
    fn auto_off_becomes_display_only_dark_and_consumes_exactly_the_wake_press() {
        let mut state =
            DisplayPowerState::new(super::super::DisplayPowerControl::Available, 0, AUTO_OFF);
        assert_eq!(state.tick(59), DisplayPowerCommand::NoChange);
        assert_eq!(state.tick(60), DisplayPowerCommand::Darken);
        assert_eq!(
            state,
            DisplayPowerState::Dark {
                reason: DisplayDarkReason::DisplayOnly,
                auto_off: DisplayAutoOff::Enabled,
            }
        );

        assert_eq!(
            state.button_pressed(75, AUTO_OFF),
            DisplayButtonOutcome::WakeAndConsume
        );
        assert_eq!(state, DisplayPowerState::LitUntilAutoOff { at_ms: 135 });
        assert_eq!(
            state.button_pressed(80, AUTO_OFF),
            DisplayButtonOutcome::ForwardToUi
        );
    }

    #[test]
    fn toggling_auto_off_has_no_deadline_when_disabled_and_rearms_when_enabled() {
        let mut state =
            DisplayPowerState::new(super::super::DisplayPowerControl::Available, 0, AUTO_OFF);
        assert_eq!(
            state.toggle_auto_off(5, AUTO_OFF),
            Some(DisplayAutoOff::Disabled)
        );
        assert_eq!(state, DisplayPowerState::LitIndefinitely);
        assert_eq!(state.tick(u64::MAX), DisplayPowerCommand::NoChange);

        assert_eq!(
            state.toggle_auto_off(10, AUTO_OFF),
            Some(DisplayAutoOff::Enabled)
        );
        assert_eq!(state, DisplayPowerState::LitUntilAutoOff { at_ms: 70 });
    }

    #[test]
    fn pending_display_off_is_cancelled_by_a_forwarded_press() {
        let mut state =
            DisplayPowerState::new(super::super::DisplayPowerControl::Available, 0, AUTO_OFF);
        state.schedule_display_off(5);
        assert_eq!(
            state,
            DisplayPowerState::LitUntilDark {
                at_ms: 5,
                reason: DisplayDarkReason::DisplayOnly,
                auto_off: DisplayAutoOff::Enabled,
            }
        );
        assert_eq!(
            state.button_pressed(3, AUTO_OFF),
            DisplayButtonOutcome::ForwardToUi
        );
        assert_eq!(state, DisplayPowerState::LitUntilAutoOff { at_ms: 63 });
    }

    #[test]
    fn system_sleep_darkness_forwards_the_press_then_wake_powers_the_panel() {
        let mut state =
            DisplayPowerState::new(super::super::DisplayPowerControl::Available, 0, AUTO_OFF);
        state.schedule_system_sleep(5);
        assert_eq!(state.tick(5), DisplayPowerCommand::Darken);
        assert_eq!(
            state,
            DisplayPowerState::Dark {
                reason: DisplayDarkReason::SystemSleep,
                auto_off: DisplayAutoOff::Enabled,
            }
        );
        assert_eq!(
            state.button_pressed(10, AUTO_OFF),
            DisplayButtonOutcome::ForwardToUi
        );
        assert_eq!(state.wake(10, AUTO_OFF), DisplayPowerCommand::Wake);
        assert_eq!(state, DisplayPowerState::LitUntilAutoOff { at_ms: 70 });
    }

    #[test]
    fn waking_before_the_sleep_deadline_does_not_power_cycle_the_lit_panel() {
        let mut state =
            DisplayPowerState::new(super::super::DisplayPowerControl::Available, 0, AUTO_OFF);
        state.schedule_system_sleep(10);
        assert_eq!(
            state.button_pressed(5, AUTO_OFF),
            DisplayButtonOutcome::ForwardToUi
        );
        assert_eq!(state.wake(5, AUTO_OFF), DisplayPowerCommand::NoChange);
        assert!(state.is_lit());
    }

    #[test]
    fn a_hardware_failure_makes_every_future_power_operation_unavailable() {
        let mut state =
            DisplayPowerState::new(super::super::DisplayPowerControl::Available, 0, AUTO_OFF);
        state.mark_unavailable();
        assert_eq!(state, DisplayPowerState::Unavailable);
        assert_eq!(state.tick(u64::MAX), DisplayPowerCommand::NoChange);
        assert_eq!(
            state.button_pressed(1, AUTO_OFF),
            DisplayButtonOutcome::ForwardToUi
        );
        assert_eq!(state.toggle_auto_off(1, AUTO_OFF), None);
        assert_eq!(state.wake(1, AUTO_OFF), DisplayPowerCommand::NoChange);
    }
}
