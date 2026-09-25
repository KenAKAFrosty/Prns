use std::collections::VecDeque;
use std::future::{pending, poll_fn, Future};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use personal_rns::interfaces::bluetooth_auto::{
    copy_received_frame, BleIdentity, BleSink, BleSource, BLE_WIRE_FRAME_LEN,
};
use personal_rns::interfaces::{FrameSink, InterfaceStatus};
use personal_rns::manifold::interface_seam::{Interface, InterfaceSeam};
use prns_interfaces_tokio::bluetooth_auto::BluetoothPeer;

#[derive(Debug, Default, PartialEq, Eq)]
struct Capture {
    inbound: Vec<Vec<u8>>,
    outbound: Vec<Vec<u8>>,
    receive_capacities: Vec<usize>,
}

enum ReadStep {
    Frame(Vec<u8>),
    ReportedLength(usize),
    Wait,
}

#[derive(Debug)]
enum SourceError {
    Closed,
    InvalidFrame,
}

struct Source {
    steps: VecDeque<ReadStep>,
    capture: Arc<Mutex<Capture>>,
}
struct Sink(Arc<Mutex<Capture>>);
struct Seam {
    capture: Arc<Mutex<Capture>>,
    inbound: Vec<u8>,
    outbound: VecDeque<Vec<u8>>,
    current_outbound: Vec<u8>,
}

impl BleSource for Source {
    type Error = SourceError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        if matches!(self.steps.front(), Some(ReadStep::Wait)) {
            return pending().await;
        }
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .receive_capacities
            .push(out.len());
        match self.steps.pop_front().ok_or(SourceError::Closed)? {
            ReadStep::Frame(frame) => {
                copy_received_frame(&frame, out).map_err(|_| SourceError::InvalidFrame)
            }
            ReadStep::ReportedLength(length) => Ok(length),
            ReadStep::Wait => unreachable!("a parked read stays queued"),
        }
    }
}

impl BleSink for Sink {
    type Error = std::convert::Infallible;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outbound
            .push(frame.to_vec());
        Ok(())
    }
}

impl InterfaceSeam for Seam {
    fn fill_random(&mut self, bytes: &mut [u8]) {
        bytes.fill(0);
    }
    async fn inbound_sink(&mut self) -> &mut dyn FrameSink {
        &mut self.inbound
    }
    async fn commit_inbound(&mut self) {
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .inbound
            .push(std::mem::take(&mut self.inbound));
    }
    async fn next_outbound(&mut self) -> &[u8] {
        let Some(frame) = self.outbound.pop_front() else {
            return pending().await;
        };
        self.current_outbound = frame;
        &self.current_outbound
    }
}

struct Fixture {
    peer: BluetoothPeer<Source, Sink>,
    seam: Seam,
    capture: Arc<Mutex<Capture>>,
}

fn fixture(steps: impl IntoIterator<Item = ReadStep>, outbound: Vec<Vec<u8>>) -> Fixture {
    let capture = Arc::new(Mutex::new(Capture::default()));
    let peer = BluetoothPeer::new(
        BleIdentity::new([1; 16]),
        Source {
            steps: steps.into_iter().collect(),
            capture: capture.clone(),
        },
        Sink(capture.clone()),
    );
    let seam = Seam {
        capture: capture.clone(),
        inbound: Vec::new(),
        outbound: outbound.into(),
        current_outbound: Vec::new(),
    };
    Fixture {
        peer,
        seam,
        capture,
    }
}

#[tokio::test]
async fn maximum_wire_frames_preserve_ifac_headroom_and_empty_frames_do_not_count() {
    let maximum: Vec<_> = (0..BLE_WIRE_FRAME_LEN).map(|index| index as u8).collect();
    let small = vec![9, 8, 7];
    let Fixture {
        peer,
        seam,
        capture,
    } = fixture(
        [
            ReadStep::Frame(maximum.clone()),
            ReadStep::Frame(Vec::new()),
            ReadStep::Frame(small.clone()),
        ],
        vec![],
    );
    let status = peer.status();
    peer.run(seam).await;
    assert_eq!(
        *capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        Capture {
            inbound: vec![maximum, small],
            outbound: vec![],
            receive_capacities: vec![BLE_WIRE_FRAME_LEN; 4],
        }
    );
    assert_eq!(
        (status.rx_bytes(), status.tx_bytes()),
        ((BLE_WIRE_FRAME_LEN + 3) as u64, 0)
    );
}

#[tokio::test]
async fn oversized_or_dishonest_sources_close_without_forwarding_or_accounting_a_prefix() {
    for step in [
        ReadStep::Frame(vec![0xA5; BLE_WIRE_FRAME_LEN + 1]),
        ReadStep::ReportedLength(BLE_WIRE_FRAME_LEN + 1),
        ReadStep::ReportedLength(usize::MAX),
    ] {
        let Fixture {
            peer,
            seam,
            capture,
        } = fixture([step, ReadStep::Frame(vec![1, 2, 3])], vec![]);
        let status = peer.status();
        peer.run(seam).await;
        assert_eq!(
            *capture
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Capture {
                inbound: vec![],
                outbound: vec![],
                receive_capacities: vec![BLE_WIRE_FRAME_LEN],
            }
        );
        assert_eq!((status.rx_bytes(), status.tx_bytes()), (0, 0));
    }
}

#[tokio::test]
async fn maximum_wire_outbound_frames_are_unchanged_while_receive_is_parked() {
    let maximum: Vec<_> = (0..BLE_WIRE_FRAME_LEN).map(|index| index as u8).collect();
    let Fixture {
        peer,
        seam,
        capture,
    } = fixture([ReadStep::Wait], vec![maximum.clone()]);
    let status = peer.status();
    let mut running = std::pin::pin!(peer.run(seam));
    poll_fn(|cx| {
        assert!(running.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(
        *capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        Capture {
            inbound: vec![],
            outbound: vec![maximum],
            receive_capacities: vec![],
        }
    );
    assert_eq!(
        (status.rx_bytes(), status.tx_bytes()),
        (0, BLE_WIRE_FRAME_LEN as u64)
    );
}
