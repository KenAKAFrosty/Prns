//WIP NEEDS REVIEW
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};

use super::super::core::{
    descriptor, INITIAL_CONNECT_TIMEOUT, RECONNECT_WAIT, TCP_KEEPIDLE, TCP_MTU,
};
#[cfg(target_os = "linux")]
use super::super::core::{TCP_KEEPCNT, TCP_KEEPINTVL, TCP_USER_TIMEOUT};
use crate::interfaces::impls::rns_parity::framed_stream::{self, ConnectionEnd};
use crate::interfaces::substrate::StdHostSubstrate;
use crate::interfaces::{
    ConnectionState, ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceId,
    InterfaceWorkerContext, OutboundDrain, SelfDrivenInterface,
};

const ACCEPT_POLL: Duration = Duration::from_millis(250);

type TcpContext = InterfaceWorkerContext<StdHostSubstrate<TCP_MTU>>;

pub fn tcp_client_interface(
    id: InterfaceId,
    peer: SocketAddr,
) -> SelfDrivenInterface<impl FnOnce(TcpContext)> {
    SelfDrivenInterface::new(descriptor(id), move |context| {
        std::thread::spawn(move || run_client(peer, context));
    })
}

pub fn tcp_server_interface(
    id: InterfaceId,
    bind: SocketAddr,
) -> SelfDrivenInterface<impl FnOnce(TcpContext)> {
    SelfDrivenInterface::new(descriptor(id), move |context| {
        std::thread::spawn(move || run_server(bind, context));
    })
}

// `TCP_NODELAY` + keepalive, matching `connect` and `set_timeouts_linux` /
// `set_timeouts_osx` (`TCPInterface.py` L241, L181-L205); Darwin sets idle only.
fn configure(stream: &TcpStream) {
    let sock = SockRef::from(stream);
    let _ = sock.set_nodelay(true);

    let keepalive = TcpKeepalive::new().with_time(TCP_KEEPIDLE);
    #[cfg(target_os = "linux")]
    let keepalive = keepalive
        .with_interval(TCP_KEEPINTVL)
        .with_retries(TCP_KEEPCNT);
    let _ = sock.set_tcp_keepalive(&keepalive);

    #[cfg(target_os = "linux")]
    let _ = sock.set_tcp_user_timeout(Some(TCP_USER_TIMEOUT));
}

fn run_client(peer: SocketAddr, context: TcpContext) {
    let InterfaceWorkerContext {
        mut inbound,
        mut outbound,
        mut control,
    } = context;

    let (wake_tx, wake_rx) = sync_channel::<()>(1);
    outbound.arm_wake({
        let wake_tx = wake_tx.clone();
        move || {
            let _ = wake_tx.try_send(());
        }
    });

    // Initiator: connect (`INITIAL_CONNECT_TIMEOUT`), serve, and on any drop
    // reconnect forever every `RECONNECT_WAIT` (`TCPInterface.py` L230, L270).
    // Only initiators reconnect.
    loop {
        if matches!(control.next_command(), Some(ControlCommand::Stop)) {
            break;
        }

        match TcpStream::connect_timeout(&peer, INITIAL_CONNECT_TIMEOUT) {
            Ok(stream) => {
                configure(&stream);
                control.report(ControlReport::ConnectionState(ConnectionState::Connected));
                let end = serve(
                    stream,
                    &mut inbound,
                    &mut outbound,
                    &mut control,
                    &wake_rx,
                    &wake_tx,
                );
                if matches!(end, ConnectionEnd::Stopped) {
                    break;
                }
                control.report(ControlReport::ConnectionState(
                    ConnectionState::Reconnecting,
                ));
            }
            Err(_) => {
                control.report(ControlReport::ConnectionState(
                    ConnectionState::Reconnecting,
                ));
            }
        }

        while wake_rx.try_recv().is_ok() {}
        let _ = wake_rx.recv_timeout(RECONNECT_WAIT);
    }

    control.report(ControlReport::Stopped);
}

fn run_server(bind: SocketAddr, context: TcpContext) {
    let InterfaceWorkerContext {
        mut inbound,
        mut outbound,
        mut control,
    } = context;

    let (wake_tx, wake_rx) = sync_channel::<()>(1);
    outbound.arm_wake({
        let wake_tx = wake_tx.clone();
        move || {
            let _ = wake_tx.try_send(());
        }
    });

    let Ok(listener) = TcpListener::bind(bind) else {
        control.report(ControlReport::ConnectionState(ConnectionState::Failed));
        control.report(ControlReport::Stopped);
        return;
    };
    let _ = listener.set_nonblocking(true);

    // Responder: accept one peer, serve it as a non-initiator (no reconnect),
    // then re-accept. RNS's `TCPServerInterface` spawns a child interface per
    // client (`TCPInterface.py` L452, L575); multi-client fan-out is a parity
    // gap pending runtime dynamic-interface registration.
    loop {
        if matches!(control.next_command(), Some(ControlCommand::Stop)) {
            break;
        }

        match listener.accept() {
            Ok((stream, _peer)) => {
                let _ = stream.set_nonblocking(false);
                configure(&stream);
                control.report(ControlReport::ConnectionState(ConnectionState::Connected));
                let end = serve(
                    stream,
                    &mut inbound,
                    &mut outbound,
                    &mut control,
                    &wake_rx,
                    &wake_tx,
                );
                if matches!(end, ConnectionEnd::Stopped) {
                    break;
                }
                control.report(ControlReport::ConnectionState(
                    ConnectionState::Disconnected,
                ));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                while wake_rx.try_recv().is_ok() {}
                let _ = wake_rx.recv_timeout(ACCEPT_POLL);
            }
            Err(_) => {
                let _ = wake_rx.recv_timeout(ACCEPT_POLL);
            }
        }
    }

    control.report(ControlReport::Stopped);
}

fn serve<I, O, C>(
    stream: TcpStream,
    inbound: &mut I,
    outbound: &mut O,
    control: &mut C,
    wake_rx: &Receiver<()>,
    wake_tx: &SyncSender<()>,
) -> ConnectionEnd
where
    I: InboundSink + Send,
    O: OutboundDrain,
    C: ControlEndpoint,
{
    let Ok(reader) = stream.try_clone() else {
        return ConnectionEnd::Disconnected;
    };
    let mut writer = stream;
    let connection_dead = AtomicBool::new(false);

    std::thread::scope(|scope| {
        let dead = &connection_dead;
        let reader = scope.spawn(move || framed_stream::read_loop(reader, inbound, dead, wake_tx));

        let end =
            framed_stream::write_loop(&mut writer, outbound, control, wake_rx, &connection_dead);

        let _ = writer.shutdown(Shutdown::Both);
        let _ = reader.join();
        end
    })
}

#[cfg(test)]
mod tests {
    use super::{tcp_client_interface, tcp_server_interface, TCP_MTU};
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::mpsc::sync_channel;
    use std::time::{Duration, Instant};

    use crate::interfaces::rns_serial_framing::{self, RnsSerialDecoder, ESC, FLAG};
    use crate::interfaces::substrate::StdInterfaceSeam;
    use crate::interfaces::{ControlReport, Interface, InterfaceHandle, InterfaceId};

    const MAX_BUFFERED_PACKETS: usize = 8;
    const PATIENCE: Duration = Duration::from_secs(5);

    fn test_id() -> InterfaceId {
        InterfaceId::new([0x7C; 16])
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn send_frame(socket: &mut TcpStream, payload: &[u8]) {
        let mut buf = [0u8; rns_serial_framing::max_encoded_len(TCP_MTU)];
        let n = rns_serial_framing::encode(payload, &mut buf).unwrap();
        socket.write_all(&buf[..n]).unwrap();
        socket.flush().unwrap();
    }

    fn recv_frame(socket: &mut TcpStream) -> Option<Vec<u8>> {
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut decoder = RnsSerialDecoder::<TCP_MTU>::new();
        let mut out: Option<Vec<u8>> = None;
        let mut buf = [0u8; 256];
        let deadline = Instant::now() + PATIENCE;
        while out.is_none() && Instant::now() < deadline {
            match socket.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => decoder.feed_slice(&buf[..n], |frame| {
                    if !frame.is_empty() && out.is_none() {
                        out = Some(frame.to_vec());
                    }
                }),
                Err(_) => {}
            }
        }
        out
    }

    fn connect_within(addr: SocketAddr, patience: Duration) -> Option<TcpStream> {
        let deadline = Instant::now() + patience;
        while Instant::now() < deadline {
            if let Ok(stream) = TcpStream::connect(addr) {
                return Some(stream);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    fn accept_within(listener: &TcpListener, patience: Duration) -> Option<TcpStream> {
        listener.set_nonblocking(true).ok()?;
        let deadline = Instant::now() + patience;
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = listener.set_nonblocking(false);
                    return Some(stream);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return None,
            }
        }
        None
    }

    #[test]
    fn client_round_trips_through_a_raw_tcp_peer() {
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<TCP_MTU>::new(
            test_id(),
            Instant::now(),
            MAX_BUFFERED_PACKETS,
            wake_tx,
        );
        let _drive = tcp_client_interface(test_id(), addr).start(worker_context);

        let mut peer = accept_within(&listener, PATIENCE).expect("client connects to the peer");

        send_frame(&mut peer, &payload);
        let mut received: Option<Vec<u8>> = None;
        let deadline = Instant::now() + PATIENCE;
        while received.is_none() && Instant::now() < deadline {
            runtime_handle.drain_inbound(|packet| {
                assert_eq!(packet.source_interface, test_id());
                received = Some(packet.bytes.to_vec());
            });
            if received.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert_eq!(received.as_deref(), Some(&payload[..]));

        runtime_handle
            .acquire_send_grant(|buf| {
                buf[..payload.len()].copy_from_slice(&payload);
                payload.len()
            })
            .expect("outbound queue accepts the packet");
        assert_eq!(recv_frame(&mut peer).as_deref(), Some(&payload[..]));

        runtime_handle.request_stop();
        assert!(reports_stopped(&mut runtime_handle), "reports Stopped");
    }

    #[test]
    fn server_round_trips_with_a_raw_tcp_client() {
        let payload = [0xAAu8, FLAG, 0xBB, ESC, 0xCC];

        for _ in 0..8 {
            let addr: SocketAddr = ([127, 0, 0, 1], free_port()).into();
            let (wake_tx, _wake_rx) = sync_channel::<()>(1);
            let StdInterfaceSeam {
                worker_context,
                mut runtime_handle,
            } = StdInterfaceSeam::<TCP_MTU>::new(
                test_id(),
                Instant::now(),
                MAX_BUFFERED_PACKETS,
                wake_tx,
            );
            let _drive = tcp_server_interface(test_id(), addr).start(worker_context);

            let Some(mut peer) = connect_within(addr, Duration::from_secs(1)) else {
                runtime_handle.request_stop();
                continue;
            };

            send_frame(&mut peer, &payload);
            let mut received: Option<Vec<u8>> = None;
            let deadline = Instant::now() + PATIENCE;
            while received.is_none() && Instant::now() < deadline {
                runtime_handle.drain_inbound(|packet| {
                    received = Some(packet.bytes.to_vec());
                });
                if received.is_none() {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            assert_eq!(received.as_deref(), Some(&payload[..]));

            runtime_handle
                .acquire_send_grant(|buf| {
                    buf[..payload.len()].copy_from_slice(&payload);
                    payload.len()
                })
                .expect("outbound queue accepts the packet");
            assert_eq!(recv_frame(&mut peer).as_deref(), Some(&payload[..]));

            runtime_handle.request_stop();
            assert!(reports_stopped(&mut runtime_handle), "reports Stopped");
            return;
        }

        panic!("server interface never became connectable on a free port");
    }

    #[test]
    fn client_reconnects_after_the_peer_drops() {
        let payload = [0x09u8, FLAG, 0x08];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<TCP_MTU>::new(
            test_id(),
            Instant::now(),
            MAX_BUFFERED_PACKETS,
            wake_tx,
        );
        let _drive = tcp_client_interface(test_id(), addr).start(worker_context);

        let conn1 = accept_within(&listener, PATIENCE).expect("client makes its first connection");
        drop(conn1);

        let mut conn2 = accept_within(&listener, Duration::from_secs(12))
            .expect("initiator reconnects after the peer drops");

        send_frame(&mut conn2, &payload);
        let mut received: Option<Vec<u8>> = None;
        let deadline = Instant::now() + PATIENCE;
        while received.is_none() && Instant::now() < deadline {
            runtime_handle.drain_inbound(|packet| {
                received = Some(packet.bytes.to_vec());
            });
            if received.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert_eq!(
            received.as_deref(),
            Some(&payload[..]),
            "a packet round-trips on the reconnected link"
        );

        runtime_handle.request_stop();
    }

    fn reports_stopped(
        handle: &mut crate::interfaces::substrate::StdInterfaceHandle<TCP_MTU>,
    ) -> bool {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            while let Some(report) = handle.next_report() {
                if matches!(report, ControlReport::Stopped) {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }
}
