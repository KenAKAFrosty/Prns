use crate::MioSessionPoll;
use crate::{
    DeviceSessionInput, DeviceSessionRoute, DeviceSessionRoutingError, DeviceSessionSwitchboard,
    MioSessionReaction, MioSessionReactor,
};
use hopspot_hub_core::DeviceRegistry;
use pipecircuit::{Circuit, CircuitReactionOutcome, Reactor, Switchboard};

#[cfg(test)]
mod behavior;
#[cfg(test)]
mod tests;

pub struct MioDeviceSessionCircuit<Physical = MioSessionPoll> {
    registry: DeviceRegistry,
    reactor: MioSessionReactor<Physical>,
}

#[expect(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Eq)]
pub enum MioDeviceSessionTurn<const CAPACITY: usize> {
    Routed(DeviceSessionRoute<CAPACITY>),
    SendersClosed,
}

impl<Physical> MioDeviceSessionCircuit<Physical> {
    pub fn new(registry: DeviceRegistry, reactor: MioSessionReactor<Physical>) -> Self {
        Self { registry, reactor }
    }

    pub fn into_registry(self) -> DeviceRegistry {
        self.registry
    }
}

impl<const CAPACITY: usize, Physical: Reactor<Failure = std::io::Error>>
    Circuit<DeviceSessionSwitchboard<CAPACITY>> for MioDeviceSessionCircuit<Physical>
{
    type Reactor = MioSessionReactor<Physical>;
    type Completion = MioDeviceSessionTurn<CAPACITY>;
    type Failure = DeviceSessionRoutingError;

    fn reactor(&mut self) -> &mut Self::Reactor {
        &mut self.reactor
    }

    fn react(
        &mut self,
        switchboard: &mut DeviceSessionSwitchboard<CAPACITY>,
        reaction: &MioSessionReaction,
    ) -> Result<CircuitReactionOutcome<Self::Completion>, Self::Failure> {
        match reaction {
            MioSessionReaction::MessageReady => {
                let Some(message) = self.reactor.take_message() else {
                    return Ok(CircuitReactionOutcome::AwaitingReaction);
                };
                switchboard
                    .route(DeviceSessionInput {
                        registry: &mut self.registry,
                        message,
                    })
                    .map(|route| CircuitReactionOutcome::Completed {
                        completion: MioDeviceSessionTurn::Routed(route),
                    })
            }
            MioSessionReaction::SendersClosed => Ok(CircuitReactionOutcome::Completed {
                completion: MioDeviceSessionTurn::SendersClosed,
            }),
        }
    }
}
