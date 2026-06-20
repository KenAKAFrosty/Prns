use std::io;
use std::net::{IpAddr, SocketAddr};

use tokio::net::UdpSocket;

use crate::engine::InstantMillis;
use crate::interfaces::rns_parity::udp::core;
use crate::interfaces::{ConnectionState, InterfaceConfig, InterfaceId, InterfaceKind};
use crate::reactor::airtime::{frame_airtime_us, AirtimeLedger};
use crate::reactor::impls::tokio_reactor::TokioInterfaceStatus;
use crate::reactor::interface_seam::{Interface, InterfaceSeam};
use crate::reactor::throughput::ThroughputLedger;

/// One end of an RNS UDP pair (`UDPInterface` parity): bind the local address up front —
/// so the bound port is readable and a bind refusal surfaces here — and forward every
/// outbound frame to the fixed `peer`, exactly as the reference forwards to its
/// configured `forward_ip:forward_port`. Datagrams arriving on the bound port are taken
/// as whole wire frames regardless of source (reference parity: its handler never
/// inspects the sender). Connectionless, so there is no reconnect machinery to mirror;
/// `bitrate_bps` is the host's claim about its pipe and sets the declared hardware MTU
/// through the reference's tier table, datagram-clamped.
pub struct UdpInterface {
    id: InterfaceId,
    socket: UdpSocket,
    peer: SocketAddr,
    bitrate_bps: u32,
    channel_tag: heapless::Vec<u8, 18>,
    status: TokioInterfaceStatus,
}

/// The bytes that tag this UDP channel: the fixed forward target it speaks to (RNS forwards every
/// datagram to one `forward_ip:forward_port`), the same target across a reconnect.
fn udp_channel_tag(peer: SocketAddr) -> heapless::Vec<u8, 18> {
    let mut tag = heapless::Vec::new();
    match peer.ip() {
        IpAddr::V4(v4) => {
            let _ = tag.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            let _ = tag.extend_from_slice(&v6.octets());
        }
    }
    let _ = tag.extend_from_slice(&peer.port().to_be_bytes());
    tag
}

impl UdpInterface {
    pub async fn bind(
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        bitrate_bps: u32,
    ) -> io::Result<Self> {
        let (socket, peer) = Self::bind_socket(local, peer).await?;
        Ok(Self::assemble(None, socket, peer, bitrate_bps))
    }

    /// Bind with a caller-chosen id instead of one derived from the forward target — for advanced
    /// setups that drive the reactor by hand and must pin the interface to a routing key their own
    /// wiring references. Ordinary nodes call [`bind`](Self::bind).
    pub async fn bind_with_id(
        id: InterfaceId,
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
        bitrate_bps: u32,
    ) -> io::Result<Self> {
        let (socket, peer) = Self::bind_socket(local, peer).await?;
        Ok(Self::assemble(Some(id), socket, peer, bitrate_bps))
    }

    async fn bind_socket(
        local: impl tokio::net::ToSocketAddrs,
        peer: impl tokio::net::ToSocketAddrs,
    ) -> io::Result<(UdpSocket, SocketAddr)> {
        let socket = UdpSocket::bind(local).await?;
        let peer = tokio::net::lookup_host(peer)
            .await?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "peer resolved to nothing"))?;
        Ok((socket, peer))
    }

    fn assemble(
        id_override: Option<InterfaceId>,
        socket: UdpSocket,
        peer: SocketAddr,
        bitrate_bps: u32,
    ) -> Self {
        let channel_tag = udp_channel_tag(peer);
        let id = id_override
            .unwrap_or_else(|| InterfaceId::from_channel_tag(InterfaceKind::Udp, &channel_tag));
        Self {
            id,
            socket,
            peer,
            bitrate_bps,
            channel_tag,
            status: TokioInterfaceStatus::new(id, ConnectionState::Initializing),
        }
    }

    /// This interface's id: derived from its forward target by [`bind`](Self::bind), or the one
    /// handed to [`bind_with_id`](Self::bind_with_id). For the app that wants to name it (an
    /// [`AnnounceTarget::Interface`](crate::engine::AnnounceTarget), a log line).
    #[must_use]
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// A clone of this interface's live-status handle for the app to read on its own render
    /// cadence. Call before [`run`](Interface::run) consumes the interface.
    #[must_use]
    pub fn status(&self) -> TokioInterfaceStatus {
        self.status.clone()
    }
}

impl Interface for UdpInterface {
    const HW_MTU: usize = core::UDP_HW_MTU_CAP;
    const KIND: InterfaceKind = InterfaceKind::Udp;

    fn descriptor(&self) -> InterfaceConfig {
        core::descriptor(self.id, self.bitrate_bps)
    }

    fn channel_tag(&self) -> &[u8] {
        &self.channel_tag
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        let mut airtime = AirtimeLedger::new();
        let mut throughput = ThroughputLedger::new();
        let started = tokio::time::Instant::now();
        let mut recv_buf = std::vec![0u8; core::RECV_BUF_LEN].into_boxed_slice();
        self.status.set_connection(ConnectionState::Connected);
        loop {
            tokio::select! {
                received = self.socket.recv_from(&mut recv_buf) => {
                    let Ok((len, _)) = received else { continue };
                    if len == 0 {
                        continue;
                    }
                    self.status.add_rx(len as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_rx(now, len as u64);
                    self.status.set_transfer_rates(throughput.rates());
                    seam.next_inbound(&recv_buf[..len]).await;
                }
                outbound = seam.next_outbound() => {
                    if outbound.is_empty() || outbound.len() > core::UDP_DATAGRAM_MAX {
                        continue;
                    }
                    let Ok(sent) = self.socket.send_to(outbound, self.peer).await else {
                        continue;
                    };
                    self.status.add_tx(sent as u64);
                    let now = InstantMillis(started.elapsed().as_millis() as u64);
                    throughput.record_tx(now, sent as u64);
                    self.status.set_transfer_rates(throughput.rates());
                    let frame_airtime = frame_airtime_us(sent, self.bitrate_bps);
                    self.status.set_airtime(airtime.record_tx(now, frame_airtime));
                }
            }
        }
    }
}

impl crate::interfaces::ReportsStatus for UdpInterface {
    fn status_view(&self) -> Option<crate::interfaces::StatusView> {
        let status = self.status();
        Some(std::sync::Arc::new(move || {
            std::vec![crate::interfaces::InterfaceSnapshot::of(&status)]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::impls::tokio_reactor::{tokio_grant_lane, TokioGrantConsumer};
    use std::time::Duration;
    use tokio::sync::mpsc::{self, UnboundedSender};

    const TEST_FRAME_CAP: usize = 2_048;

    /// A hand-driven seam, as in the TCP tests: captures every `next_inbound`, supplies
    /// `next_outbound` from a grant lane the test fills.
    struct MockSeam {
        inbound: UnboundedSender<std::vec::Vec<u8>>,
        outbound: TokioGrantConsumer,
    }

    impl InterfaceSeam for MockSeam {
        async fn next_inbound(&mut self, frame: &[u8]) {
            let _ = self.inbound.send(frame.to_vec());
        }

        async fn next_outbound(&mut self) -> &[u8] {
            self.outbound.release();
            self.outbound.peek().await.frame()
        }
    }

    #[tokio::test]
    async fn datagrams_cross_unframed_in_both_directions_over_real_sockets() {
        let far = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("binds the test peer");
        let far_addr = far.local_addr().expect("the peer address is known");

        let interface = UdpInterface::bind("127.0.0.1:0", far_addr, core::UDP_BITRATE_GUESS_BPS)
            .await
            .expect("binds an ephemeral local port");
        let near_addr = interface.local_addr().expect("the bound address is known");

        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<std::vec::Vec<u8>>();
        let (mut out_tx, out_rx) = tokio_grant_lane(TEST_FRAME_CAP, 2);
        let seam = MockSeam {
            inbound: in_tx,
            outbound: out_rx,
        };
        tokio::spawn(interface.run(seam));

        // Inbound: one datagram is one frame, byte-for-byte — no framing to strip.
        let payload = [0x7Eu8, 0x01, 0x7D, 0x02, 0x7E];
        far.send_to(&payload, near_addr)
            .await
            .expect("the test peer transmits");
        let received = tokio::time::timeout(Duration::from_secs(2), in_rx.recv())
            .await
            .expect("the interface hands the datagram up within the window")
            .expect("the interface task is alive");
        assert_eq!(received, payload, "raw bytes, exactly as sent");

        // Outbound: the seam yields a frame; it arrives at the fixed peer as one datagram.
        let out_payload = [0xAAu8, 0x7E, 0xBB];
        out_tx
            .try_grant()
            .expect("the outbound lane has a free slot")
            .fill(&out_payload);
        out_tx.commit();
        let mut buf = [0u8; 64];
        let (len, from) = tokio::time::timeout(Duration::from_secs(2), far.recv_from(&mut buf))
            .await
            .expect("the datagram arrives within the window")
            .expect("the test peer receives");
        assert_eq!(&buf[..len], out_payload, "raw bytes, exactly as granted");
        assert_eq!(from, near_addr, "sent from the interface's bound port");
    }

    /// The live-reactor capstone over real UDP: the TCP capstone's lifecycle — announce,
    /// establish, data, the ProveAll proof settling Delivered — with the stream replaced
    /// by two fixed-peer datagram sockets, proving the matrix's UDP leg end to end.
    #[tokio::test]
    async fn a_link_establishes_and_carries_data_across_two_live_reactors_over_udp() {
        use crate::engine::test_support::{
            fixed_secret_key, personal_node_destination, second_secret_key, Cap,
        };
        use crate::engine::{
            AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
            EstablishLink, IssuedCommand, Journaled, LinkEstablished, RatchetPolicy, SendLink,
            SendLinkPayload, Settlement,
        };
        use crate::reactor::impls::tokio_reactor::{
            run, Egress, HostCommand, TokioHost, TokioInterfaceSeam,
        };
        use crate::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
        use crate::routing::delivery::Delivery;
        use crate::routing::links::LinkId;
        use crate::routing::upstream_app_destinations::ProofStrategy;

        let probe_a = UdpSocket::bind("127.0.0.1:0").await.expect("probes a port");
        let addr_a = probe_a.local_addr().expect("port known");
        let probe_b = UdpSocket::bind("127.0.0.1:0").await.expect("probes a port");
        let addr_b = probe_b.local_addr().expect("port known");
        drop(probe_a);
        drop(probe_b);

        let a_interface = UdpInterface::bind(addr_a, addr_b, core::UDP_BITRATE_GUESS_BPS)
            .await
            .expect("binds the initiator socket");
        let initiator_iface = a_interface.id();
        let b_interface = UdpInterface::bind(addr_b, addr_a, core::UDP_BITRATE_GUESS_BPS)
            .await
            .expect("binds the responder socket");
        let responder_iface = b_interface.id();

        let initiator_engine = EngineState::<Cap>::new(second_secret_key());
        let (a_notify_tx, a_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (a_in_tx, a_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (a_out_tx, a_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let a_seam = TokioInterfaceSeam::new(initiator_iface, a_in_tx, a_notify_tx, a_out_rx);
        let a_egress = Egress::new(std::vec![(initiator_iface, a_out_tx)]);
        let (a_command_tx, a_command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (a_heard_tx, mut a_heard_rx) = mpsc::unbounded_channel::<()>();
        let (a_settled_tx, mut a_settled_rx) = mpsc::unbounded_channel::<(CommandId, Settlement)>();
        let a_app = move |journaled: Journaled<'_>| match journaled {
            Journaled::AnnounceHeard { .. } => {
                let _ = a_heard_tx.send(());
            }
            Journaled::CommandSettled { id, settlement } => {
                let _ = a_settled_tx.send((id, settlement));
            }
            _ => {}
        };

        let responder_engine = {
            let mut engine: EngineState<Cap> = EngineState::new(fixed_secret_key());
            let node = engine.held_identity_hashes()[0];
            engine
                .register_single_destination(
                    &node,
                    "personal",
                    &["node"],
                    b"hello-personal",
                    ProofStrategy::ProveAll,
                    RatchetPolicy::NoRatchets,
                )
                .expect("registers the proving destination");
            engine
        };
        let (b_notify_tx, b_notify_rx) = mpsc::unbounded_channel::<InterfaceId>();
        let (b_in_tx, b_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (b_out_tx, b_out_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let b_seam = TokioInterfaceSeam::new(responder_iface, b_in_tx, b_notify_tx, b_out_rx);
        let b_egress = Egress::new(std::vec![(responder_iface, b_out_tx)]);
        let (b_command_tx, b_command_rx) = mpsc::unbounded_channel::<HostCommand>();
        let (b_established_tx, mut b_established_rx) = mpsc::unbounded_channel::<LinkEstablished>();
        let (b_delivered_tx, mut b_delivered_rx) =
            mpsc::unbounded_channel::<(LinkId, std::vec::Vec<u8>)>();
        let b_app = move |journaled: Journaled<'_>| match journaled {
            Journaled::LinkEstablished(established) => {
                let _ = b_established_tx.send(established);
            }
            Journaled::Delivered(Delivery::Link(link)) => {
                let _ = b_delivered_tx.send((link.link_id, link.plaintext.to_vec()));
            }
            _ => {}
        };

        tokio::spawn(run(
            initiator_engine,
            std::vec![core::descriptor(
                initiator_iface,
                core::UDP_BITRATE_GUESS_BPS
            )],
            std::vec![],
            TokioHost::new(),
            a_notify_rx,
            std::vec![(initiator_iface, a_in_rx)],
            a_command_rx,
            a_egress,
            a_app,
        ));
        tokio::spawn(run(
            responder_engine,
            std::vec![core::descriptor(
                responder_iface,
                core::UDP_BITRATE_GUESS_BPS
            )],
            std::vec![],
            TokioHost::new(),
            b_notify_rx,
            std::vec![(responder_iface, b_in_rx)],
            b_command_rx,
            b_egress,
            b_app,
        ));
        tokio::spawn(a_interface.run(a_seam));
        tokio::spawn(b_interface.run(b_seam));

        // UDP loses what nobody is bound for yet, so announce on a cadence until heard —
        // the same posture the live scenarios take.
        let heard = async {
            for attempt in 1.. {
                b_command_tx
                    .send(HostCommand::Engine(IssuedCommand {
                        id: CommandId(attempt),
                        command: EngineCommand::AnnounceNow(AnnounceNow {
                            destination: personal_node_destination(),
                            target: AnnounceTarget::AllInterfaces,
                            app_data: AnnounceAppData::Registered,
                        }),
                    }))
                    .unwrap();
                if tokio::time::timeout(Duration::from_millis(200), a_heard_rx.recv())
                    .await
                    .is_ok()
                {
                    return;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(5), heard)
            .await
            .expect("the announce crosses the wire");

        a_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(7),
                command: EngineCommand::EstablishLink(EstablishLink {
                    destination: personal_node_destination(),
                }),
            }))
            .unwrap();
        let (settled_id, settlement) =
            tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
                .await
                .expect("the link settles within the window")
                .expect("the initiator reactor is alive");
        assert_eq!(settled_id, CommandId(7));
        let Settlement::EstablishLink(Ok(established)) = settlement else {
            panic!("the command must settle established, got {settlement:?}");
        };
        let responder_side = tokio::time::timeout(Duration::from_secs(5), b_established_rx.recv())
            .await
            .expect("the responder journals the link up")
            .expect("the responder reactor is alive");
        assert_eq!(
            responder_side.link_id, established.link_id,
            "one link, two ends",
        );

        a_command_tx
            .send(HostCommand::Engine(IssuedCommand {
                id: CommandId(8),
                command: EngineCommand::SendLink(SendLink {
                    link_id: established.link_id,
                    payload: SendLinkPayload::from_slice(b"ping over real udp").unwrap(),
                }),
            }))
            .unwrap();
        let delivered = tokio::time::timeout(Duration::from_secs(5), b_delivered_rx.recv())
            .await
            .expect("the responder journals the delivery")
            .expect("the responder reactor is alive");
        assert_eq!(
            delivered,
            (established.link_id, b"ping over real udp".to_vec())
        );
        let (sent_id, sent) = tokio::time::timeout(Duration::from_secs(5), a_settled_rx.recv())
            .await
            .expect("the initiator's send settles")
            .expect("the initiator reactor is alive");
        assert_eq!(sent_id, CommandId(8));
        let Settlement::SendLink(Ok(_)) = sent else {
            panic!("the ProveAll responder's proof settles the send Delivered, got {sent:?}");
        };
    }
}
