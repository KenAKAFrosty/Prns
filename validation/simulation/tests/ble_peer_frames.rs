use std::collections::VecDeque;
use std::future::{pending, poll_fn, Future};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use personal_rns::interfaces::bluetooth_auto::{
    copy_received_frame, BleIdentity, BleSink, BleSource, BLE_WIRE_FRAME_LEN,
};
use personal_rns::interfaces::{FrameSink, InterfaceStatus};
use personal_rns::manifold::interface_seam::{
    Interface, InterfaceSeam, OutboundDisposition, OutboundDropReason,
};
use prns_interfaces_tokio::bluetooth_auto::BluetoothPeer;

#[derive(Debug, Default, PartialEq, Eq)]
struct Capture {
    inbound: Vec<Vec<u8>>,
    outbound: Vec<Vec<u8>>,
    receive_capacities: Vec<usize>,
    outbound_custody: usize,
    completions: Vec<OutboundDisposition>,
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
    fn accept_outbound_custody(&mut self) {
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outbound_custody += 1;
        self.current_outbound.fill(0);
    }
    fn complete_outbound(&mut self, disposition: OutboundDisposition) {
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .completions
            .push(disposition);
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
            outbound_custody: 0,
            completions: vec![],
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
                outbound_custody: 0,
                completions: vec![],
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
            outbound_custody: 1,
            completions: vec![OutboundDisposition::Sent],
        }
    );
    assert_eq!(
        (status.rx_bytes(), status.tx_bytes()),
        (0, BLE_WIRE_FRAME_LEN as u64)
    );
}

#[tokio::test]
async fn oversized_outbound_is_rejected_whole_before_custody_or_send() {
    let Fixture {
        peer,
        seam,
        capture,
    } = fixture([ReadStep::Wait], vec![vec![0xA5; BLE_WIRE_FRAME_LEN + 1]]);
    let status = peer.status();
    peer.run(seam).await;
    assert_eq!(
        *capture.lock().unwrap(),
        Capture {
            completions: vec![OutboundDisposition::Dropped(OutboundDropReason::Rejected)],
            ..Capture::default()
        }
    );
    assert_eq!((status.rx_bytes(), status.tx_bytes()), (0, 0));
}

const DUPLEX_FRAME_BYTES: usize = 256;
const DUPLEX_FRAGMENT_BYTES: usize = 16;
const DUPLEX_POLL_BUDGET: usize = 32;

struct DuplexSource {
    fragments: tokio::sync::mpsc::Receiver<Vec<u8>>,
    accumulated: Vec<u8>,
    sending: tokio::sync::watch::Receiver<usize>,
}

impl BleSource for DuplexSource {
    type Error = SourceError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        self.sending
            .wait_for(|count| *count == 2)
            .await
            .map_err(|_| SourceError::Closed)?;
        while self.accumulated.len() < DUPLEX_FRAME_BYTES {
            let fragment = self.fragments.recv().await.ok_or(SourceError::Closed)?;
            self.accumulated.extend_from_slice(&fragment);
        }
        let length =
            copy_received_frame(&self.accumulated, out).map_err(|_| SourceError::InvalidFrame)?;
        self.accumulated.clear();
        Ok(length)
    }
}

struct DuplexSink {
    fragments: tokio::sync::mpsc::Sender<Vec<u8>>,
    sending: tokio::sync::watch::Sender<usize>,
    capture: Arc<Mutex<Capture>>,
}

impl BleSink for DuplexSink {
    type Error = SourceError;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        self.sending.send_modify(|count| *count += 1);
        for fragment in frame.chunks(DUPLEX_FRAGMENT_BYTES) {
            self.fragments
                .send(fragment.to_vec())
                .await
                .map_err(|_| SourceError::Closed)?;
        }
        self.capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outbound
            .push(frame.to_vec());
        Ok(())
    }
}

#[tokio::test]
async fn simultaneous_fragmented_sends_drain_receive_without_restarting_either_send() {
    let (first_tx, second_rx) = tokio::sync::mpsc::channel(1);
    let (second_tx, first_rx) = tokio::sync::mpsc::channel(1);
    let (sending, receiving) = tokio::sync::watch::channel(0);
    let first_capture = Arc::new(Mutex::new(Capture::default()));
    let second_capture = Arc::new(Mutex::new(Capture::default()));
    let first_frame = vec![0xA5; DUPLEX_FRAME_BYTES];
    let second_frame = vec![0xB6; DUPLEX_FRAME_BYTES];
    let make_peer = |identity, fragments, outbound, capture: Arc<Mutex<Capture>>| {
        BluetoothPeer::new(
            BleIdentity::new([identity; 16]),
            DuplexSource {
                fragments,
                accumulated: Vec::new(),
                sending: receiving.clone(),
            },
            DuplexSink {
                fragments: outbound,
                sending: sending.clone(),
                capture,
            },
        )
    };
    let first = make_peer(1, first_rx, first_tx, first_capture.clone());
    let second = make_peer(2, second_rx, second_tx, second_capture.clone());
    let first_status = first.status();
    let second_status = second.status();
    let make_seam = |capture, frame| Seam {
        capture,
        inbound: Vec::new(),
        outbound: VecDeque::from([frame]),
        current_outbound: Vec::new(),
    };
    let mut first =
        std::pin::pin!(first.run(make_seam(first_capture.clone(), first_frame.clone())));
    let mut second =
        std::pin::pin!(second.run(make_seam(second_capture.clone(), second_frame.clone())));
    for _ in 0..DUPLEX_POLL_BUDGET {
        poll_fn(|context| {
            assert!(first.as_mut().poll(context).is_pending());
            assert!(second.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    let first_capture = first_capture.lock().unwrap();
    let second_capture = second_capture.lock().unwrap();
    assert_eq!(
        (
            &first_capture.inbound,
            &first_capture.outbound,
            &second_capture.inbound,
            &second_capture.outbound
        ),
        (
            &vec![second_frame.clone()],
            &vec![first_frame.clone()],
            &vec![first_frame],
            &vec![second_frame]
        )
    );
    assert_eq!(*sending.borrow(), 2, "each send must start exactly once");
    assert_eq!(
        (
            first_capture.outbound_custody,
            second_capture.outbound_custody
        ),
        (1, 1)
    );
    assert_eq!(
        (&first_capture.completions, &second_capture.completions),
        (
            &vec![OutboundDisposition::Sent],
            &vec![OutboundDisposition::Sent]
        )
    );
    assert_eq!(
        (
            first_status.rx_bytes(),
            first_status.tx_bytes(),
            second_status.rx_bytes(),
            second_status.tx_bytes()
        ),
        (
            DUPLEX_FRAME_BYTES as u64,
            DUPLEX_FRAME_BYTES as u64,
            DUPLEX_FRAME_BYTES as u64,
            DUPLEX_FRAME_BYTES as u64
        )
    );
}

struct GatedSource {
    inner: Source,
    sending: tokio::sync::watch::Receiver<usize>,
}

impl BleSource for GatedSource {
    type Error = SourceError;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        self.sending
            .wait_for(|count| *count > 0)
            .await
            .map_err(|_| SourceError::Closed)?;
        self.inner.recv_frame(out).await
    }
}

struct ControlledSink {
    sending: tokio::sync::watch::Sender<usize>,
    completion: tokio::sync::oneshot::Receiver<Result<(), SourceError>>,
}

impl BleSink for ControlledSink {
    type Error = SourceError;

    async fn send_frame(&mut self, _frame: &[u8]) -> Result<(), Self::Error> {
        self.sending.send_modify(|count| *count += 1);
        (&mut self.completion)
            .await
            .map_err(|_| SourceError::Closed)?
    }
}

#[tokio::test]
async fn a_stalled_send_forwards_multiple_inbound_frames_and_settles_custody_exactly() {
    for result in [Ok(()), Err(SourceError::Closed)] {
        let success = result.is_ok();
        let capture = Arc::new(Mutex::new(Capture::default()));
        let seam = Seam {
            capture: capture.clone(),
            inbound: Vec::new(),
            outbound: VecDeque::from([vec![0xA5; DUPLEX_FRAME_BYTES]]),
            current_outbound: Vec::new(),
        };
        let (sending, receiving) = tokio::sync::watch::channel(0);
        let (complete, completion) = tokio::sync::oneshot::channel();
        let peer = BluetoothPeer::new(
            BleIdentity::new([1; 16]),
            GatedSource {
                inner: Source {
                    steps: [
                        ReadStep::Frame(vec![1, 2]),
                        ReadStep::Frame(Vec::new()),
                        ReadStep::Frame(vec![3]),
                        ReadStep::Wait,
                    ]
                    .into(),
                    capture: capture.clone(),
                },
                sending: receiving,
            },
            ControlledSink {
                sending: sending.clone(),
                completion,
            },
        );
        let status = peer.status();
        let mut running = std::pin::pin!(peer.run(seam));
        poll_fn(|context| {
            assert!(running.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(capture.lock().unwrap().inbound, vec![vec![1, 2], vec![3]]);
        assert_eq!((status.rx_bytes(), status.tx_bytes()), (3, 0));
        assert!(capture.lock().unwrap().completions.is_empty());
        assert!(complete.send(result).is_ok());
        poll_fn(|context| {
            assert_eq!(running.as_mut().poll(context).is_pending(), success);
            Poll::Ready(())
        })
        .await;
        assert_eq!(*sending.borrow(), 1);
        assert_eq!(capture.lock().unwrap().outbound_custody, 1);
        assert_eq!(
            capture.lock().unwrap().completions,
            vec![if success {
                OutboundDisposition::Sent
            } else {
                OutboundDisposition::Dropped(OutboundDropReason::TransportFailure)
            }]
        );
        assert_eq!(
            status.tx_bytes(),
            if success {
                DUPLEX_FRAME_BYTES as u64
            } else {
                0
            }
        );
    }
}
