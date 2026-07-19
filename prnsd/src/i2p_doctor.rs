use std::fmt;
use std::io;
use std::time::Duration;

use personal_rns::i2p::sam::{
    SamControlError, SamProtocolError, SamReplyKind, SamSessionDestination,
};
use personal_rns::i2p::{
    generate_session_id, I2pSessionIdError, SamBridgeAddress, SamBridgeError, SamBridgeScope,
    TokioSamBridge,
};
use tokio::net::TcpStream;
use tokio::time::timeout;

const DOCTOR_TIMEOUT: Duration = Duration::from_secs(5);
const CONSOLE_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const JAVA_I2P_CONSOLE_HOST: &str = "127.0.0.1";
const JAVA_I2P_CONSOLE_PORT: u16 = 7657;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteSamAccess {
    LoopbackOnly,
    ExplicitlyAllowed,
}

#[derive(Debug)]
pub(super) struct I2pDoctorRequest {
    address: SamBridgeAddress,
    remote_access: RemoteSamAccess,
}

impl I2pDoctorRequest {
    pub(super) fn new(address: SamBridgeAddress, remote_access: RemoteSamAccess) -> Self {
        Self {
            address,
            remote_access,
        }
    }
}

#[derive(Debug)]
pub(super) struct I2pDoctorReady {
    address: SamBridgeAddress,
}

impl fmt::Display for I2pDoctorReady {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "I2P doctor: SAM 3.1 session creation succeeded at {}",
            self.address
        )?;
        writeln!(formatter, "  The I2P router and SAM bridge are ready for Prnsd.")?;
        formatter.write_str(
            "  Peer reachability was not tested; a newly started router may still be warming up.",
        )?;
        if self.address.scope() == SamBridgeScope::NonLoopback {
            formatter.write_str(
                "\n  Warning: this non-loopback SAM path is plaintext and must remain trusted and private.",
            )?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct SamConnectionFailure {
    address: SamBridgeAddress,
    source: io::Error,
}

#[derive(Debug)]
pub(super) struct SamProtocolFailure {
    address: SamBridgeAddress,
    source: SamControlError,
}

#[derive(Debug)]
pub(super) enum I2pDoctorError {
    UnsafeEndpoint(SamBridgeAddress),
    RouterUnavailable(SamConnectionFailure),
    SamDisabled(SamConnectionFailure),
    SamUnavailable(SamConnectionFailure),
    IncompatibleSam(SamProtocolFailure),
    SessionUnavailable(SamProtocolFailure),
    UnexpectedBridgeFailure {
        address: SamBridgeAddress,
        source: SamBridgeError,
    },
    TimedOut {
        address: SamBridgeAddress,
        duration: Duration,
    },
    SessionIdUnavailable(I2pSessionIdError),
}

impl fmt::Display for I2pDoctorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafeEndpoint(address) => write!(
                formatter,
                "I2P doctor refused the non-loopback SAM bridge {address}.\n  SAM is plaintext and carries I2P destination credentials.\n  Fix: use 127.0.0.1:7656, tunnel SAM to loopback, or rerun with --allow-remote-sam only for a trusted private path."
            ),
            Self::RouterUnavailable(failure) => write!(
                formatter,
                "I2P router was not found at the default SAM bridge {}: {}\n  The local Java I2P router console was also unavailable at http://{JAVA_I2P_CONSOLE_HOST}:{JAVA_I2P_CONSOLE_PORT}.\n  Fix: start Java I2P, wait for its router console to open, then rerun the I2P doctor.",
                failure.address, failure.source
            ),
            Self::SamDisabled(failure) => write!(
                formatter,
                "Java I2P appears to be running, but SAM is not accepting connections at {}: {}\n  Fix: open http://{JAVA_I2P_CONSOLE_HOST}:{JAVA_I2P_CONSOLE_PORT}, enable the SAM application bridge, then rerun the I2P doctor.",
                failure.address, failure.source
            ),
            Self::SamUnavailable(failure) => write!(
                formatter,
                "SAM is not accepting connections at {}: {}\n  Fix: verify the bridge address, start the I2P router, enable its SAM interface, and rerun the doctor.",
                failure.address, failure.source
            ),
            Self::IncompatibleSam(failure) => write!(
                formatter,
                "A service answered at {}, but SAM 3.1 negotiation was incompatible: {}\n  Fix: verify this is an I2P SAM endpoint with SAM 3.1 enabled, then inspect the router logs for the rejected exchange.",
                failure.address, failure.source
            ),
            Self::SessionUnavailable(failure) => write!(
                formatter,
                "SAM 3.1 negotiation succeeded at {}, but the router could not create a transient session: {}\n  Fix: allow the I2P router to finish starting, inspect its logs, then rerun the doctor.",
                failure.address, failure.source
            ),
            Self::UnexpectedBridgeFailure { address, source } => write!(
                formatter,
                "The SAM bridge at {address} failed during the session probe: {source}\n  Fix: inspect the I2P router logs, verify SAM 3.1 is enabled, then rerun the doctor."
            ),
            Self::TimedOut { address, duration } => write!(
                formatter,
                "The SAM probe at {address} did not finish within {} seconds.\n  Fix: verify the bridge address and router health, allow a newly started router to warm up, then rerun the doctor.",
                duration.as_secs()
            ),
            Self::SessionIdUnavailable(source) => write!(
                formatter,
                "Could not create the one-time SAM session identifier: {source}\n  Fix: verify the operating system random source is available, then rerun the doctor."
            ),
        }
    }
}

impl std::error::Error for I2pDoctorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RouterUnavailable(failure)
            | Self::SamDisabled(failure)
            | Self::SamUnavailable(failure) => Some(&failure.source),
            Self::IncompatibleSam(failure) | Self::SessionUnavailable(failure) => {
                Some(&failure.source)
            }
            Self::UnexpectedBridgeFailure { source, .. } => Some(source),
            Self::SessionIdUnavailable(source) => Some(source),
            Self::UnsafeEndpoint(_) | Self::TimedOut { .. } => None,
        }
    }
}

pub(super) async fn run(request: I2pDoctorRequest) -> Result<I2pDoctorReady, I2pDoctorError> {
    if request.address.scope() == SamBridgeScope::NonLoopback
        && request.remote_access == RemoteSamAccess::LoopbackOnly
    {
        return Err(I2pDoctorError::UnsafeEndpoint(request.address));
    }
    let session_id = generate_session_id().map_err(I2pDoctorError::SessionIdUnavailable)?;
    let bridge = TokioSamBridge::new(request.address.clone());
    let probe = bridge.create_session(session_id, SamSessionDestination::Transient);
    let session = match timeout(DOCTOR_TIMEOUT, probe).await {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => return Err(classify_bridge_error(request.address, error).await),
        Err(_) => {
            return Err(I2pDoctorError::TimedOut {
                address: request.address,
                duration: DOCTOR_TIMEOUT,
            })
        }
    };
    drop(session);
    Ok(I2pDoctorReady {
        address: request.address,
    })
}

async fn classify_bridge_error(
    requested_address: SamBridgeAddress,
    error: SamBridgeError,
) -> I2pDoctorError {
    match error {
        SamBridgeError::Connect { address, source } => {
            classify_connection_failure(address, source).await
        }
        SamBridgeError::Control(source) if is_session_rejection(&source) => {
            I2pDoctorError::SessionUnavailable(SamProtocolFailure {
                address: requested_address,
                source,
            })
        }
        SamBridgeError::Control(source) if source.protocol_error().is_some() => {
            I2pDoctorError::IncompatibleSam(SamProtocolFailure {
                address: requested_address,
                source,
            })
        }
        SamBridgeError::Control(source) => I2pDoctorError::UnexpectedBridgeFailure {
            address: requested_address,
            source: SamBridgeError::Control(source),
        },
        source @ SamBridgeError::Stream(_) => I2pDoctorError::UnexpectedBridgeFailure {
            address: requested_address,
            source,
        },
    }
}

fn is_session_rejection(error: &SamControlError) -> bool {
    matches!(
        error,
        SamControlError::Protocol(SamProtocolError::Rejected {
            kind: SamReplyKind::Session,
            ..
        })
    )
}

async fn classify_connection_failure(
    address: SamBridgeAddress,
    source: io::Error,
) -> I2pDoctorError {
    let failure = SamConnectionFailure { address, source };
    if failure.address != SamBridgeAddress::default() {
        return I2pDoctorError::SamUnavailable(failure);
    }
    if java_i2p_console_is_reachable().await {
        I2pDoctorError::SamDisabled(failure)
    } else {
        I2pDoctorError::RouterUnavailable(failure)
    }
}

async fn java_i2p_console_is_reachable() -> bool {
    timeout(
        CONSOLE_PROBE_TIMEOUT,
        TcpStream::connect((JAVA_I2P_CONSOLE_HOST, JAVA_I2P_CONSOLE_PORT)),
    )
    .await
    .is_ok_and(|connection| connection.is_ok())
}

#[cfg(test)]
mod tests {
    use personal_rns::i2p::sam::{SamRejection, SamReplyKind};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    use super::*;

    const REFERENCE_PRIVATE_DESTINATION_LEN: usize = 884;

    async fn read_command(reader: &mut BufReader<TcpStream>) -> String {
        let mut command = String::new();
        reader.read_line(&mut command).await.unwrap();
        command
    }

    #[tokio::test]
    async fn doctor_creates_and_releases_a_real_transient_sam_session() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = SamBridgeAddress::new(listener.local_addr().unwrap().to_string()).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            assert_eq!(
                read_command(&mut stream).await,
                "HELLO VERSION MIN=3.1 MAX=3.1\n"
            );
            stream
                .get_mut()
                .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
                .await
                .unwrap();
            let create = read_command(&mut stream).await;
            assert!(create.starts_with("SESSION CREATE STYLE=STREAM ID=reticulum-"));
            assert!(create.ends_with(" DESTINATION=TRANSIENT \n"));
            let private_destination = "S".repeat(REFERENCE_PRIVATE_DESTINATION_LEN);
            stream
                .get_mut()
                .write_all(
                    format!("SESSION STATUS RESULT=OK DESTINATION={private_destination}\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
        });

        let ready = run(I2pDoctorRequest::new(
            address.clone(),
            RemoteSamAccess::LoopbackOnly,
        ))
        .await
        .unwrap();
        assert_eq!(ready.address, address);
        assert!(ready.to_string().contains("Peer reachability was not tested"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn doctor_refuses_non_loopback_sam_before_connecting() {
        let address = SamBridgeAddress::new("router.internal:7656").unwrap();
        assert!(matches!(
            run(I2pDoctorRequest::new(
                address,
                RemoteSamAccess::LoopbackOnly,
            ))
            .await,
            Err(I2pDoctorError::UnsafeEndpoint(_))
        ));
    }

    #[tokio::test]
    async fn custom_unavailable_sam_does_not_claim_the_router_is_absent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = SamBridgeAddress::new(listener.local_addr().unwrap().to_string()).unwrap();
        drop(listener);

        assert!(matches!(
            run(I2pDoctorRequest::new(
                address,
                RemoteSamAccess::LoopbackOnly,
            ))
            .await,
            Err(I2pDoctorError::SamUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn session_rejections_are_distinct_from_sam_incompatibility() {
        let address = SamBridgeAddress::default();
        let session_rejection = SamControlError::Protocol(SamProtocolError::Rejected {
            kind: SamReplyKind::Session,
            rejection: SamRejection::I2pError,
            message: None,
        });
        let hello_rejection = SamControlError::Protocol(SamProtocolError::Rejected {
            kind: SamReplyKind::Hello,
            rejection: SamRejection::NoVersion,
            message: None,
        });

        assert!(matches!(
            classify_bridge_error(
                address.clone(),
                SamBridgeError::Control(session_rejection)
            )
            .await,
            I2pDoctorError::SessionUnavailable(_)
        ));
        assert!(matches!(
            classify_bridge_error(address, SamBridgeError::Control(hello_rejection)).await,
            I2pDoctorError::IncompatibleSam(_)
        ));
    }
}
