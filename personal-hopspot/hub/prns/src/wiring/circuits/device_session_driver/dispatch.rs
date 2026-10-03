use super::*;
use hopspot_hub_core::Connection;

impl<Physical> DeviceSessionDriver<Physical> {
    pub(super) fn dispatch<const CAPACITY: usize>(
        &mut self,
        route: &DeviceSessionRoute<CAPACITY>,
    ) -> Result<(), DeviceDriverFailure> {
        for request in route.cancelled.iter().flatten() {
            if self.reactor.active.as_ref().is_some_and(|active| matches!(active.input, PrnsDeviceIn::Inventory { request: active } if active == *request)) {
                self.reactor.active = None;
            }
            for pending in &mut self.queued {
                if matches!(pending, Some(PrnsDeviceIn::Inventory { request: pending }) if pending == request)
                {
                    *pending = None;
                }
            }
        }
        for input in route.commands.iter().flatten() {
            if let PrnsDeviceIn::Close { connection } = input {
                if self
                    .reactor
                    .active
                    .as_ref()
                    .is_some_and(|active| cancels(&active.input, *connection))
                {
                    self.reactor.active = None;
                }
                for pending in &mut self.queued {
                    if pending
                        .as_ref()
                        .is_some_and(|pending| cancels(pending, *connection))
                    {
                        *pending = None;
                    }
                }
            }
            if self.queued.first().is_some_and(Option::is_none) {
                self.queued.rotate_left(1);
            }
            let slot = self
                .queued
                .iter_mut()
                .find(|slot| slot.is_none())
                .ok_or_else(|| DeviceDriverFailure::CommandCapacity {
                    rejected: duplicate(input),
                })?;
            *slot = Some(duplicate(input));
        }
        if self.reactor.active.is_none()
            && let Some(input) = self.queued.iter_mut().find_map(Option::take)
        {
            let completion = Box::pin(self.worker.execute(duplicate(&input)));
            self.reactor.active = Some(reactor::ActiveWork { input, completion });
        }
        self.reactor.admit_intents = self.queued.iter().all(Option::is_none);
        Ok(())
    }
}

fn cancels(input: &PrnsDeviceIn, closed: Connection) -> bool {
    match input {
        PrnsDeviceIn::Connect { connection } => *connection == closed,
        PrnsDeviceIn::Inventory { request } => request.connection() == closed,
        PrnsDeviceIn::Close { .. } => false,
    }
}

pub(super) fn duplicate(input: &PrnsDeviceIn) -> PrnsDeviceIn {
    match input {
        PrnsDeviceIn::Connect { connection } => PrnsDeviceIn::Connect {
            connection: *connection,
        },
        PrnsDeviceIn::Inventory { request } => PrnsDeviceIn::Inventory { request: *request },
        PrnsDeviceIn::Close { connection } => PrnsDeviceIn::Close {
            connection: *connection,
        },
    }
}
