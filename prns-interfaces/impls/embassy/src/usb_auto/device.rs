use core::mem::MaybeUninit;

use ::embassy_usb::driver::{
    Driver as UsbDriver, Endpoint as UsbEndpoint, EndpointError, EndpointIn, EndpointOut,
};
use ::embassy_usb::types::StringIndex;
use ::embassy_usb::{msos, Builder, Handler};

/// Full-speed USB bulk endpoint packet size.
pub const WEBUSB_AUTO_PACKET_SIZE: u16 = 64;

#[derive(Debug)]
pub struct WebUsbAutoError;

impl embedded_io_async::Error for WebUsbAutoError {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        embedded_io_async::ErrorKind::Other
    }
}

pub struct WebUsbAutoState {
    control: MaybeUninit<WebUsbAutoControl>,
}

impl WebUsbAutoState {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            control: MaybeUninit::uninit(),
        }
    }
}

impl Default for WebUsbAutoState {
    fn default() -> Self {
        Self::new()
    }
}

struct WebUsbAutoControl {
    iface_string: StringIndex,
}

impl Handler for WebUsbAutoControl {
    fn get_string(&mut self, index: StringIndex, _lang_id: u16) -> Option<&str> {
        (index == self.iface_string).then_some("Personal Hopspot WebUSB Auto")
    }
}

pub struct WebUsbAutoClass<'d, D: UsbDriver<'d>> {
    read_ep: D::EndpointOut,
    write_ep: D::EndpointIn,
}

impl<'d, D: UsbDriver<'d>> WebUsbAutoClass<'d, D> {
    #[must_use]
    pub fn new(
        builder: &mut Builder<'d, D>,
        state: &'d mut WebUsbAutoState,
        max_packet_size: u16,
    ) -> Self {
        let iface_string = builder.string();
        let mut function = builder.function(0xff, 0, 0);
        function.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        function.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            msos::PropertyData::RegMultiSz(&["{D6F980C1-0B65-4B3B-A029-01A93A3DEB44}"]),
        ));
        let mut interface = function.interface();
        let mut alt = interface.alt_setting(0xff, 0, 0, Some(iface_string));
        let read_ep = alt.endpoint_bulk_out(None, max_packet_size);
        let write_ep = alt.endpoint_bulk_in(None, max_packet_size);
        drop(function);

        builder.handler(state.control.write(WebUsbAutoControl { iface_string }));

        Self { read_ep, write_ep }
    }

    #[must_use]
    pub fn split(self) -> (WebUsbAutoTx<'d, D>, WebUsbAutoRx<'d, D>) {
        (
            WebUsbAutoTx {
                write_ep: self.write_ep,
            },
            WebUsbAutoRx {
                read_ep: self.read_ep,
            },
        )
    }
}

pub struct WebUsbAutoRx<'d, D: UsbDriver<'d>> {
    read_ep: D::EndpointOut,
}

impl<'d, D: UsbDriver<'d>> embedded_io_async::ErrorType for WebUsbAutoRx<'d, D> {
    type Error = WebUsbAutoError;
}

impl<'d, D: UsbDriver<'d>> embedded_io_async::Read for WebUsbAutoRx<'d, D> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        loop {
            match self.read_ep.read(buf).await {
                Ok(n) => return Ok(n),
                Err(EndpointError::Disabled) => self.read_ep.wait_enabled().await,
                Err(_) => return Err(WebUsbAutoError),
            }
        }
    }
}

pub struct WebUsbAutoTx<'d, D: UsbDriver<'d>> {
    write_ep: D::EndpointIn,
}

impl<'d, D: UsbDriver<'d>> embedded_io_async::ErrorType for WebUsbAutoTx<'d, D> {
    type Error = WebUsbAutoError;
}

impl<'d, D: UsbDriver<'d>> embedded_io_async::Write for WebUsbAutoTx<'d, D> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let len = core::cmp::min(buf.len(), self.write_ep.info().max_packet_size as usize);
        match self.write_ep.write(&buf[..len]).await {
            Ok(()) => Ok(len),
            Err(EndpointError::BufferOverflow) => unreachable!(),
            Err(_) => Err(WebUsbAutoError),
        }
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
