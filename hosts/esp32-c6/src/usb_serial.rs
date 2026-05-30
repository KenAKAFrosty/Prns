//! ESP32-C6 USB Serial/JTAG transport as a Reticulum point-to-point
//! interface.
//!
//! The C6 has a built-in USB Serial/JTAG controller that presents to a
//! laptop as a CDC-ACM device (`/dev/ttyACM*` on Linux). Outgoing
//! Reticulum packets are wrapped in canonical RNS serial framing and
//! pushed to the peripheral; incoming bytes are fed to a
//! streaming [`RnsSerialDecoder`] that surfaces one complete Reticulum
//! packet per `try_read` call.
//!
//! This file ships the [`Interface`] impl wired against the real
//! `esp_hal::usb_serial_jtag::UsbSerialJtag` peripheral so the C6 main
//! loop can register it with the engine and speak the same serial wire
//! format as a stock RNS `SerialInterface`.

use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_hal::Blocking;

use personal_rns::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use personal_rns::interfaces::{
    Capabilities, Interface, InterfaceId, InterfaceMode, InterfaceState, MediumKind,
    PointToPointInterface,
};
use personal_rns::wire::MTU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Esp32UsbSerialError {
    FrameLargerThanCallerBuffer,
    FrameLargerThanInterfaceBuffer,
    PayloadLargerThanMtu,
}

/// Size the encode scratch at the RNS serial framing worst case (every
/// payload byte escaped) for an MTU-sized packet. Const-fn keeps this a
/// stack constant rather than a runtime computation.
const ENCODE_BUF_LEN: usize = rns_serial_framing::max_encoded_len(MTU);

pub struct Esp32UsbSerialInterface<'d> {
    id: InterfaceId,
    usb: UsbSerialJtag<'d, Blocking>,
    decoder: RnsSerialDecoder<MTU>,
}

impl<'d> Esp32UsbSerialInterface<'d> {
    pub fn new(id: InterfaceId, usb: UsbSerialJtag<'d, Blocking>) -> Self {
        Self {
            id,
            usb,
            decoder: RnsSerialDecoder::new(),
        }
    }
}

impl Interface for Esp32UsbSerialInterface<'_> {
    type Error = Esp32UsbSerialError;

    fn id(&self) -> InterfaceId {
        self.id
    }

    fn capabilities(&self) -> Capabilities {
        // USB CDC to a single host: full-duplex byte stream with no
        // broadcast or in-medium repeat semantics.
        Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            repeats: false,
        }
    }

    fn mode(&self) -> InterfaceMode {
        InterfaceMode::PointToPoint
    }

    fn medium_kind(&self) -> MediumKind {
        MediumKind::DirectPeer
    }

    fn state(&self) -> InterfaceState {
        // Plug/unplug lifecycle detection has not landed; while the USB
        // peripheral is owned, report Connected.
        InterfaceState::Connected
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        // Drain bytes one at a time from the USB RX FIFO and feed each
        // to the RNS serial decoder. As soon as a frame closes, copy the
        // decoded payload into the caller's buf and return.
        //
        // The decoder's state survives across `try_read` calls, so a
        // partial frame at the end of one call resumes cleanly at the
        // start of the next.
        //
        // `read_byte` returns `Err(WouldBlock)` once the FIFO is dry;
        // the `let Ok(...) else` pattern surfaces that as `Ok(None)`
        // to the engine. The peripheral's Error type is `Infallible`,
        // so no other Err variant can be observed here.
        loop {
            let Ok(byte) = self.usb.read_byte() else {
                return Ok(None);
            };

            match self.decoder.feed(byte) {
                Ok(None) => continue,
                Ok(Some(frame)) => {
                    if frame.is_empty() {
                        continue;
                    }
                    if frame.len() > buf.len() {
                        return Err(Esp32UsbSerialError::FrameLargerThanCallerBuffer);
                    }
                    let n = frame.len();
                    buf[..n].copy_from_slice(frame);
                    return Ok(Some(n));
                }
                Err(rns_serial_framing::DecodeError::FrameTooBig) => {
                    return Err(Esp32UsbSerialError::FrameLargerThanInterfaceBuffer);
                }
            }
        }
    }

    fn write(&mut self, packet: &[u8]) -> Result<(), Self::Error> {
        let mut framed = [0u8; ENCODE_BUF_LEN];
        let n = rns_serial_framing::encode(packet, &mut framed)
            .map_err(|_| Esp32UsbSerialError::PayloadLargerThanMtu)?;
        // The peripheral's Error type is `Infallible`, so this write
        // can't fail at the Rust level; we still discard the Result
        // explicitly so the call shape stays self-documenting.
        let _ = self.usb.write(&framed[..n]);
        Ok(())
    }
}

impl PointToPointInterface for Esp32UsbSerialInterface<'_> {}
