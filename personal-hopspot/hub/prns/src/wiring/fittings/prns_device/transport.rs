use super::{PrnsDeviceFitting, PrnsDeviceWork, PrnsFittingError, PrnsInventoryTransport};
use crate::{PrnsDevice, PrnsDeviceIn, PrnsDeviceOut};
use pipecircuit::{DuplexFitting, ReceiveFromOutcome, TransportFrom, TransportTo};

pub struct PrnsDeviceIncoming;

pub struct PrnsDeviceOutgoing<'fitting, Backend: PrnsInventoryTransport> {
    fitting: &'fitting mut PrnsDeviceFitting<Backend>,
}

impl<Backend: PrnsInventoryTransport> DuplexFitting<PrnsDevice> for PrnsDeviceFitting<Backend> {
    type Incoming<'fitting>
        = PrnsDeviceIncoming
    where
        Self: 'fitting;
    type Outgoing<'fitting>
        = PrnsDeviceOutgoing<'fitting, Backend>
    where
        Self: 'fitting;

    fn split(&mut self) -> (Self::Incoming<'_>, Self::Outgoing<'_>) {
        (PrnsDeviceIncoming, PrnsDeviceOutgoing { fitting: self })
    }
}

impl<Backend: PrnsInventoryTransport> TransportTo<PrnsDevice> for PrnsDeviceOutgoing<'_, Backend> {
    type Outcome<'transport, 'message>
        = PrnsDeviceWork<'transport, Backend>
    where
        Self: 'transport;

    fn send<'transport, 'message>(
        &'transport mut self,
        input: PrnsDeviceIn,
    ) -> Self::Outcome<'transport, 'message> {
        PrnsDeviceWork {
            fitting: self.fitting,
            input,
        }
    }
}

impl TransportFrom<PrnsDevice> for PrnsDeviceIncoming {
    type IncomingMessage<'message> = Result<PrnsDeviceOut, PrnsFittingError>;
    type Failure = PrnsFittingError;

    fn receive_from(
        &mut self,
        completion: Self::IncomingMessage<'_>,
    ) -> ReceiveFromOutcome<PrnsDeviceOut, Self::IncomingMessage<'_>, Self::Failure> {
        match completion {
            Ok(output) => ReceiveFromOutcome::Received { output },
            Err(failure) => ReceiveFromOutcome::Failed { failure },
        }
    }
}
