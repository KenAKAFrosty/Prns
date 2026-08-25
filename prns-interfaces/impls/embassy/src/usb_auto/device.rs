use core::mem::MaybeUninit;

use ::embassy_usb::driver::{
    Direction, Driver as UsbDriver, Endpoint as UsbEndpoint, EndpointError, EndpointIn, EndpointOut,
};
use ::embassy_usb::types::StringIndex;
use ::embassy_usb::{
    control::{OutResponse, Recipient, Request, RequestType},
    msos, Builder, Handler,
};
use prns_core::interfaces::usb_auto::{
    BOOTLOADER_ENTRY_CONTROL_INDEX, BOOTLOADER_ENTRY_CONTROL_REQUEST,
    BOOTLOADER_ENTRY_CONTROL_VALUE,
};

pub const WEBUSB_AUTO_PACKET_SIZE: u16 = 64;
pub const WEBUSB_AUTO_CONTROL_BUFFER_BYTES: usize = 128;
pub const WEBUSB_AUTO_MSOS_DESCRIPTOR_BYTES: usize = 192;

#[derive(Debug)]
pub enum WebUsbAutoError {
    Disconnected,
    PacketTooLarge,
}

impl embedded_io_async::Error for WebUsbAutoError {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        match self {
            Self::Disconnected => embedded_io_async::ErrorKind::NotConnected,
            Self::PacketTooLarge => embedded_io_async::ErrorKind::OutOfMemory,
        }
    }
}

pub struct WebUsbAutoState {
    control: MaybeUninit<WebUsbAutoControl>,
    bootloader_entry: WebUsbBootloaderEntry,
}

impl WebUsbAutoState {
    #[must_use]
    pub const fn new(bootloader_entry: WebUsbBootloaderEntry) -> Self {
        Self {
            control: MaybeUninit::uninit(),
            bootloader_entry,
        }
    }
}

#[derive(Clone, Copy)]
pub enum WebUsbBootloaderEntry {
    Unsupported,
    Supported { request: fn() },
}

struct WebUsbAutoControl {
    iface_string: StringIndex,
    bootloader_entry: WebUsbBootloaderEntry,
}

impl Handler for WebUsbAutoControl {
    fn get_string(&mut self, index: StringIndex, _lang_id: u16) -> Option<&str> {
        (index == self.iface_string).then_some("Personal Hopspot WebUSB Auto")
    }

    fn control_out(&mut self, request: Request, data: &[u8]) -> Option<OutResponse> {
        if !is_bootloader_entry_request(request, data) {
            return None;
        }
        match self.bootloader_entry {
            WebUsbBootloaderEntry::Unsupported => Some(OutResponse::Rejected),
            WebUsbBootloaderEntry::Supported { request } => {
                request();
                Some(OutResponse::Accepted)
            }
        }
    }
}

fn is_bootloader_entry_request(request: Request, data: &[u8]) -> bool {
    request.direction == Direction::Out
        && request.request_type == RequestType::Vendor
        && request.recipient == Recipient::Device
        && request.request == BOOTLOADER_ENTRY_CONTROL_REQUEST
        && request.value == BOOTLOADER_ENTRY_CONTROL_VALUE
        && request.index == BOOTLOADER_ENTRY_CONTROL_INDEX
        && request.length == 0
        && data.is_empty()
}

pub struct WebUsbAutoClass<'d, D: UsbDriver<'d>> {
    read_ep: D::EndpointOut,
    write_ep: D::EndpointIn,
}

impl<'d, D: UsbDriver<'d>> WebUsbAutoClass<'d, D> {
    /// Tells Windows to use WinUSB for this device. USB Auto must be its only USB function.
    #[must_use]
    pub fn new(
        builder: &mut Builder<'d, D>,
        state: &'d mut WebUsbAutoState,
        max_packet_size: u16,
    ) -> Self {
        let iface_string = builder.string();
        builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            msos::PropertyData::RegMultiSz(&["{D6F980C1-0B65-4B3B-A029-01A93A3DEB44}"]),
        ));
        let mut function = builder.function(0xff, 0, 0);
        let mut interface = function.interface();
        let mut alt = interface.alt_setting(0xff, 0, 0, Some(iface_string));
        let read_ep = alt.endpoint_bulk_out(None, max_packet_size);
        let write_ep = alt.endpoint_bulk_in(None, max_packet_size);
        drop(function);

        builder.handler(state.control.write(WebUsbAutoControl {
            iface_string,
            bootloader_entry: state.bootloader_entry,
        }));

        Self { read_ep, write_ep }
    }

    #[must_use]
    pub fn split(self) -> (WebUsbAutoTx<'d, D>, WebUsbAutoRx<'d, D>) {
        (
            WebUsbAutoTx {
                write_ep: self.write_ep,
                needs_zlp: false,
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
            if let Some(n) = endpoint_read(self.read_ep.read(buf).await)? {
                return Ok(n);
            }
        }
    }
}

fn endpoint_read(result: Result<usize, EndpointError>) -> Result<Option<usize>, WebUsbAutoError> {
    match result {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(n)),
        Err(EndpointError::Disabled) => Err(WebUsbAutoError::Disconnected),
        Err(EndpointError::BufferOverflow) => Err(WebUsbAutoError::PacketTooLarge),
    }
}

pub struct WebUsbAutoTx<'d, D: UsbDriver<'d>> {
    write_ep: D::EndpointIn,
    needs_zlp: bool,
}

impl<'d, D: UsbDriver<'d>> embedded_io_async::ErrorType for WebUsbAutoTx<'d, D> {
    type Error = WebUsbAutoError;
}

impl<'d, D: UsbDriver<'d>> embedded_io_async::Write for WebUsbAutoTx<'d, D> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = core::cmp::min(buf.len(), self.write_ep.info().max_packet_size as usize);
        match self.write_ep.write(&buf[..len]).await {
            Ok(()) => {
                self.needs_zlp = len == self.write_ep.info().max_packet_size as usize;
                Ok(len)
            }
            Err(EndpointError::Disabled) => Err(WebUsbAutoError::Disconnected),
            Err(EndpointError::BufferOverflow) => Err(WebUsbAutoError::PacketTooLarge),
        }
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        if !self.needs_zlp {
            return Ok(());
        }
        match self.write_ep.write(&[]).await {
            Ok(()) => {
                self.needs_zlp = false;
                Ok(())
            }
            Err(EndpointError::Disabled) => Err(WebUsbAutoError::Disconnected),
            Err(EndpointError::BufferOverflow) => Err(WebUsbAutoError::PacketTooLarge),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bootloader_entry_request() -> Request {
        Request {
            direction: Direction::Out,
            request_type: RequestType::Vendor,
            recipient: Recipient::Device,
            request: BOOTLOADER_ENTRY_CONTROL_REQUEST,
            value: BOOTLOADER_ENTRY_CONTROL_VALUE,
            index: BOOTLOADER_ENTRY_CONTROL_INDEX,
            length: 0,
        }
    }

    #[test]
    fn bootloader_entry_requires_the_exact_control_contract() {
        let request = bootloader_entry_request();
        assert!(is_bootloader_entry_request(request, &[]));

        let mut wrong_value = request;
        wrong_value.value ^= 1;
        assert!(!is_bootloader_entry_request(wrong_value, &[]));
        assert!(!is_bootloader_entry_request(request, &[0]));
    }

    #[test]
    fn zero_length_usb_packets_are_transport_idle_not_stream_eof() {
        assert!(matches!(endpoint_read(Ok(0)), Ok(None)));
        assert!(matches!(endpoint_read(Ok(17)), Ok(Some(17))));
    }

    #[test]
    fn endpoint_failures_preserve_disconnect_and_capacity_meaning() {
        assert!(matches!(
            endpoint_read(Err(EndpointError::Disabled)),
            Err(WebUsbAutoError::Disconnected)
        ));
        assert!(matches!(
            endpoint_read(Err(EndpointError::BufferOverflow)),
            Err(WebUsbAutoError::PacketTooLarge)
        ));
    }
}
