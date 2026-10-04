use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use hopspot_hub_core::ObserveEnrollmentResult;
use personal_rns::runtime::{Message, PrnsEvent, RemoteControlControllerPairingConfirmation};
use tokio::sync::mpsc::{Receiver, Sender, channel};

pub enum EnrollmentEvent {
    Confirmation(RemoteControlControllerPairingConfirmation),
    Settlement(ObserveEnrollmentResult),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnrollmentEventsFailure {
    Overflow,
    NodeStopped,
}

pub struct EnrollmentEvents {
    events: Receiver<EnrollmentEvent>,
    overflow: Arc<AtomicBool>,
}

pub(super) struct EnrollmentEventSink {
    events: Sender<EnrollmentEvent>,
    overflow: Arc<AtomicBool>,
}

pub(super) fn enrollment_events() -> (EnrollmentEventSink, EnrollmentEvents) {
    let (sender, events) = channel(8);
    let overflow = Arc::new(AtomicBool::new(false));
    (
        EnrollmentEventSink {
            events: sender,
            overflow: overflow.clone(),
        },
        EnrollmentEvents { events, overflow },
    )
}

impl EnrollmentEvents {
    pub async fn receive(&mut self) -> Result<EnrollmentEvent, EnrollmentEventsFailure> {
        let event = self.events.recv().await;
        if self.overflow.load(Ordering::Acquire) {
            return Err(EnrollmentEventsFailure::Overflow);
        }
        event.ok_or(EnrollmentEventsFailure::NodeStopped)
    }
}

impl EnrollmentEventSink {
    pub(super) fn capture<'a>(&self, event: PrnsEvent<'a>) -> Option<PrnsEvent<'a>> {
        let captured = match event {
            PrnsEvent::Message(Message::RemoteControlControllerPairingConfirmationRequired(
                confirmation,
            )) => EnrollmentEvent::Confirmation(confirmation),
            PrnsEvent::Message(ref message) => {
                match super::super::enrollment_coordinator::protocol_result(message) {
                    Some(result) => EnrollmentEvent::Settlement(result),
                    None => return Some(event),
                }
            }
            PrnsEvent::Diagnostic(_) => return Some(event),
        };
        if self.events.try_send(captured).is_err() {
            self.overflow.store(true, Ordering::Release);
        }
        None
    }
}
