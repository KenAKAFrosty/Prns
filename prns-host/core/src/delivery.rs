use alloc::vec::Vec;
use core::marker::PhantomData;

use crate::PrnsLimits;

pub trait EventDelivery {
    fn application_bytes(&self) -> Option<usize>;
    fn terminal(&self) -> bool;
    fn diagnostic_gap(dropped_diagnostics: u64) -> Self;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventDeliveryAdmission<Event> {
    Accepted(Vec<Event>),
    ApplicationRejected(Event),
    DroppedDiagnostic,
    Ignored,
}

pub struct EventDeliveryQueue<Event> {
    limits: PrnsLimits,
    application_events: usize,
    retained_event_bytes: usize,
    diagnostics: usize,
    dropped_diagnostics: u64,
    failed: bool,
    terminal_queued: bool,
    event: PhantomData<Event>,
}

impl<Event: EventDelivery> EventDeliveryQueue<Event> {
    #[must_use]
    pub const fn new(limits: PrnsLimits) -> Self {
        Self {
            limits,
            application_events: 0,
            retained_event_bytes: 0,
            diagnostics: 0,
            dropped_diagnostics: 0,
            failed: false,
            terminal_queued: false,
            event: PhantomData,
        }
    }

    pub fn admit(&mut self, event: Event) -> EventDeliveryAdmission<Event> {
        if self.failed && !event.terminal() {
            return EventDeliveryAdmission::Ignored;
        }
        if let Some(event_bytes) = event.application_bytes() {
            let byte_total = self.retained_event_bytes.checked_add(event_bytes);
            if self.application_events == self.limits.application_events()
                || byte_total.is_none_or(|total| total > self.limits.retained_event_bytes())
            {
                self.failed = true;
                return EventDeliveryAdmission::ApplicationRejected(event);
            }
            self.application_events += 1;
            self.retained_event_bytes = byte_total.unwrap_or(usize::MAX);
            return EventDeliveryAdmission::Accepted(alloc::vec![event]);
        }
        if event.terminal() {
            if self.terminal_queued {
                return EventDeliveryAdmission::Ignored;
            }
            self.terminal_queued = true;
            if self.dropped_diagnostics == 0 {
                return EventDeliveryAdmission::Accepted(alloc::vec![event]);
            }
            let dropped = core::mem::take(&mut self.dropped_diagnostics);
            self.diagnostics = self.diagnostics.saturating_add(1);
            return EventDeliveryAdmission::Accepted(alloc::vec![
                Event::diagnostic_gap(dropped),
                event,
            ]);
        }
        if self.diagnostics == self.limits.diagnostics() {
            self.dropped_diagnostics = self.dropped_diagnostics.saturating_add(1);
            return EventDeliveryAdmission::DroppedDiagnostic;
        }
        if self.dropped_diagnostics == 0 {
            self.diagnostics += 1;
            return EventDeliveryAdmission::Accepted(alloc::vec![event]);
        }
        let dropped = core::mem::take(&mut self.dropped_diagnostics);
        self.diagnostics += 1;
        if self.diagnostics < self.limits.diagnostics() {
            self.diagnostics += 1;
            return EventDeliveryAdmission::Accepted(alloc::vec![
                Event::diagnostic_gap(dropped),
                event,
            ]);
        }
        self.dropped_diagnostics = 1;
        EventDeliveryAdmission::Accepted(alloc::vec![Event::diagnostic_gap(dropped)])
    }

    pub fn complete(&mut self, event: &Event) {
        self.complete_parts(event.application_bytes(), event.terminal());
    }

    pub fn complete_parts(&mut self, application_bytes: Option<usize>, terminal: bool) {
        if let Some(event_bytes) = application_bytes {
            self.application_events = self.application_events.saturating_sub(1);
            self.retained_event_bytes = self.retained_event_bytes.saturating_sub(event_bytes);
        } else if !terminal {
            self.diagnostics = self.diagnostics.saturating_sub(1);
        }
    }

    #[must_use]
    pub const fn failed(&self) -> bool {
        self.failed
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::PrnsLimitsError;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Event {
        Application(usize),
        Diagnostic,
        Gap(u64),
        Terminal,
    }

    impl EventDelivery for Event {
        fn application_bytes(&self) -> Option<usize> {
            match self {
                Self::Application(bytes) => Some(*bytes),
                Self::Diagnostic | Self::Gap(_) | Self::Terminal => None,
            }
        }

        fn terminal(&self) -> bool {
            matches!(self, Self::Terminal)
        }

        fn diagnostic_gap(dropped_diagnostics: u64) -> Self {
            Self::Gap(dropped_diagnostics)
        }
    }

    fn limits(
        application_events: usize,
        retained_event_bytes: usize,
        diagnostics: usize,
    ) -> PrnsLimits {
        match PrnsLimits::try_new(1, application_events, retained_event_bytes, diagnostics) {
            Ok(limits) => limits,
            Err(error) => {
                assert_eq!(error, PrnsLimitsError::PendingCommandsZero);
                PrnsLimits::balanced()
            }
        }
    }

    #[test]
    fn application_pressure_is_terminal_and_explicit() {
        let mut queue = EventDeliveryQueue::new(limits(1, 1, 1));
        assert_eq!(
            queue.admit(Event::Application(2)),
            EventDeliveryAdmission::ApplicationRejected(Event::Application(2))
        );
        assert!(queue.failed());
        assert_eq!(
            queue.admit(Event::Terminal),
            EventDeliveryAdmission::Accepted(vec![Event::Terminal])
        );
    }

    #[test]
    fn diagnostics_drop_newest_and_flush_exact_gaps() {
        let mut queue = EventDeliveryQueue::new(limits(1, 1, 1));
        assert_eq!(
            queue.admit(Event::Diagnostic),
            EventDeliveryAdmission::Accepted(vec![Event::Diagnostic])
        );
        assert_eq!(
            queue.admit(Event::Diagnostic),
            EventDeliveryAdmission::DroppedDiagnostic
        );
        assert_eq!(
            queue.admit(Event::Diagnostic),
            EventDeliveryAdmission::DroppedDiagnostic
        );
        queue.complete(&Event::Diagnostic);
        assert_eq!(
            queue.admit(Event::Diagnostic),
            EventDeliveryAdmission::Accepted(vec![Event::Gap(2)])
        );
        queue.complete(&Event::Gap(2));
        assert_eq!(
            queue.admit(Event::Terminal),
            EventDeliveryAdmission::Accepted(vec![Event::Gap(1), Event::Terminal])
        );
    }
}
