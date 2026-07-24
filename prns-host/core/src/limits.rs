#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrnsLimits {
    pending_commands: usize,
    application_events: usize,
    retained_event_bytes: usize,
    diagnostics: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrnsLimitsError {
    PendingCommandsZero,
    ApplicationEventsZero,
    RetainedEventBytesZero,
    DiagnosticsZero,
}

impl PrnsLimits {
    pub const BALANCED_PENDING_COMMANDS: usize = 256;
    pub const BALANCED_APPLICATION_EVENTS: usize = 1_024;
    pub const BALANCED_RETAINED_EVENT_BYTES: usize = 8 * 1_024 * 1_024;
    pub const BALANCED_DIAGNOSTICS: usize = 1_024;

    #[must_use]
    pub const fn balanced() -> Self {
        Self {
            pending_commands: Self::BALANCED_PENDING_COMMANDS,
            application_events: Self::BALANCED_APPLICATION_EVENTS,
            retained_event_bytes: Self::BALANCED_RETAINED_EVENT_BYTES,
            diagnostics: Self::BALANCED_DIAGNOSTICS,
        }
    }

    pub const fn try_new(
        pending_commands: usize,
        application_events: usize,
        retained_event_bytes: usize,
        diagnostics: usize,
    ) -> Result<Self, PrnsLimitsError> {
        if pending_commands == 0 {
            return Err(PrnsLimitsError::PendingCommandsZero);
        }
        if application_events == 0 {
            return Err(PrnsLimitsError::ApplicationEventsZero);
        }
        if retained_event_bytes == 0 {
            return Err(PrnsLimitsError::RetainedEventBytesZero);
        }
        if diagnostics == 0 {
            return Err(PrnsLimitsError::DiagnosticsZero);
        }
        Ok(Self {
            pending_commands,
            application_events,
            retained_event_bytes,
            diagnostics,
        })
    }

    #[must_use]
    pub const fn pending_commands(self) -> usize {
        self.pending_commands
    }

    #[must_use]
    pub const fn application_events(self) -> usize {
        self.application_events
    }

    #[must_use]
    pub const fn retained_event_bytes(self) -> usize {
        self.retained_event_bytes
    }

    #[must_use]
    pub const fn diagnostics(self) -> usize {
        self.diagnostics
    }
}

impl Default for PrnsLimits {
    fn default() -> Self {
        Self::balanced()
    }
}
