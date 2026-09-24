use super::*;
use personal_rns::wifi_halow::{HaLowDatagrams, ReceivedDatagram};
use std::io;
use tokio::sync::{mpsc, watch, Mutex};

pub(super) struct Datagram {
    pub source: PeerMac,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Copy)]
pub(super) struct RadioControl {
    pub send: SendBehavior,
    pub receive: ReceiveBehavior,
}

pub struct VirtualHaLowRadio {
    medium: VirtualHaLowMedium,
    id: RadioId,
    inbound: Mutex<mpsc::Receiver<Datagram>>,
    control: watch::Receiver<RadioControl>,
}
impl VirtualHaLowRadio {
    pub(super) fn new(
        medium: VirtualHaLowMedium,
        id: RadioId,
        inbound: mpsc::Receiver<Datagram>,
        control: watch::Receiver<RadioControl>,
    ) -> Self {
        Self {
            medium,
            id,
            inbound: Mutex::new(inbound),
            control,
        }
    }
    pub fn id(&self) -> RadioId {
        self.id
    }
}
impl Drop for VirtualHaLowRadio {
    fn drop(&mut self) {
        self.medium.detach(self.id);
    }
}
impl HaLowDatagrams for VirtualHaLowRadio {
    async fn send(&self, destination: Destination, payload: &[u8]) -> io::Result<()> {
        let mut control = self.control.clone();
        loop {
            let behavior = control.borrow_and_update().send;
            match behavior {
                SendBehavior::Ready => return self.medium.transmit(self.id, destination, payload),
                SendBehavior::Failed => return Err(io::ErrorKind::BrokenPipe.into()),
                SendBehavior::Stalled => control
                    .changed()
                    .await
                    .map_err(|_| io::ErrorKind::BrokenPipe)?,
            }
        }
    }
    async fn receive(&self, buffer: &mut [u8]) -> io::Result<ReceivedDatagram> {
        let mut control = self.control.clone();
        let mut inbound = self.inbound.lock().await;
        loop {
            if control.borrow_and_update().receive == ReceiveBehavior::Failed {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            tokio::select! {
                changed = control.changed() => changed.map_err(|_| io::ErrorKind::BrokenPipe)?,
                received = inbound.recv() => {
                    let received = received.ok_or(io::ErrorKind::UnexpectedEof)?;
                    if received.bytes.len() > buffer.len() { return Err(io::ErrorKind::InvalidData.into()); }
                    buffer[..received.bytes.len()].copy_from_slice(&received.bytes);
                    return Ok(ReceivedDatagram { source: received.source, length: received.bytes.len() });
                }
            }
        }
    }
}
