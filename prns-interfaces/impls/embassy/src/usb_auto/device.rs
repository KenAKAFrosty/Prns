use ::embassy_usb::driver::{
    Direction, Driver as UsbDriver, Endpoint as UsbEndpoint, EndpointError, EndpointIn, EndpointOut,
};
use ::embassy_usb::types::StringIndex;
use ::embassy_usb::{
    control::{InResponse, OutResponse, Recipient, Request, RequestType},
    msos, Builder, Handler,
};
use prns_core::interfaces::usb_auto::{
    UsbControllerEnrollment, UsbControllerEnrollmentBusy, UsbControllerEnrollmentStatus,
    BOOTLOADER_ENTRY_CONTROL_INDEX, BOOTLOADER_ENTRY_CONTROL_REQUEST,
    BOOTLOADER_ENTRY_CONTROL_VALUE, CONTROLLER_ENROLL_CONTROL_REQUEST,
    CONTROLLER_ENROLL_REQUEST_BYTES, CONTROLLER_ENROLL_STATUS_BYTES,
    CONTROLLER_ENROLL_STATUS_REQUEST, UF2_HAND_OFF_CONTROL_REQUEST,
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
    control: WebUsbAutoControl,
}

impl WebUsbAutoState {
    #[must_use]
    pub const fn new(bootloader_entry: WebUsbBootloaderEntry) -> Self {
        Self {
            control: WebUsbAutoControl {
                iface_string: None,
                bootloader_entry,
                controller_enrollment: WebUsbControllerEnrollment::Unsupported,
            },
        }
    }

    #[must_use]
    pub const fn with_controller_enrollment(
        mut self,
        enrollment: WebUsbControllerEnrollment,
    ) -> Self {
        self.control.controller_enrollment = enrollment;
        self
    }
}

/// A local USB host may explicitly grant controller access. This is never exposed
/// through radio packets; completion is reported only by the durable grant owner.
#[derive(Clone, Copy)]
pub enum WebUsbControllerEnrollment {
    Unsupported,
    Supported {
        request: fn(UsbControllerEnrollment) -> Result<(), UsbControllerEnrollmentBusy>,
        status: fn() -> UsbControllerEnrollmentStatus,
        target_public_key: [u8; prns_core::identity::IDENTITY_PUBLIC_KEY_LEN],
    },
}

/// The bootloader transport requested by a validated USB control transfer.
#[derive(Debug, PartialEq, Eq)]
pub enum WebUsbBootloaderMode {
    /// Enters the transport used by the Hopspot updater.
    PrnsFlasher,
    /// Makes the stock UF2 drive available for switching firmware.
    Uf2HandOff,
}

#[derive(Clone, Copy)]
pub enum WebUsbBootloaderEntry {
    Unsupported,
    Supported { request: fn(WebUsbBootloaderMode) },
}

struct WebUsbAutoControl {
    iface_string: Option<StringIndex>,
    bootloader_entry: WebUsbBootloaderEntry,
    controller_enrollment: WebUsbControllerEnrollment,
}

impl Handler for WebUsbAutoControl {
    fn get_string(&mut self, index: StringIndex, _lang_id: u16) -> Option<&str> {
        (Some(index) == self.iface_string).then_some("Personal Hopspot WebUSB Auto")
    }

    fn control_in<'a>(&'a mut self, request: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        if !enrollment_request(
            request,
            Direction::In,
            CONTROLLER_ENROLL_STATUS_REQUEST,
            CONTROLLER_ENROLL_STATUS_BYTES,
        ) {
            return None;
        }
        match self.controller_enrollment {
            WebUsbControllerEnrollment::Unsupported => Some(InResponse::Rejected),
            WebUsbControllerEnrollment::Supported {
                status,
                target_public_key,
                ..
            } => {
                let Some(response) = buf.get_mut(..CONTROLLER_ENROLL_STATUS_BYTES) else {
                    return Some(InResponse::Rejected);
                };
                response.copy_from_slice(&status().encode(&target_public_key));
                Some(InResponse::Accepted(response))
            }
        }
    }

    fn control_out(&mut self, request: Request, data: &[u8]) -> Option<OutResponse> {
        if enrollment_request(
            request,
            Direction::Out,
            CONTROLLER_ENROLL_CONTROL_REQUEST,
            CONTROLLER_ENROLL_REQUEST_BYTES,
        ) {
            let Some(enrollment) = UsbControllerEnrollment::decode(data) else {
                return Some(OutResponse::Rejected);
            };
            return Some(match self.controller_enrollment {
                WebUsbControllerEnrollment::Unsupported => OutResponse::Rejected,
                WebUsbControllerEnrollment::Supported { request, .. } => {
                    match request(enrollment) {
                        Ok(()) => OutResponse::Accepted,
                        Err(UsbControllerEnrollmentBusy) => OutResponse::Rejected,
                    }
                }
            });
        }
        let mode = bootloader_mode(request, data)?;
        match self.bootloader_entry {
            WebUsbBootloaderEntry::Unsupported => Some(OutResponse::Rejected),
            WebUsbBootloaderEntry::Supported { request } => {
                request(mode);
                Some(OutResponse::Accepted)
            }
        }
    }
}

fn enrollment_request(request: Request, direction: Direction, verb: u8, length: usize) -> bool {
    request.direction == direction
        && request.request_type == RequestType::Vendor
        && request.recipient == Recipient::Device
        && request.request == verb
        && request.value == BOOTLOADER_ENTRY_CONTROL_VALUE
        && request.index == BOOTLOADER_ENTRY_CONTROL_INDEX
        && usize::from(request.length) == length
}

fn bootloader_mode(request: Request, data: &[u8]) -> Option<WebUsbBootloaderMode> {
    let carries_prns_signature = request.direction == Direction::Out
        && request.request_type == RequestType::Vendor
        && request.recipient == Recipient::Device
        && request.value == BOOTLOADER_ENTRY_CONTROL_VALUE
        && request.index == BOOTLOADER_ENTRY_CONTROL_INDEX
        && request.length == 0
        && data.is_empty();
    if !carries_prns_signature {
        return None;
    }
    match request.request {
        BOOTLOADER_ENTRY_CONTROL_REQUEST => Some(WebUsbBootloaderMode::PrnsFlasher),
        UF2_HAND_OFF_CONTROL_REQUEST => Some(WebUsbBootloaderMode::Uf2HandOff),
        _ => None,
    }
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

        state.control.iface_string = Some(iface_string);
        builder.handler(&mut state.control);

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
    fn enrollment_requires_exact_vendor_signature_direction_and_lengths() {
        for (verb, direction, length) in [
            (
                CONTROLLER_ENROLL_CONTROL_REQUEST,
                Direction::Out,
                CONTROLLER_ENROLL_REQUEST_BYTES,
            ),
            (
                CONTROLLER_ENROLL_STATUS_REQUEST,
                Direction::In,
                CONTROLLER_ENROLL_STATUS_BYTES,
            ),
        ] {
            let mut request = bootloader_entry_request();
            request.request = verb;
            request.direction = direction;
            request.length = length as u16;
            assert!(enrollment_request(request, direction, verb, length));
            let mut invalid = request;
            invalid.value ^= 1;
            assert!(!enrollment_request(invalid, direction, verb, length));
            invalid = request;
            invalid.index ^= 1;
            assert!(!enrollment_request(invalid, direction, verb, length));
            invalid = request;
            invalid.request_type = RequestType::Class;
            assert!(!enrollment_request(invalid, direction, verb, length));
            invalid = request;
            invalid.recipient = Recipient::Interface;
            assert!(!enrollment_request(invalid, direction, verb, length));
            invalid = request;
            invalid.length += 1;
            assert!(!enrollment_request(invalid, direction, verb, length));
            invalid = request;
            invalid.request ^= 1;
            assert!(!enrollment_request(invalid, direction, verb, length));
            invalid = request;
            invalid.direction = match direction {
                Direction::In => Direction::Out,
                Direction::Out => Direction::In,
            };
            assert!(!enrollment_request(invalid, direction, verb, length));
        }
    }

    #[test]
    fn enrollment_rejects_unsupported_busy_and_truncated_requests() {
        let mut handler = WebUsbAutoControl {
            iface_string: Some(StringIndex(1)),
            bootloader_entry: WebUsbBootloaderEntry::Unsupported,
            controller_enrollment: WebUsbControllerEnrollment::Unsupported,
        };
        let mut request = bootloader_entry_request();
        request.request = CONTROLLER_ENROLL_CONTROL_REQUEST;
        request.length = CONTROLLER_ENROLL_REQUEST_BYTES as u16;
        let bytes = [42; CONTROLLER_ENROLL_REQUEST_BYTES];
        assert!(matches!(
            handler.control_out(request, &bytes),
            Some(OutResponse::Rejected)
        ));
        handler.controller_enrollment = WebUsbControllerEnrollment::Supported {
            request: |_| Err(UsbControllerEnrollmentBusy),
            status: || UsbControllerEnrollmentStatus::Pending { transaction: 42 },
            target_public_key: [42; 64],
        };
        assert!(matches!(
            handler.control_out(request, &bytes),
            Some(OutResponse::Rejected)
        ));
        handler.controller_enrollment = WebUsbControllerEnrollment::Supported {
            request: |_| Ok(()),
            status: || UsbControllerEnrollmentStatus::Saved { transaction: 42 },
            target_public_key: [42; 64],
        };
        assert!(matches!(
            handler.control_out(request, &bytes[..67]),
            Some(OutResponse::Rejected)
        ));
        assert!(matches!(
            handler.control_out(request, &bytes),
            Some(OutResponse::Accepted)
        ));
        request.request = CONTROLLER_ENROLL_STATUS_REQUEST;
        request.direction = Direction::In;
        request.length = CONTROLLER_ENROLL_STATUS_BYTES as u16;
        let mut response = [0; CONTROLLER_ENROLL_STATUS_BYTES];
        assert!(matches!(
            handler.control_in(request, &mut response),
            Some(InResponse::Accepted(bytes)) if bytes[..5] == [2, 42, 0, 0, 0] && bytes[5..] == [42; 64]
        ));
    }

    #[test]
    fn bootloader_entry_requires_the_exact_control_contract() {
        let request = bootloader_entry_request();
        assert_eq!(
            bootloader_mode(request, &[]),
            Some(WebUsbBootloaderMode::PrnsFlasher)
        );

        let mut wrong_value = request;
        wrong_value.value ^= 1;
        assert_eq!(bootloader_mode(wrong_value, &[]), None);
        assert_eq!(bootloader_mode(request, &[0]), None);

        let mut unknown_verb = request;
        unknown_verb.request = 0x51;
        assert_eq!(bootloader_mode(unknown_verb, &[]), None);
    }

    #[test]
    fn uf2_hand_off_shares_the_signature_and_differs_only_in_verb() {
        let mut request = bootloader_entry_request();
        request.request = UF2_HAND_OFF_CONTROL_REQUEST;
        assert_eq!(
            bootloader_mode(request, &[]),
            Some(WebUsbBootloaderMode::Uf2HandOff)
        );

        let mut wrong_index = request;
        wrong_index.index ^= 1;
        assert_eq!(bootloader_mode(wrong_index, &[]), None);
    }

    #[test]
    fn neither_reset_mode_accepts_non_vendor_or_nonempty_control_transfers() {
        for verb in [
            BOOTLOADER_ENTRY_CONTROL_REQUEST,
            UF2_HAND_OFF_CONTROL_REQUEST,
        ] {
            let mut request = bootloader_entry_request();
            request.request = verb;
            let mut invalid = request;
            invalid.direction = Direction::In;
            assert_eq!(bootloader_mode(invalid, &[]), None);
            invalid = request;
            invalid.request_type = RequestType::Standard;
            assert_eq!(bootloader_mode(invalid, &[]), None);
            invalid = request;
            invalid.recipient = Recipient::Interface;
            assert_eq!(bootloader_mode(invalid, &[]), None);
            invalid = request;
            invalid.length = 1;
            assert_eq!(bootloader_mode(invalid, &[]), None);
            assert_eq!(bootloader_mode(request, &[0]), None);
        }
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
