use bt_hci::transport::Transport;
use bt_hci::{ControllerToHostPacket, HostToControllerPacket};
use embassy_futures::yield_now;
use embedded_io_07::ErrorType;

pub struct CooperativeTransport<T> {
    inner: T,
}

impl<T> CooperativeTransport<T> {
    #[must_use]
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

impl<T: ErrorType> ErrorType for CooperativeTransport<T> {
    type Error = T::Error;
}

impl<T: Transport> Transport for CooperativeTransport<T> {
    async fn read<'a>(&self, rx: &'a mut [u8]) -> Result<ControllerToHostPacket<'a>, Self::Error> {
        yield_now().await;
        self.inner.read(rx).await
    }

    async fn write<P: HostToControllerPacket>(&self, packet: &P) -> Result<(), Self::Error> {
        self.inner.write(packet).await
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};

    use embedded_io_07::ErrorKind;

    use super::*;

    struct ImmediateTransport<'a> {
        reads: &'a Cell<usize>,
    }

    impl ErrorType for ImmediateTransport<'_> {
        type Error = ErrorKind;
    }

    impl Transport for ImmediateTransport<'_> {
        async fn read<'a>(
            &self,
            _rx: &'a mut [u8],
        ) -> Result<ControllerToHostPacket<'a>, Self::Error> {
            self.reads.set(self.reads.get() + 1);
            Err(ErrorKind::Other)
        }

        async fn write<P: HostToControllerPacket>(&self, _packet: &P) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn an_immediately_ready_transport_yields_before_each_read() {
        let reads = Cell::new(0);
        let transport = CooperativeTransport::new(ImmediateTransport { reads: &reads });
        let mut rx = [0u8; 1];
        let mut read = pin!(transport.read(&mut rx));
        let mut context = Context::from_waker(Waker::noop());

        assert!(matches!(read.as_mut().poll(&mut context), Poll::Pending));
        assert_eq!(reads.get(), 0);
        assert!(matches!(
            read.as_mut().poll(&mut context),
            Poll::Ready(Err(ErrorKind::Other))
        ));
        assert_eq!(reads.get(), 1);
    }
}
