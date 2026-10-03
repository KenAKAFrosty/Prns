use super::NativeUsbDiscoveryEvent;
use crate::{
    UsbDiscoveryInput, UsbDiscoveryIntent, UsbDiscoveryMessage, UsbDiscoveryRoutingError,
    UsbDiscoverySwitchboard, UsbDiscoveryUpdate,
};
use alloc::sync::Arc;
use personal_rns::interfaces::InterfaceId;
use personal_rns::manifold::tokio::TokioClock;
use personal_rns::runtime::{Message, PrnsEvent};
use pipecircuit::Switchboard;
use std::sync::Mutex;

#[derive(Debug, PartialEq, Eq)]
pub enum UsbDiscoveryFailure {
    Poisoned,
    Routing(UsbDiscoveryRoutingError),
}

struct Shared<const CAPACITY: usize> {
    clock: TokioClock,
    switchboard: Mutex<UsbDiscoverySwitchboard<CAPACITY>>,
}

#[derive(Clone)]
pub struct UsbDiscoveryHandle<const CAPACITY: usize> {
    shared: Arc<Shared<CAPACITY>>,
}

impl<const CAPACITY: usize> UsbDiscoveryHandle<CAPACITY> {
    pub(super) fn new(interface: InterfaceId, clock: TokioClock) -> Self {
        Self {
            shared: Arc::new(Shared {
                switchboard: Mutex::new(UsbDiscoverySwitchboard::new(interface, clock.now())),
                clock,
            }),
        }
    }

    pub fn submit(
        &self,
        intent: UsbDiscoveryIntent,
    ) -> Result<UsbDiscoveryUpdate<CAPACITY>, UsbDiscoveryFailure> {
        self.route(UsbDiscoveryMessage::Intent(intent))
    }

    fn route(
        &self,
        message: UsbDiscoveryMessage<'_>,
    ) -> Result<UsbDiscoveryUpdate<CAPACITY>, UsbDiscoveryFailure> {
        let mut switchboard = self
            .shared
            .switchboard
            .lock()
            .map_err(|_| UsbDiscoveryFailure::Poisoned)?;
        switchboard
            .route(UsbDiscoveryInput {
                now: self.shared.clock.now(),
                message,
            })
            .map_err(UsbDiscoveryFailure::Routing)
    }

    pub(super) fn receive<'a>(
        &self,
        event: PrnsEvent<'a>,
    ) -> NativeUsbDiscoveryEvent<'a, CAPACITY> {
        match event {
            PrnsEvent::Message(Message::RemoteControlPairingAvailable(observation)) => {
                NativeUsbDiscoveryEvent::Discovery(
                    self.route(UsbDiscoveryMessage::Available(observation)),
                )
            }
            event @ PrnsEvent::Message(_) | event @ PrnsEvent::Diagnostic(_) => {
                NativeUsbDiscoveryEvent::Prns(event)
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/handle.rs"]
mod tests;
