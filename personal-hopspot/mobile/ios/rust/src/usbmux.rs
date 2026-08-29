use std::io;
use std::time::Duration;

use personal_rns::interfaces::usb_auto::{self as contract, Capabilities, Message, NodeTag};
use personal_rns::interfaces::{ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceKind};
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use personal_rns::manifold::tokio::TokioInterfaceStatus;
use personal_rns::tcp::tune;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const USBMUX_AUTO_PORT: u16 = 42_700;

const READ_CHUNK_BYTES: usize = contract::READ_CHUNK_BYTES;
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

pub struct UsbMuxAutoDevice {
    id: InterfaceId,
    node_tag: NodeTag,
    status: TokioInterfaceStatus,
    listener: TcpListener,
}

impl UsbMuxAutoDevice {
    pub fn with_listener(id: InterfaceId, listener: TcpListener) -> Self {
        Self {
            id,
            node_tag: contract::node_tag_for(id),
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Initializing),
            listener,
        }
    }

    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl Interface for UsbMuxAutoDevice {
    const HW_MTU: usize = personal_rns::interfaces::usb_auto::DEVICE_USB_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::UsbAutoDevice;

    fn descriptor(&self) -> InterfaceDescriptor {
        contract::device_descriptor(self.id)
    }

    fn channel_tag(&self) -> &[u8] {
        self.id.as_bytes()
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        println!(
            "usbmux-auto: listening on :{} for USB Auto over a usbmux/iProxy stream",
            USBMUX_AUTO_PORT
        );
        self.status.set_connection(ConnectionState::Disconnected);
        let listener = self.listener;

        loop {
            if !self.status.is_enabled() {
                self.status.set_connection(ConnectionState::Disabled);
                drain_disabled(&self.status, &mut seam).await;
                continue;
            }

            self.status.set_connection(ConnectionState::Disconnected);
            let accepted = tokio::select! {
                result = listener.accept() => result,
                () = self.status.wait_until_disabled() => continue,
                out = seam.next_outbound() => {
                    let _ = out;
                    continue;
                }
            };

            let (stream, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    eprintln!("usbmux-auto: accept failed: {error}");
                    self.status.set_connection(ConnectionState::Reconnecting);
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                        () = self.status.wait_until_disabled() => {}
                    }
                    continue;
                }
            };

            println!("usbmux-auto: accepted USB Auto stream from {peer}");
            tune(&stream);
            self.status.set_connection(ConnectionState::Reconnecting);
            serve_stream(stream, self.node_tag, &self.status, &mut seam).await;
            if self.status.is_enabled() {
                self.status.set_connection(ConnectionState::Disconnected);
            }
        }
    }
}

impl personal_rns::interfaces::ReportsStatus for UsbMuxAutoDevice {
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![personal_rns::interfaces::InterfaceVitals::of(&status)]
        }))
    }
}

async fn drain_disabled<Seam: InterfaceSeam>(status: &TokioInterfaceStatus, seam: &mut Seam) {
    tokio::select! {
        () = status.wait_until_enabled() => {}
        out = seam.next_outbound() => {
            let _ = out;
        }
    }
}

async fn serve_stream<Seam: InterfaceSeam>(
    mut stream: TcpStream,
    node_tag: NodeTag,
    status: &TokioInterfaceStatus,
    seam: &mut Seam,
) {
    let mut decoder = contract::Decoder::new();
    let mut read_buf = [0u8; READ_CHUNK_BYTES];
    let mut frame_buf = [0u8; contract::MAX_FRAMED_BYTES];
    let mut linked = false;

    loop {
        tokio::select! {
            read = stream.read(&mut read_buf) => {
                let n = match read {
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(error) => {
                        eprintln!("usbmux-auto: read failed: {error}");
                        return;
                    }
                };
                status.add_rx(n as u64);
                for &byte in &read_buf[..n] {
                    let Ok(Some(frame)) = decoder.feed(byte) else {
                        continue;
                    };
                    if frame.is_empty() {
                        continue;
                    }
                    match contract::decode_message(frame) {
                        Ok(Message::Hello(_)) => {
                            let ack = Message::HelloAck {
                                tag: node_tag,
                                capabilities: Capabilities::none(),
                            };
                            if write_message(&mut stream, &ack, &mut frame_buf, status).await.is_err() {
                                return;
                            }
                            if !linked {
                                linked = true;
                                status.set_connection(ConnectionState::Connected);
                            }
                        }
                        Ok(Message::Data(packet)) if !packet.is_empty() => {
                            seam.next_inbound(packet).await;
                        }
                        Ok(Message::Data(_)) | Ok(Message::HelloAck { .. }) | Err(_) => {}
                    }
                }
            }
            out = seam.next_outbound() => {
                if linked && status.is_enabled() {
                    let data = Message::Data(out);
                    if write_message(&mut stream, &data, &mut frame_buf, status).await.is_err() {
                        return;
                    }
                }
            }
            () = status.wait_until_disabled() => {
                status.set_connection(ConnectionState::Disabled);
                return;
            }
        }
    }
}

async fn write_message(
    stream: &mut TcpStream,
    message: &Message<'_>,
    frame_buf: &mut [u8; contract::MAX_FRAMED_BYTES],
    status: &TokioInterfaceStatus,
) -> io::Result<()> {
    let n = message
        .write_framed(frame_buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "USB Auto frame too large"))?;
    tokio::time::timeout(WRITE_TIMEOUT, stream.write_all(&frame_buf[..n]))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "USB Auto write timed out"))??;
    status.add_tx(n as u64);
    Ok(())
}
