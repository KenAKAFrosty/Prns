use embassy_futures::select::{select3, select4, Either3, Either4};
use embassy_net::tcp::{Error as TcpIoError, TcpSocket};
use embassy_net::{IpAddress, IpEndpoint, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::zerocopy_channel::{Channel, Receiver, Sender};
use embassy_time::{with_timeout, Duration, Instant, Timer};

use prns_core::interfaces::rns_serial_framing::{self, RnsSerialDecoder};
use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::{InterfaceId, InterfaceKind};

pub const TCP_RENDEZVOUS_FRAME_CAP: usize = contract::HARDWARE_MTU;
pub const TCP_RENDEZVOUS_FRAMED_LEN: usize =
    rns_serial_framing::max_encoded_len(TCP_RENDEZVOUS_FRAME_CAP);
pub const TCP_RENDEZVOUS_READ_BUFFER_BYTES: usize = 1_024;
pub const TCP_RENDEZVOUS_SOCKET_BUFFER_BYTES: usize = 1_024;
pub const TCP_RENDEZVOUS_LIVENESS_TIMEOUT: Duration = Duration::from_secs(180);
pub const TCP_RENDEZVOUS_CLIENT_CAPACITY: usize = 4;

const KEEP_ALIVE: Duration = Duration::from_secs(5);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TcpRendezvousWriteFailure {
    Socket(TcpIoError),
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TcpRendezvousExitCause {
    PeerClosed,
    ReadFailure(TcpIoError),
    WriteFailure(TcpRendezvousWriteFailure),
    Timeout,
    RequestedDisconnect,
}

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
    ClientUnavailable { index: usize },
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

/// Four independent listeners share the SoftAP port through smoltcp. Once one listener accepts a
/// peer it stops matching new SYNs, so the next idle listener receives the next connection. Keeping
/// one SPSC event/command pair per listener also prevents a slow peer from consuming another
/// peer's command stream.
pub struct TcpRendezvousClients<'a> {
    clients: [TcpRendezvousClient<'a>; TCP_RENDEZVOUS_CLIENT_CAPACITY],
}

pub(super) struct TcpRendezvousClientEvent<'a> {
    pub index: usize,
    pub slot: &'a mut TcpRendezvousWireSlot,
}

impl<'a> TcpRendezvousClients<'a> {
    #[must_use]
    pub fn new(clients: [TcpRendezvousClient<'a>; TCP_RENDEZVOUS_CLIENT_CAPACITY]) -> Self {
        Self { clients }
    }

    pub(super) async fn next_event_slot(&mut self) -> TcpRendezvousClientEvent<'_> {
        let [first, second, third, fourth] = &mut self.clients;
        match select4(
            first.next_event_slot(),
            second.next_event_slot(),
            third.next_event_slot(),
            fourth.next_event_slot(),
        )
        .await
        {
            Either4::First(slot) => TcpRendezvousClientEvent { index: 0, slot },
            Either4::Second(slot) => TcpRendezvousClientEvent { index: 1, slot },
            Either4::Third(slot) => TcpRendezvousClientEvent { index: 2, slot },
            Either4::Fourth(slot) => TcpRendezvousClientEvent { index: 3, slot },
        }
    }

    pub(super) fn event_received(&mut self, index: usize) {
        if let Some(client) = self.clients.get_mut(index) {
            client.event_received();
        }
    }

    pub(super) async fn send_frame(
        &mut self,
        index: usize,
        session: u32,
        bytes: &[u8],
    ) -> Result<(), TcpRendezvousSendError> {
        let Some(client) = self.clients.get_mut(index) else {
            return Err(TcpRendezvousSendError::ClientUnavailable { index });
        };
        client.send_frame(session, bytes).await
    }

    pub(super) fn disconnect(&self, index: usize, session: u32) {
        if let Some(client) = self.clients.get(index) {
            client.disconnect(session);
        }
    }
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
            let connected_at = Instant::now();
            let exit = serve_connection(
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
            let lifetime_ms = connected_at.elapsed().as_millis();
            socket.abort();
            let flush = with_timeout(FLUSH_TIMEOUT, socket.flush()).await;
            crate::diagnostic_log::info!(
                "wifi-auto: TCP rendezvous disconnected peer={peer:?} session={session} exit={exit:?} lifetime_ms={lifetime_ms} flush={flush:?}"
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
) -> TcpRendezvousExitCause {
    loop {
        let next = with_timeout(
            TCP_RENDEZVOUS_LIVENESS_TIMEOUT,
            select3(
                socket.read(read_buffer),
                commands.receive(),
                disconnect.wait(),
            ),
        )
        .await;
        let Ok(next) = next else {
            return TcpRendezvousExitCause::Timeout;
        };
        match next {
            Either3::First(Ok(0)) => return TcpRendezvousExitCause::PeerClosed,
            Either3::First(Err(error)) => {
                return TcpRendezvousExitCause::ReadFailure(error);
            }
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
                let write_failure = if let Some(bytes) = command.frame_for_session(session) {
                    match rns_serial_framing::encode(bytes, framed_buffer) {
                        Ok(framed) => tcp_write_all(socket, &framed_buffer[..framed]).await.err(),
                        Err(_) => {
                            crate::diagnostic_log::warn!("wifi-auto: TCP rendezvous encode failed");
                            None
                        }
                    }
                } else {
                    None
                };
                commands.receive_done();
                if let Some(failure) = write_failure {
                    return TcpRendezvousExitCause::WriteFailure(failure);
                }
            }
            Either3::Third(disconnect_session) => {
                if disconnect_session == session {
                    return TcpRendezvousExitCause::RequestedDisconnect;
                }
            }
        }
    }
}

async fn tcp_write_all(
    socket: &mut TcpSocket<'_>,
    mut bytes: &[u8],
) -> Result<(), TcpRendezvousWriteFailure> {
    while !bytes.is_empty() {
        let written = socket
            .write(bytes)
            .await
            .map_err(TcpRendezvousWriteFailure::Socket)?;
        if written == 0 {
            return Err(TcpRendezvousWriteFailure::Closed);
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

    fn client_bridge<'a>(
        storage: &'a mut TcpRendezvousStorage<'a>,
    ) -> (
        Sender<'a, CriticalSectionRawMutex, TcpRendezvousWireSlot>,
        TcpRendezvousClient<'a>,
    ) {
        let disconnect = &storage.disconnect;
        let (events_tx, events_rx) = storage.events.split();
        let (commands_tx, _) = storage.commands.split();
        (
            events_tx,
            TcpRendezvousClient {
                events: events_rx,
                commands: commands_tx,
                disconnect,
            },
        )
    }

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
    fn client_fleet_reports_the_listener_that_produced_an_event() {
        let mut event0 = [TcpRendezvousWireSlot::empty()];
        let mut command0 = [TcpRendezvousWireSlot::empty()];
        let mut event1 = [TcpRendezvousWireSlot::empty()];
        let mut command1 = [TcpRendezvousWireSlot::empty()];
        let mut event2 = [TcpRendezvousWireSlot::empty()];
        let mut command2 = [TcpRendezvousWireSlot::empty()];
        let mut event3 = [TcpRendezvousWireSlot::empty()];
        let mut command3 = [TcpRendezvousWireSlot::empty()];
        let mut storage0 = TcpRendezvousStorage::new(&mut event0, &mut command0);
        let mut storage1 = TcpRendezvousStorage::new(&mut event1, &mut command1);
        let mut storage2 = TcpRendezvousStorage::new(&mut event2, &mut command2);
        let mut storage3 = TcpRendezvousStorage::new(&mut event3, &mut command3);
        let (_, client0) = client_bridge(&mut storage0);
        let (_, client1) = client_bridge(&mut storage1);
        let (mut producer2, client2) = client_bridge(&mut storage2);
        let (_, client3) = client_bridge(&mut storage3);
        let mut clients = TcpRendezvousClients::new([client0, client1, client2, client3]);

        block_on(async {
            let id = InterfaceId::new([9u8; 8]);
            let slot = producer2.send().await;
            slot.set_control(WireKind::Connected, 17, id);
            producer2.send_done();

            let index = {
                let event = clients.next_event_slot().await;
                assert_eq!(event.index, 2);
                assert_eq!(event.slot.session, 17);
                assert_eq!(event.slot.id, id);
                event.index
            };
            clients.event_received(index);
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

    #[test]
    fn rendezvous_liveness_exceeds_the_idle_resume_window() {
        assert!(TCP_RENDEZVOUS_LIVENESS_TIMEOUT > Duration::from_secs(120));
        assert!(KEEP_ALIVE < TCP_RENDEZVOUS_LIVENESS_TIMEOUT);
    }
}
