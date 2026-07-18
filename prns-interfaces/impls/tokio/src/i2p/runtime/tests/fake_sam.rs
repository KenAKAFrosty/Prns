use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use tokio::io::{BufReader, DuplexStream};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::i2p::sam::{
    I2pAcceptedStream, I2pAddress, I2pGeneratedDestination, I2pPrivateDestination,
    I2pPublicDestination, SamSessionDestination, SamSessionId,
};
use crate::i2p::{SamBridgeTransport, SamFailureClass, SamSessionTransport, SamTransportError};

type FakeAcceptResult = Result<I2pAcceptedStream<DuplexStream>, FakeSamError>;
type FakeAcceptSender = mpsc::UnboundedSender<FakeAcceptResult>;
type FakeAcceptReceiver = mpsc::UnboundedReceiver<FakeAcceptResult>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FakeSamError {
    BridgeUnavailable,
    PeerUnreachable,
    SessionLost,
}

impl fmt::Display for FakeSamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BridgeUnavailable => formatter.write_str("fake SAM bridge unavailable"),
            Self::PeerUnreachable => formatter.write_str("fake I2P peer unreachable"),
            Self::SessionLost => formatter.write_str("fake SAM session lost"),
        }
    }
}

impl std::error::Error for FakeSamError {}

impl SamTransportError for FakeSamError {
    fn failure_class(&self) -> SamFailureClass {
        match self {
            Self::PeerUnreachable => SamFailureClass::PeerUnreachable,
            Self::BridgeUnavailable | Self::SessionLost => SamFailureClass::SamUnavailable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecordedSessionDestination {
    Transient,
    Persistent,
}

#[derive(Clone)]
pub(super) struct FakeSamBridge {
    shared: Arc<FakeSamShared>,
}

struct FakeSamShared {
    state: Mutex<FakeSamState>,
    connected_streams_tx: mpsc::UnboundedSender<DuplexStream>,
    connected_streams_rx: AsyncMutex<mpsc::UnboundedReceiver<DuplexStream>>,
}

struct FakeSamState {
    generated: I2pGeneratedDestination,
    resolved: I2pPublicDestination,
    session_results: VecDeque<Result<(), FakeSamError>>,
    connect_results: VecDeque<Result<(), FakeSamError>>,
    session_attempts: usize,
    connect_attempts: usize,
    destination_generations: usize,
    resolved_names: Vec<String>,
    sessions: Vec<(String, RecordedSessionDestination)>,
    accept_senders: Vec<FakeAcceptSender>,
}

impl FakeSamBridge {
    pub(super) fn new() -> Self {
        let (connected_streams_tx, connected_streams_rx) = mpsc::unbounded_channel();
        Self {
            shared: Arc::new(FakeSamShared {
                state: Mutex::new(FakeSamState {
                    generated: I2pGeneratedDestination {
                        public: Some(public_destination(0x31)),
                        private: private_destination(0x31),
                    },
                    resolved: public_destination(0x42),
                    session_results: VecDeque::new(),
                    connect_results: VecDeque::new(),
                    session_attempts: 0,
                    connect_attempts: 0,
                    destination_generations: 0,
                    resolved_names: Vec::new(),
                    sessions: Vec::new(),
                    accept_senders: Vec::new(),
                }),
                connected_streams_tx,
                connected_streams_rx: AsyncMutex::new(connected_streams_rx),
            }),
        }
    }

    pub(super) fn queue_session_result(&self, result: Result<(), FakeSamError>) {
        self.state().session_results.push_back(result);
    }

    pub(super) fn queue_connect_result(&self, result: Result<(), FakeSamError>) {
        self.state().connect_results.push_back(result);
    }

    pub(super) fn session_attempts(&self) -> usize {
        self.state().session_attempts
    }

    pub(super) fn connect_attempts(&self) -> usize {
        self.state().connect_attempts
    }

    pub(super) fn destination_generations(&self) -> usize {
        self.state().destination_generations
    }

    pub(super) fn session_destinations(&self) -> Vec<RecordedSessionDestination> {
        self.state()
            .sessions
            .iter()
            .map(|(_, destination)| *destination)
            .collect()
    }

    pub(super) fn resolved_names(&self) -> Vec<String> {
        self.state().resolved_names.clone()
    }

    pub(super) async fn next_connected_stream(&self) -> DuplexStream {
        self.shared
            .connected_streams_rx
            .lock()
            .await
            .recv()
            .await
            .expect("a fake peer stream is connected")
    }

    pub(super) fn accept_session_count(&self) -> usize {
        self.state().accept_senders.len()
    }

    pub(super) fn inject_accepted(&self, peer: I2pPublicDestination) -> DuplexStream {
        let sender = self
            .state()
            .accept_senders
            .last()
            .cloned()
            .expect("a fake listening session exists");
        let (local, remote) = tokio::io::duplex(64 * 1024);
        sender
            .send(Ok(I2pAcceptedStream {
                peer,
                stream: BufReader::new(local),
            }))
            .expect("the fake listening session remains active");
        remote
    }

    pub(super) fn fail_latest_accept(&self, error: FakeSamError) {
        let sender = self
            .state()
            .accept_senders
            .last()
            .cloned()
            .expect("a fake listening session exists");
        sender
            .send(Err(error))
            .expect("the fake listening session remains active");
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeSamState> {
        self.shared.state.lock().expect("fake SAM state is valid")
    }
}

pub(super) struct FakeSamSession {
    shared: Arc<FakeSamShared>,
    private_destination: I2pPrivateDestination,
    accepts: Arc<AsyncMutex<FakeAcceptReceiver>>,
}

impl SamSessionTransport for FakeSamSession {
    type Stream = DuplexStream;
    type Error = FakeSamError;

    fn private_destination(&self) -> &I2pPrivateDestination {
        &self.private_destination
    }

    async fn connect(
        &self,
        _destination: I2pPublicDestination,
    ) -> Result<BufReader<Self::Stream>, Self::Error> {
        let result = {
            let mut state = self.shared.state.lock().expect("fake SAM state is valid");
            state.connect_attempts += 1;
            state.connect_results.pop_front().unwrap_or(Ok(()))
        };
        result?;
        let (local, remote) = tokio::io::duplex(64 * 1024);
        self.shared
            .connected_streams_tx
            .send(remote)
            .map_err(|_| FakeSamError::BridgeUnavailable)?;
        Ok(BufReader::new(local))
    }

    async fn accept(&self) -> Result<I2pAcceptedStream<Self::Stream>, Self::Error> {
        self.accepts
            .lock()
            .await
            .recv()
            .await
            .unwrap_or(Err(FakeSamError::BridgeUnavailable))
    }
}

impl SamBridgeTransport for FakeSamBridge {
    type Stream = DuplexStream;
    type Session = FakeSamSession;
    type Error = FakeSamError;

    async fn generate_destination(&self) -> Result<I2pGeneratedDestination, Self::Error> {
        let mut state = self.state();
        state.destination_generations += 1;
        Ok(state.generated.clone())
    }

    async fn resolve_destination(
        &self,
        name: I2pAddress,
    ) -> Result<I2pPublicDestination, Self::Error> {
        let mut state = self.state();
        state.resolved_names.push(name.as_str().to_owned());
        Ok(state.resolved.clone())
    }

    async fn create_session(
        &self,
        id: SamSessionId,
        destination: SamSessionDestination,
    ) -> Result<Self::Session, Self::Error> {
        let (private_destination, accepts) = {
            let mut state = self.state();
            state.session_attempts += 1;
            state.session_results.pop_front().unwrap_or(Ok(()))?;
            let (destination_kind, private_destination) = match destination {
                SamSessionDestination::Transient => (
                    RecordedSessionDestination::Transient,
                    private_destination(0x57),
                ),
                SamSessionDestination::Persistent(destination) => {
                    (RecordedSessionDestination::Persistent, destination)
                }
            };
            let (accepts_tx, accepts_rx) = mpsc::unbounded_channel();
            state
                .sessions
                .push((id.as_str().to_owned(), destination_kind));
            state.accept_senders.push(accepts_tx);
            (private_destination, accepts_rx)
        };
        Ok(FakeSamSession {
            shared: self.shared.clone(),
            private_destination,
            accepts: Arc::new(AsyncMutex::new(accepts)),
        })
    }
}

pub(super) fn private_destination(seed: u8) -> I2pPrivateDestination {
    let mut bytes = (0..512)
        .map(|index| seed.wrapping_add(index as u8))
        .collect::<Vec<_>>();
    bytes[385] = 0;
    bytes[386] = 0;
    I2pPrivateDestination::new(encode_destination(&bytes))
        .expect("the private destination fixture is valid")
}

pub(super) fn oracle_private_destination() -> I2pPrivateDestination {
    let mut bytes = (0..512)
        .map(|index| (index as u8).wrapping_mul(17).wrapping_add(3))
        .collect::<Vec<_>>();
    bytes[385] = 0;
    bytes[386] = 0;
    I2pPrivateDestination::new(encode_destination(&bytes))
        .expect("the Python-oracle private destination fixture is valid")
}

pub(super) fn public_destination(seed: u8) -> I2pPublicDestination {
    private_destination(seed)
        .public_destination()
        .expect("the public destination fixture is valid")
}

fn encode_destination(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(bytes)
        .replace('+', "-")
        .replace('/', "~")
}
