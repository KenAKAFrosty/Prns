use std::fmt;
use std::time::Duration;
use std::vec::Vec;

use prns_core::interfaces::rns_management::{
    RnsInterfaceStatsDecodeError, RnsInterfaceStatsReport,
};
use prns_core::interfaces::shared_instance::rns_rpc::{
    RnsRpcRequest, RnsRpcScalarReply, RnsRpcScalarReplyDecodeError, RpcAuthenticationKey,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use super::authentication::{answer_client_challenge, deliver_our_challenge};
use super::framing::{read_frame, write_frame};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedInstanceRpcEndpoint {
    Tcp {
        control_port: u16,
    },
    #[cfg(target_os = "linux")]
    AbstractUnix {
        socket_path: String,
    },
}

impl SharedInstanceRpcEndpoint {
    pub const fn tcp(control_port: u16) -> Self {
        Self::Tcp { control_port }
    }

    #[cfg(target_os = "linux")]
    pub fn abstract_unix(socket_path: impl Into<String>) -> Self {
        Self::AbstractUnix {
            socket_path: socket_path.into(),
        }
    }
}

impl fmt::Display for SharedInstanceRpcEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp { control_port } => write!(formatter, "127.0.0.1:{control_port}"),
            #[cfg(target_os = "linux")]
            Self::AbstractUnix { socket_path } => write!(formatter, "\\0rns/{socket_path}/rpc"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedInstanceRpcClientPhase {
    Connect,
    AnswerInstanceChallenge,
    AuthenticateInstance,
    WriteRequest,
    ReadReply,
}

impl fmt::Display for SharedInstanceRpcClientPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connect => "connect",
            Self::AnswerInstanceChallenge => "answer the instance challenge",
            Self::AuthenticateInstance => "authenticate the instance",
            Self::WriteRequest => "write the request",
            Self::ReadReply => "read the reply",
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum SharedInstanceRpcClientError {
    TimedOut {
        endpoint: SharedInstanceRpcEndpoint,
        timeout: Duration,
    },
    Io {
        endpoint: SharedInstanceRpcEndpoint,
        phase: SharedInstanceRpcClientPhase,
        kind: std::io::ErrorKind,
    },
    CredentialsRejected,
    InstanceAuthenticationFailed,
    RequestEncode,
    InterfaceStatsReply(RnsInterfaceStatsDecodeError),
    LinkCountReply(RnsRpcScalarReplyDecodeError),
    InvalidLinkCount(RnsRpcScalarReply),
}

impl fmt::Display for SharedInstanceRpcClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut { endpoint, timeout } => write!(
                formatter,
                "shared-instance RPC at {endpoint} did not answer within {timeout:?}"
            ),
            Self::Io {
                endpoint,
                phase,
                kind,
            } => write!(
                formatter,
                "could not {phase} for shared-instance RPC at {endpoint}: {kind}"
            ),
            Self::CredentialsRejected => formatter
                .write_str("the shared RNS instance rejected this client's RPC credentials"),
            Self::InstanceAuthenticationFailed => formatter.write_str(
                "the shared RNS instance could not prove that it knows the configured RPC key",
            ),
            Self::RequestEncode => formatter.write_str("the RNS RPC request could not be encoded"),
            Self::InterfaceStatsReply(error) => {
                write!(formatter, "invalid rnstatus reply: {error}")
            }
            Self::LinkCountReply(error) => write!(formatter, "invalid link-count reply: {error}"),
            Self::InvalidLinkCount(reply) => {
                write!(
                    formatter,
                    "link-count reply must be a nonnegative integer, got {reply:?}"
                )
            }
        }
    }
}

impl std::error::Error for SharedInstanceRpcClientError {}

pub struct SharedInstanceRpcClient {
    endpoint: SharedInstanceRpcEndpoint,
    rpc_key: RpcAuthenticationKey,
    timeout: Duration,
}

impl SharedInstanceRpcClient {
    pub const fn new(
        endpoint: SharedInstanceRpcEndpoint,
        rpc_key: RpcAuthenticationKey,
        timeout: Duration,
    ) -> Self {
        Self {
            endpoint,
            rpc_key,
            timeout,
        }
    }

    pub async fn interface_stats(
        &self,
    ) -> Result<RnsInterfaceStatsReport, SharedInstanceRpcClientError> {
        let reply = self.exchange(RnsRpcRequest::InterfaceStats).await?;
        RnsInterfaceStatsReport::decode_message_pack(&reply)
            .map_err(SharedInstanceRpcClientError::InterfaceStatsReply)
    }

    pub async fn link_count(&self) -> Result<u64, SharedInstanceRpcClientError> {
        let reply = self.exchange(RnsRpcRequest::LinkCount).await?;
        let reply = RnsRpcScalarReply::decode_message_pack(&reply)
            .map_err(SharedInstanceRpcClientError::LinkCountReply)?;
        reply
            .nonnegative_integer()
            .ok_or(SharedInstanceRpcClientError::InvalidLinkCount(reply))
    }

    pub async fn exchange(
        &self,
        request: RnsRpcRequest,
    ) -> Result<Vec<u8>, SharedInstanceRpcClientError> {
        tokio::time::timeout(self.timeout, self.exchange_without_timeout(request))
            .await
            .map_err(|_| SharedInstanceRpcClientError::TimedOut {
                endpoint: self.endpoint.clone(),
                timeout: self.timeout,
            })?
    }

    async fn exchange_without_timeout(
        &self,
        request: RnsRpcRequest,
    ) -> Result<Vec<u8>, SharedInstanceRpcClientError> {
        match &self.endpoint {
            SharedInstanceRpcEndpoint::Tcp { control_port } => {
                let address = ("127.0.0.1", *control_port);
                let mut stream = TcpStream::connect(address)
                    .await
                    .map_err(|error| self.io_error(SharedInstanceRpcClientPhase::Connect, error))?;
                self.exchange_stream(&mut stream, request).await
            }
            #[cfg(target_os = "linux")]
            SharedInstanceRpcEndpoint::AbstractUnix { socket_path } => {
                let mut stream = connect_abstract_rpc(socket_path)
                    .map_err(|error| self.io_error(SharedInstanceRpcClientPhase::Connect, error))?;
                self.exchange_stream(&mut stream, request).await
            }
        }
    }

    async fn exchange_stream<S>(
        &self,
        stream: &mut S,
        request: RnsRpcRequest,
    ) -> Result<Vec<u8>, SharedInstanceRpcClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let accepted = answer_client_challenge(stream, &self.rpc_key)
            .await
            .map_err(|error| {
                self.io_error(SharedInstanceRpcClientPhase::AnswerInstanceChallenge, error)
            })?;
        if !accepted {
            return Err(SharedInstanceRpcClientError::CredentialsRejected);
        }
        let authenticated =
            deliver_our_challenge(stream, &self.rpc_key)
                .await
                .map_err(|error| {
                    self.io_error(SharedInstanceRpcClientPhase::AuthenticateInstance, error)
                })?;
        if !authenticated {
            return Err(SharedInstanceRpcClientError::InstanceAuthenticationFailed);
        }
        let request = request
            .encode_message_pack()
            .map_err(|_| SharedInstanceRpcClientError::RequestEncode)?;
        write_frame(stream, &request)
            .await
            .map_err(|error| self.io_error(SharedInstanceRpcClientPhase::WriteRequest, error))?;
        read_frame(stream)
            .await
            .map_err(|error| self.io_error(SharedInstanceRpcClientPhase::ReadReply, error))
    }

    fn io_error(
        &self,
        phase: SharedInstanceRpcClientPhase,
        error: std::io::Error,
    ) -> SharedInstanceRpcClientError {
        SharedInstanceRpcClientError::Io {
            endpoint: self.endpoint.clone(),
            phase,
            kind: error.kind(),
        }
    }
}

#[cfg(target_os = "linux")]
fn connect_abstract_rpc(socket_path: &str) -> std::io::Result<tokio::net::UnixStream> {
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}/rpc");
    let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
    let stream = std::os::unix::net::UnixStream::connect_addr(&address)?;
    stream.set_nonblocking(true)?;
    tokio::net::UnixStream::from_std(stream)
}

#[cfg(test)]
mod tests {
    use std::vec;

    use prns_core::identity::IdentityHash;
    use prns_core::interfaces::shared_instance::rns_rpc::{RpcAuthenticationKey, RpcRequest};

    use super::*;
    use crate::shared_instance::rpc_compat::authentication::{
        answer_client_challenge, deliver_our_challenge, SharedInstanceCredentials,
    };
    use crate::shared_instance::rpc_compat::framing::{read_frame, write_frame};

    fn credentials(key: u8) -> SharedInstanceCredentials {
        SharedInstanceCredentials::new(
            RpcAuthenticationKey::new(vec![key; 32]),
            IdentityHash::new([0x42; 16]),
        )
    }

    async fn serve_one(
        mut stream: tokio::io::DuplexStream,
        credentials: SharedInstanceCredentials,
        expected: RnsRpcRequest,
        reply: Vec<u8>,
    ) {
        assert!(deliver_our_challenge(&mut stream, credentials.rpc_key())
            .await
            .unwrap());
        assert!(answer_client_challenge(&mut stream, credentials.rpc_key())
            .await
            .unwrap());
        let request = read_frame(&mut stream).await.unwrap();
        assert_eq!(
            RpcRequest::decode(&request),
            Ok(RpcRequest::Msgpack(expected))
        );
        write_frame(&mut stream, &reply).await.unwrap();
    }

    #[tokio::test]
    async fn mutually_authenticates_and_decodes_link_count() {
        let credentials = credentials(0x11);
        let client = SharedInstanceRpcClient::new(
            SharedInstanceRpcEndpoint::tcp(1),
            credentials.rpc_key().clone(),
            Duration::from_secs(1),
        );
        let (mut client_stream, server_stream) = tokio::io::duplex(4_096);
        let server = tokio::spawn(serve_one(
            server_stream,
            credentials,
            RnsRpcRequest::LinkCount,
            vec![0x03],
        ));

        let reply = client
            .exchange_stream(&mut client_stream, RnsRpcRequest::LinkCount)
            .await
            .unwrap();
        let scalar = RnsRpcScalarReply::decode_message_pack(&reply).unwrap();
        assert_eq!(scalar.nonnegative_integer(), Some(3));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn reports_rejected_credentials_without_sending_a_request() {
        let client = SharedInstanceRpcClient::new(
            SharedInstanceRpcEndpoint::tcp(1),
            credentials(0x11).rpc_key().clone(),
            Duration::from_secs(1),
        );
        let (mut client_stream, mut server_stream) = tokio::io::duplex(4_096);
        let server = tokio::spawn(async move {
            assert!(
                !deliver_our_challenge(&mut server_stream, credentials(0x22).rpc_key())
                    .await
                    .unwrap()
            );
        });

        assert_eq!(
            client
                .exchange_stream(&mut client_stream, RnsRpcRequest::LinkCount)
                .await,
            Err(SharedInstanceRpcClientError::CredentialsRejected)
        );
        server.await.unwrap();
    }
}
