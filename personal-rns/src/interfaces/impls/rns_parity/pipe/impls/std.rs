//! WIP — unreviewed (PipeInterface, rns_parity). API, naming, and structure may still change.

use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::Duration;

use super::super::core::{descriptor, PIPE_MTU};
use crate::interfaces::impls::rns_parity::framed_stream::{self, ConnectionEnd};
use crate::interfaces::substrate::StdHostSubstrate;
use crate::interfaces::{
    ConnectionState, ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceId,
    InterfaceWorkerContext, OutboundDrain, SelfDrivenInterface,
};

type PipeContext = InterfaceWorkerContext<StdHostSubstrate<PIPE_MTU>>;

pub fn std_pipe_interface<Build>(
    id: InterfaceId,
    build: Build,
    respawn_delay: Duration,
) -> SelfDrivenInterface<impl FnOnce(PipeContext)>
where
    Build: FnMut() -> Command + Send + 'static,
{
    SelfDrivenInterface::new(descriptor(id), move |context| {
        std::thread::spawn(move || supervise(build, respawn_delay, context));
    })
}

fn supervise<Build>(mut build: Build, respawn_delay: Duration, context: PipeContext)
where
    Build: FnMut() -> Command,
{
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

    loop {
        if matches!(control.next_command(), Some(ControlCommand::Stop)) {
            break;
        }

        let mut command = build();
        command.stdin(Stdio::piped()).stdout(Stdio::piped());

        match command.spawn() {
            Ok(child) => {
                let end = serve_connection(
                    child,
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
        let _ = wake_rx.recv_timeout(respawn_delay);
    }

    control.report(ControlReport::Stopped);
}

fn serve_connection<I, O, C>(
    mut child: Child,
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
    let Some(stdout) = child.stdout.take() else {
        return ConnectionEnd::Disconnected;
    };
    let Some(mut stdin) = child.stdin.take() else {
        return ConnectionEnd::Disconnected;
    };

    let connection_dead = AtomicBool::new(false);

    let end = std::thread::scope(|scope| {
        let dead = &connection_dead;
        let reader = scope.spawn(move || framed_stream::read_loop(stdout, inbound, dead, wake_tx));

        control.report(ControlReport::ConnectionState(ConnectionState::Connected));
        let end =
            framed_stream::write_loop(&mut stdin, outbound, control, wake_rx, &connection_dead);

        let _ = child.kill();
        drop(stdin);
        let _ = reader.join();
        end
    });

    let _ = child.wait();
    end
}

#[cfg(all(test, unix))]
mod tests {
    use super::{std_pipe_interface, PIPE_MTU};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::sync_channel;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::interfaces::rns_serial_framing::{ESC, FLAG};
    use crate::interfaces::substrate::StdInterfaceSeam;
    use crate::interfaces::{ControlReport, Interface, InterfaceHandle, InterfaceId};

    const MAX_BUFFERED_PACKETS: usize = 8;
    const PATIENCE: Duration = Duration::from_secs(5);

    fn test_id() -> InterfaceId {
        InterfaceId::new([0xCA; 16])
    }

    fn cat_unbuffered() -> Command {
        let mut command = Command::new("cat");
        command.arg("-u");
        command
    }

    #[test]
    fn round_trips_a_packet_through_a_subprocess_and_reports_stopped() {
        let payload = [0x01u8, 0x02, FLAG, ESC, 0x03];

        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<PIPE_MTU>::new(
            test_id(),
            Instant::now(),
            MAX_BUFFERED_PACKETS,
            wake_tx,
        );

        let interface = std_pipe_interface(test_id(), cat_unbuffered, Duration::from_millis(20));
        let _drive = interface.start(worker_context);

        runtime_handle
            .acquire_send_grant(|buf| {
                buf[..payload.len()].copy_from_slice(&payload);
                payload.len()
            })
            .expect("outbound queue accepts the packet");

        let mut received: Option<std::vec::Vec<u8>> = None;
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

        runtime_handle.request_stop();

        let mut stopped = false;
        let deadline = Instant::now() + PATIENCE;
        while !stopped && Instant::now() < deadline {
            while let Some(report) = runtime_handle.next_report() {
                if matches!(report, ControlReport::Stopped) {
                    stopped = true;
                }
            }
            if !stopped {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(stopped, "interface reports Stopped after request_stop");
    }

    #[test]
    fn recovers_by_respawning_after_the_subprocess_exits() {
        let payload = [0xAAu8, FLAG, 0xBB];

        let (wake_tx, _wake_rx) = sync_channel::<()>(1);
        let StdInterfaceSeam {
            worker_context,
            mut runtime_handle,
        } = StdInterfaceSeam::<PIPE_MTU>::new(
            test_id(),
            Instant::now(),
            MAX_BUFFERED_PACKETS,
            wake_tx,
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let build = move || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut command = Command::new("sh");
                command.arg("-c").arg("exit 0");
                command
            } else {
                cat_unbuffered()
            }
        };
        let interface = std_pipe_interface(test_id(), build, Duration::from_millis(10));
        let _drive = interface.start(worker_context);

        let mut received: Option<std::vec::Vec<u8>> = None;
        let deadline = Instant::now() + PATIENCE;
        while received.is_none() && Instant::now() < deadline {
            let _ = runtime_handle.acquire_send_grant(|buf| {
                buf[..payload.len()].copy_from_slice(&payload);
                payload.len()
            });
            std::thread::sleep(Duration::from_millis(20));
            runtime_handle.drain_inbound(|packet| {
                received = Some(packet.bytes.to_vec());
            });
        }
        assert_eq!(
            received.as_deref(),
            Some(&payload[..]),
            "a packet round-trips once the interface respawns into a live subprocess"
        );

        runtime_handle.request_stop();
    }
}
