use embassy_futures::select::{select3, Either3};
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, IpEndpoint, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel::{Channel, Receiver, Sender};
use embassy_time::{with_timeout, Duration, Timer};

use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::{InterfaceId, InterfaceKind};

pub const TCP_RENDEZVOUS_FRAME_CAP: usize = contract::HARDWARE_MTU;
pub const TCP_RENDEZVOUS_FRAMED_LEN: usize =
    rns_serial_framing::max_encoded_len(TCP_RENDEZVOUS_FRAME_CAP);
pub const TCP_RENDEZVOUS_READ_BUFFER_BYTES: usize = 1_024;
pub const TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES: usize = 1_024;

const SOCKET_TIMEOUT: Duration = Duration::from_secs(24);
const KEEP_ALIVE: Duration = Duration::from_secs(5);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, PartialEq, Eq)]
enum WireKind {
    Empty,
    Connected,
    Frame,
    Disconnected,
}

pub struct TcpRendezvousWireSlot {
    kind: WireKind,
    session: u32,
    id: InterfaceId,
    bytes: [u8; TCP_RENDEZVOUS_FRAME_CAP],
    len: usize,
}

impl TcpRendezvousWireSlot {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            kind: WireKind::Empty,
            session: 0,
            id: InterfaceId::new([0u8; 8]),
            bytes: [0u8; TCP_RENDEZVOUS_FRAME_CAP],
            len: 0,
        }
    }

    fn set_control(&mut self, kind: WireKind, session: u32, id: InterfaceId) {
        self.kind = kind;
        self.session = session;
        self.id = id;
        self.len = 0;
    }

    fn set_frame(
        &mut self,
        session: u32,
        id: InterfaceId,
        bytes: &[u8],
    ) -> Result<(), TcpRendezvousSendError> {
        if bytes.len() > TCP_RENDEZVOUS_FRAME_CAP {
            return Err(TcpRendezvousSendError::FrameTooLarge {
                len: bytes.len(),
                capacity: TCP_RENDEZVOUS_FRAME_CAP,
            });
        }
        self.kind = WireKind::Frame;
        self.session = session;
        self.id = id;
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        self.len = bytes.len();
        Ok(())
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn frame_for_session(&self, session: u32) -> Option<&[u8]> {
        (self.kind == WireKind::Frame && self.session == session).then(|| self.bytes())
    }

    pub(super) fn event(&self) -> Option<TcpRendezvousEvent<'_>> {
        match self.kind {
            WireKind::Connected => Some(TcpRendezvousEvent::Connected {
                session: self.session,
                id: self.id,
            }),
            WireKind::Frame => Some(TcpRendezvousEvent::Frame {
                session: self.session,
                id: self.id,
                bytes: self.bytes(),
            }),
            WireKind::Disconnected => Some(TcpRendezvousEvent::Disconnected {
                session: self.session,
                id: self.id,
            }),
            WireKind::Empty => None,
        }
    }
}

pub(super) enum TcpRendezvousEvent<'a> {
    Connected {
        session: u32,
        id: InterfaceId,
    },
    Frame {
        session: u32,
        id: InterfaceId,
        bytes: &'a [u8],
    },
    Disconnected {
        session: u32,
        id: InterfaceId,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum TcpRendezvousSendError {
    FrameTooLarge { len: usize, capacity: usize },
}

pub struct TcpRendezvousStorage<'a> {
    events: Channel<'a, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    commands: Channel<'a, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    disconnect: Signal<CriticalSectionRawMutex, u32>,
}

impl<'a> TcpRendezvousStorage<'a> {
    #[must_use]
    pub fn new(
        event_slots: &'a mut [TcpRendezvousWireSlot; 1],
        command_slots: &'a mut [TcpRendezvousWireSlot; 1],
    ) -> Self {
        Self {
            events: Channel::new(event_slots),
            commands: Channel::new(command_slots),
            disconnect: Signal::new(),
        }
    }
}

pub struct TcpRendezvousBuffers<'a> {
    pub rx: &'a mut [u8; TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES],
    pub tx: &'a mut [u8; TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES],
    pub read: &'a mut [u8; TCP_RENDEZVOUS_READ_BUFFER_BYTES],
    pub framed: &'a mut [u8; TCP_RENDEZVOUS_FRAMED_LEN],
    pub decoder: &'a mut RnsSerialDecoder<TCP_RENDEZVOUS_FRAME_CAP>,
}

pub struct TcpRendezvousServer<'a> {
    stack: Stack<'a>,
    buffers: TcpRendezvousBuffers<'a>,
    events: Sender<'a, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    commands: Receiver<'a, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    disconnect: &'a Signal<CriticalSectionRawMutex, u32>,
}

pub struct TcpRendezvousClient<'a> {
    events: Receiver<'a, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    commands: Sender<'a, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    disconnect: &'a Signal<CriticalSectionRawMutex, u32>,
}

pub fn tcp_rendezvous<'a>(
    stack: Stack<'a>,
    buffers: TcpRendezvousBuffers<'a>,
    storage: &'a mut TcpRendezvousStorage<'a>,
) -> (TcpRendezvousServer<'a>, TcpRendezvousClient<'a>) {
    let disconnect = &storage.disconnect;
    let (events_tx, events_rx) = storage.events.split();
    let (commands_tx, commands_rx) = storage.commands.split();
    (
        TcpRendezvousServer {
            stack,
            buffers,
            events: events_tx,
            commands: commands_rx,
            disconnect,
        },
        TcpRendezvousClient {
            events: events_rx,
            commands: commands_tx,
            disconnect,
        },
    )
}

impl TcpRendezvousClient<'_> {
    pub(super) async fn next_event_slot(&mut self) -> &mut TcpRendezvousWireSlot {
        self.events.receive().await
    }

    pub(super) fn event_received(&mut self) {
        self.events.receive_done();
    }

    pub(super) async fn send_frame(
        &mut self,
        session: u32,
        bytes: &[u8],
    ) -> Result<(), TcpRendezvousSendError> {
        if bytes.len() > TCP_RENDEZVOUS_FRAME_CAP {
            return Err(TcpRendezvousSendError::FrameTooLarge {
                len: bytes.len(),
                capacity: TCP_RENDEZVOUS_FRAME_CAP,
            });
        }
        let slot = self.commands.send().await;
        let result = slot.set_frame(session, InterfaceId::new([0u8; 8]), bytes);
        if result.is_err() {
            slot.set_control(WireKind::Empty, session, InterfaceId::new([0u8; 8]));
        }
        self.commands.send_done();
        result
    }

    pub(super) fn disconnect(&self, session: u32) {
        self.disconnect.signal(session);
    }
}

impl TcpRendezvousServer<'_> {
    pub async fn run(self) -> ! {
        let TcpRendezvousServer {
            stack,
            buffers,
            mut events,
            mut commands,
            disconnect,
        } = self;
        let mut socket = TcpSocket::new(stack, buffers.rx, buffers.tx);
        socket.set_timeout(Some(SOCKET_TIMEOUT));
        socket.set_keep_alive(Some(KEEP_ALIVE));
        let mut session = 0u32;
        stack.wait_config_up().await;
        crate::diagnostic_log::info!(
            "wifi-auto: TCP rendezvous listening on port {}",
            contract::TCP_RENDEZVOUS_PORT
        );

        loop {
            commands.clear();
            disconnect.reset();
            buffers.decoder.reset();
            if let Err(error) = socket.accept(contract::TCP_RENDEZVOUS_PORT).await {
                crate::diagnostic_log::warn!("wifi-auto: TCP rendezvous accept failed: {error:?}");
                Timer::after(ACCEPT_RETRY_DELAY).await;
                continue;
            }
            let Some(peer) = socket.remote_endpoint() else {
                socket.abort();
                let _ = with_timeout(FLUSH_TIMEOUT, socket.flush()).await;
                continue;
            };
            session = session.wrapping_add(1);
            if session == 0 {
                session = 1;
            }
            let id = peer_id(peer);
            crate::diagnostic_log::info!(
                "wifi-auto: TCP rendezvous connected peer={peer:?} session={session}"
            );
            send_control_event(&mut events, WireKind::Connected, session, id).await;
            serve_connection(
                &mut socket,
                buffers.decoder,
                buffers.read,
                buffers.framed,
                &mut events,
                &mut commands,
                disconnect,
                session,
                id,
            )
            .await;
            socket.abort();
            let flush = with_timeout(FLUSH_TIMEOUT, socket.flush()).await;
            crate::diagnostic_log::info!(
                "wifi-auto: TCP rendezvous disconnected peer={peer:?} session={session} flush={flush:?}"
            );
            send_control_event(&mut events, WireKind::Disconnected, session, id).await;
        }
    }
}

async fn send_control_event(
    events: &mut Sender<'_, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    kind: WireKind,
    session: u32,
    id: InterfaceId,
) {
    let slot = events.send().await;
    slot.set_control(kind, session, id);
    events.send_done();
}

#[expect(
    clippy::too_many_arguments,
    reason = "the connection loop borrows each external buffer independently across select branches"
)]
async fn serve_connection(
    socket: &mut TcpSocket<'_>,
    decoder: &mut RnsSerialDecoder<TCP_RENDEZVOUS_FRAME_CAP>,
    read_buffer: &mut [u8; TCP_RENDEZVOUS_READ_BUFFER_BYTES],
    framed_buffer: &mut [u8; TCP_RENDEZVOUS_FRAMED_LEN],
    events: &mut Sender<'_, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    commands: &mut Receiver<'_, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
    disconnect: &Signal<CriticalSectionRawMutex, u32>,
    session: u32,
    id: InterfaceId,
) {
    loop {
        match select3(
            socket.read(read_buffer),
            commands.receive(),
            disconnect.wait(),
        )
        .await
        {
            Either3::First(Ok(0) | Err(_)) => return,
            Either3::First(Ok(read)) => {
                let mut offset = 0;
                let chunk = &read_buffer[..read];
                while offset < chunk.len() {
                    match decoder.feed_slice_next(chunk, &mut offset) {
                        Ok(Some([])) => {}
                        Ok(Some(frame)) => {
                            let slot = events.send().await;
                            if slot.set_frame(session, id, frame).is_err() {
                                slot.set_control(WireKind::Empty, session, id);
                            }
                            events.send_done();
                        }
                        Ok(None) => break,
                        Err(error) => crate::diagnostic_log::warn!(
                            "wifi-auto: TCP rendezvous decode failed: {error:?}"
                        ),
                    }
                }
            }
            Either3::Second(command) => {
                let write_failed = if let Some(bytes) = command.frame_for_session(session) {
                    match rns_serial_framing::encode(bytes, framed_buffer) {
                        Ok(framed) => tcp_write_all(socket, &framed_buffer[..framed])
                            .await
                            .is_err(),
                        Err(_) => {
                            crate::diagnostic_log::warn!("wifi-auto: TCP rendezvous encode failed");
                            false
                        }
                    }
                } else {
                    false
                };
                commands.receive_done();
                if write_failed {
                    crate::diagnostic_log::warn!("wifi-auto: TCP rendezvous write failed");
                    return;
                }
            }
            Either3::Third(disconnect_session) => {
                if disconnect_session == session {
                    return;
                }
            }
        }
    }
}

async fn tcp_write_all(socket: &mut TcpSocket<'_>, mut bytes: &[u8]) -> Result<(), ()> {
    while !bytes.is_empty() {
        let written = socket.write(bytes).await.map_err(|_| ())?;
        if written == 0 {
            return Err(());
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

fn peer_id(peer: IpEndpoint) -> InterfaceId {
    let mut tag = [0u8; 19];
    let len = match peer.addr {
        IpAddress::Ipv4(address) => {
            tag[0] = 4;
            tag[1..5].copy_from_slice(&address.octets());
            5
        }
        IpAddress::Ipv6(address) => {
            tag[0] = 6;
            tag[1..17].copy_from_slice(&address.octets());
            17
        }
    };
    tag[len..len + 2].copy_from_slice(&peer.port.to_be_bytes());
    InterfaceId::from_channel_tag(InterfaceKind::TcpServerPeer, &tag[..len + 2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::block_on;
    use embassy_net::{Ipv4Address, Ipv6Address};

    #[test]
    fn frame_capacity_is_enforced_before_queueing() {
        let mut slot = TcpRendezvousWireSlot::empty();
        assert_eq!(
            slot.set_frame(
                1,
                InterfaceId::new([0u8; 8]),
                &[0u8; TCP_RENDEZVOUS_FRAME_CAP + 1]
            ),
            Err(TcpRendezvousSendError::FrameTooLarge {
                len: TCP_RENDEZVOUS_FRAME_CAP + 1,
                capacity: TCP_RENDEZVOUS_FRAME_CAP,
            })
        );
    }

    #[test]
    fn peer_ids_include_address_family_address_and_port() {
        let v4 = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(192, 168, 4, 2)), 41000);
        let other_port = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(192, 168, 4, 2)), 41001);
        let v6 = IpEndpoint::new(IpAddress::Ipv6(Ipv6Address::LOCALHOST), 41000);

        assert_eq!(peer_id(v4).kind(), Some(InterfaceKind::TcpServerPeer));
        assert_ne!(peer_id(v4), peer_id(other_port));
        assert_ne!(peer_id(v4), peer_id(v6));
    }

    #[test]
    fn bridge_preserves_session_and_frame_bytes() {
        let mut event_slots = [TcpRendezvousWireSlot::empty()];
        let mut command_slots = [TcpRendezvousWireSlot::empty()];
        let mut storage = TcpRendezvousStorage::new(&mut event_slots, &mut command_slots);
        let (mut sender, mut receiver) = storage.commands.split();

        block_on(async {
            let slot = sender.send().await;
            slot.set_frame(7, InterfaceId::new([0u8; 8]), b"reticulum")
                .unwrap();
            sender.send_done();
            let frame = receiver.receive().await;
            assert_eq!(frame.session, 7);
            assert_eq!(frame.bytes(), b"reticulum");
            receiver.receive_done();
        });
    }

    #[test]
    fn outbound_frames_are_scoped_to_one_session() {
        let mut slot = TcpRendezvousWireSlot::empty();
        slot.set_frame(7, InterfaceId::new([0u8; 8]), b"current")
            .unwrap();

        assert_eq!(slot.frame_for_session(7), Some(b"current".as_slice()));
        assert_eq!(slot.frame_for_session(8), None);
    }
}
