use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use std::vec::Vec;

use prns_core::identity::IdentityHash;
use prns_core::interfaces::rns_management::{
    RnsAnnounceRateTable, RnsAnnounceRateTableDecodeError, RnsBlackholeDecodeError,
    RnsBlackholeTable, RnsInterfaceStatsDecodeError, RnsInterfaceStatsReport, RnsPathTable,
    RnsPathTableDecodeError,
};
use prns_core::interfaces::shared_instance::rns_rpc::{
    PacketHashArgument, RnsInteger, RnsNumber, RnsRpcRequest, RnsRpcScalarReply,
    RnsRpcScalarReplyDecodeError, RpcAuthenticationKey, RpcDialect, RpcVerb, RNS_NO_INTERFACE_NAME,
};
use prns_core::interfaces::BitrateBps;
use prns_core::routing::dedup::PacketHash;
use prns_core::routing::BlackholedIdentity;
use prns_core::units::InstantMillis;
use prns_core::wire::{DestinationHash, TransportId};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use super::authentication::{answer_client_challenge, deliver_our_challenge};
use super::framing::{read_frame, write_frame};
use super::legacy;

const DIALECT_UNKNOWN: u8 = 0;
const DIALECT_MSGPACK: u8 = 1;
const DIALECT_PICKLE: u8 = 2;
const DIALECT_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedInstanceRpcEndpoint {
    Tcp {
        control_port: u16,
    },
    #[cfg(any(target_os = "linux", target_os = "android"))]
    AbstractUnix {
        socket_path: String,
    },
}

impl SharedInstanceRpcEndpoint {
    pub const fn tcp(control_port: u16) -> Self {
        Self::Tcp { control_port }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
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
            #[cfg(any(target_os = "linux", target_os = "android"))]
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
    LegacyReplyDecode,
    InterfaceStatsReply(RnsInterfaceStatsDecodeError),
    LinkCountReply(RnsRpcScalarReplyDecodeError),
    InvalidLinkCount(RnsRpcScalarReply),
    PathTableReply(RnsPathTableDecodeError),
    RateTableReply(RnsAnnounceRateTableDecodeError),
    BlackholeTableReply(RnsBlackholeDecodeError),
    ScalarReply {
        operation: RpcVerb,
        source: RnsRpcScalarReplyDecodeError,
    },
    UnexpectedScalarReply {
        operation: RpcVerb,
        reply: RnsRpcScalarReply,
    },
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
            Self::LegacyReplyDecode => {
                formatter.write_str("the legacy RNS RPC reply could not be decoded safely")
            }
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
            Self::PathTableReply(error) => write!(formatter, "invalid path-table reply: {error}"),
            Self::RateTableReply(error) => {
                write!(formatter, "invalid announce-rate reply: {error}")
            }
            Self::BlackholeTableReply(error) => {
                write!(formatter, "invalid blackhole-table reply: {error}")
            }
            Self::ScalarReply { operation, source } => {
                write!(formatter, "invalid {} reply: {source}", operation.as_str())
            }
            Self::UnexpectedScalarReply { operation, reply } => write!(
                formatter,
                "{} reply has an unexpected value: {reply:?}",
                operation.as_str()
            ),
        }
    }
}

impl std::error::Error for SharedInstanceRpcClientError {}

pub struct SharedInstanceRpcClient {
    endpoint: SharedInstanceRpcEndpoint,
    rpc_key: RpcAuthenticationKey,
    timeout: Duration,
    dialect: AtomicU8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SharedInstancePacketPhyStats {
    pub rssi_dbm: Option<f64>,
    pub snr_db: Option<f64>,
    pub quality_percent: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedInstanceBlackholeOutcome {
    Added,
    AlreadyPresent,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedInstanceUnblackholeOutcome {
    Removed,
    NotFound,
    Rejected,
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
            dialect: AtomicU8::new(DIALECT_UNKNOWN),
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

    pub async fn path_table(
        &self,
        maximum_hops: Option<i64>,
    ) -> Result<RnsPathTable, SharedInstanceRpcClientError> {
        let reply = self
            .exchange(RnsRpcRequest::PathTable {
                max_hops: maximum_hops.map(RnsInteger::from_i64),
            })
            .await?;
        RnsPathTable::decode_message_pack(&reply)
            .map_err(SharedInstanceRpcClientError::PathTableReply)
    }

    pub async fn announce_rate_table(
        &self,
    ) -> Result<RnsAnnounceRateTable, SharedInstanceRpcClientError> {
        let reply = self.exchange(RnsRpcRequest::RateTable).await?;
        RnsAnnounceRateTable::decode_message_pack(&reply)
            .map_err(SharedInstanceRpcClientError::RateTableReply)
    }

    pub async fn blackholed_identities(
        &self,
        now: InstantMillis,
    ) -> Result<Vec<BlackholedIdentity<String>>, SharedInstanceRpcClientError> {
        let reply = self.exchange(RnsRpcRequest::BlackholedIdentities).await?;
        RnsBlackholeTable::decode_published_table(&reply, now)
            .map(RnsBlackholeTable::into_entries)
            .map_err(SharedInstanceRpcClientError::BlackholeTableReply)
    }

    pub async fn next_hop(
        &self,
        destination: DestinationHash,
    ) -> Result<Option<TransportId>, SharedInstanceRpcClientError> {
        let reply = self
            .scalar(
                RnsRpcRequest::NextHop {
                    destination_hash: destination,
                },
                RpcVerb::GetNextHop,
            )
            .await?;
        match reply {
            RnsRpcScalarReply::Null => Ok(None),
            RnsRpcScalarReply::Binary(bytes) => {
                let bytes: [u8; 16] = bytes.try_into().map_err(|bytes: Vec<u8>| {
                    SharedInstanceRpcClientError::UnexpectedScalarReply {
                        operation: RpcVerb::GetNextHop,
                        reply: RnsRpcScalarReply::Binary(bytes),
                    }
                })?;
                Ok(Some(TransportId::new(bytes)))
            }
            reply => Err(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::GetNextHop,
                reply,
            }),
        }
    }

    pub async fn next_hop_interface_name(
        &self,
        destination: DestinationHash,
    ) -> Result<Option<String>, SharedInstanceRpcClientError> {
        let reply = self
            .scalar(
                RnsRpcRequest::NextHopInterface {
                    destination_hash: destination,
                },
                RpcVerb::GetNextHopInterfaceName,
            )
            .await?;
        match reply {
            RnsRpcScalarReply::String(name) if name == RNS_NO_INTERFACE_NAME => Ok(None),
            RnsRpcScalarReply::String(name) => Ok(Some(name)),
            reply => Err(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::GetNextHopInterfaceName,
                reply,
            }),
        }
    }

    pub async fn first_hop_timeout(
        &self,
        destination: DestinationHash,
    ) -> Result<Duration, SharedInstanceRpcClientError> {
        let seconds = self
            .numeric(
                RnsRpcRequest::FirstHopTimeout {
                    destination_hash: destination,
                },
                RpcVerb::GetFirstHopTimeout,
            )
            .await?
            .ok_or(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::GetFirstHopTimeout,
                reply: RnsRpcScalarReply::Null,
            })?;
        duration_from_seconds(seconds, RpcVerb::GetFirstHopTimeout)
    }

    pub async fn lowest_interface_bitrate(
        &self,
    ) -> Result<Option<BitrateBps>, SharedInstanceRpcClientError> {
        let reply = self
            .scalar(
                RnsRpcRequest::LowestInterfaceBitrate,
                RpcVerb::GetLowestInterfaceBitrate,
            )
            .await?;
        match reply {
            RnsRpcScalarReply::Null => Ok(None),
            RnsRpcScalarReply::Integer(value) => value
                .nonnegative_value()
                .and_then(BitrateBps::new)
                .map(Some)
                .ok_or(SharedInstanceRpcClientError::UnexpectedScalarReply {
                    operation: RpcVerb::GetLowestInterfaceBitrate,
                    reply: RnsRpcScalarReply::Integer(value),
                }),
            reply => Err(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::GetLowestInterfaceBitrate,
                reply,
            }),
        }
    }

    pub async fn medium_path_timeout(&self) -> Result<Duration, SharedInstanceRpcClientError> {
        let seconds = self
            .numeric(
                RnsRpcRequest::MediumPathTimeout,
                RpcVerb::GetMediumPathTimeout,
            )
            .await?
            .ok_or(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::GetMediumPathTimeout,
                reply: RnsRpcScalarReply::Null,
            })?;
        duration_from_seconds(seconds, RpcVerb::GetMediumPathTimeout)
    }

    pub async fn packet_phy(
        &self,
        packet_hash: PacketHash,
    ) -> Result<SharedInstancePacketPhyStats, SharedInstanceRpcClientError> {
        Ok(SharedInstancePacketPhyStats {
            rssi_dbm: self
                .numeric(
                    RnsRpcRequest::PacketRssi {
                        packet_hash: packet_hash_argument(packet_hash),
                    },
                    RpcVerb::GetPacketRssi,
                )
                .await?,
            snr_db: self
                .numeric(
                    RnsRpcRequest::PacketSnr {
                        packet_hash: packet_hash_argument(packet_hash),
                    },
                    RpcVerb::GetPacketSnr,
                )
                .await?,
            quality_percent: self
                .numeric(
                    RnsRpcRequest::PacketQuality {
                        packet_hash: packet_hash_argument(packet_hash),
                    },
                    RpcVerb::GetPacketQuality,
                )
                .await?,
        })
    }

    pub async fn drop_path(
        &self,
        destination: DestinationHash,
    ) -> Result<bool, SharedInstanceRpcClientError> {
        let reply = self
            .scalar(
                RnsRpcRequest::DropPath {
                    destination_hash: destination,
                },
                RpcVerb::DropPath,
            )
            .await?;
        match reply {
            RnsRpcScalarReply::Boolean(dropped) => Ok(dropped),
            reply => Err(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::DropPath,
                reply,
            }),
        }
    }

    pub async fn drop_all_via(
        &self,
        transport: TransportId,
    ) -> Result<u64, SharedInstanceRpcClientError> {
        let reply = self
            .scalar(
                RnsRpcRequest::DropAllVia {
                    transport_id: transport,
                },
                RpcVerb::DropAllVia,
            )
            .await?;
        reply
            .nonnegative_integer()
            .ok_or(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::DropAllVia,
                reply,
            })
    }

    pub async fn drop_announce_queues(&self) -> Result<(), SharedInstanceRpcClientError> {
        let reply = self
            .scalar(
                RnsRpcRequest::DropAnnounceQueues,
                RpcVerb::DropAnnounceQueues,
            )
            .await?;
        match reply {
            RnsRpcScalarReply::Null => Ok(()),
            reply => Err(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::DropAnnounceQueues,
                reply,
            }),
        }
    }

    pub async fn blackhole_identity(
        &self,
        identity: IdentityHash,
        until: Option<RnsNumber>,
        reason: Option<String>,
    ) -> Result<SharedInstanceBlackholeOutcome, SharedInstanceRpcClientError> {
        let reply = self
            .scalar(
                RnsRpcRequest::BlackholeIdentity {
                    identity_hash: identity,
                    until,
                    reason,
                },
                RpcVerb::BlackholeIdentity,
            )
            .await?;
        match reply {
            RnsRpcScalarReply::Boolean(true) => Ok(SharedInstanceBlackholeOutcome::Added),
            RnsRpcScalarReply::Null => Ok(SharedInstanceBlackholeOutcome::AlreadyPresent),
            RnsRpcScalarReply::Boolean(false) => Ok(SharedInstanceBlackholeOutcome::Rejected),
            reply => Err(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::BlackholeIdentity,
                reply,
            }),
        }
    }

    pub async fn unblackhole_identity(
        &self,
        identity: IdentityHash,
    ) -> Result<SharedInstanceUnblackholeOutcome, SharedInstanceRpcClientError> {
        let reply = self
            .scalar(
                RnsRpcRequest::UnblackholeIdentity {
                    identity_hash: identity,
                },
                RpcVerb::UnblackholeIdentity,
            )
            .await?;
        match reply {
            RnsRpcScalarReply::Boolean(true) => Ok(SharedInstanceUnblackholeOutcome::Removed),
            RnsRpcScalarReply::Null => Ok(SharedInstanceUnblackholeOutcome::NotFound),
            RnsRpcScalarReply::Boolean(false) => Ok(SharedInstanceUnblackholeOutcome::Rejected),
            reply => Err(SharedInstanceRpcClientError::UnexpectedScalarReply {
                operation: RpcVerb::UnblackholeIdentity,
                reply,
            }),
        }
    }

    async fn scalar(
        &self,
        request: RnsRpcRequest,
        operation: RpcVerb,
    ) -> Result<RnsRpcScalarReply, SharedInstanceRpcClientError> {
        let reply = self.exchange(request).await?;
        RnsRpcScalarReply::decode_message_pack(&reply)
            .map_err(|source| SharedInstanceRpcClientError::ScalarReply { operation, source })
    }

    async fn numeric(
        &self,
        request: RnsRpcRequest,
        operation: RpcVerb,
    ) -> Result<Option<f64>, SharedInstanceRpcClientError> {
        let reply = self.scalar(request, operation).await?;
        match reply {
            RnsRpcScalarReply::Null => Ok(None),
            RnsRpcScalarReply::Integer(value) => value
                .signed_value()
                .map(|value| Some(value as f64))
                .ok_or(SharedInstanceRpcClientError::UnexpectedScalarReply {
                    operation,
                    reply: RnsRpcScalarReply::Integer(value),
                }),
            RnsRpcScalarReply::Float(value) if value.is_finite() => Ok(Some(value)),
            reply => Err(SharedInstanceRpcClientError::UnexpectedScalarReply { operation, reply }),
        }
    }

    pub async fn exchange(
        &self,
        request: RnsRpcRequest,
    ) -> Result<Vec<u8>, SharedInstanceRpcClientError> {
        tokio::time::timeout(self.timeout, self.exchange_negotiated(&request))
            .await
            .map_err(|_| SharedInstanceRpcClientError::TimedOut {
                endpoint: self.endpoint.clone(),
                timeout: self.timeout,
            })?
    }

    async fn exchange_without_timeout(
        &self,
        request: &RnsRpcRequest,
        dialect: RpcDialect,
    ) -> Result<Vec<u8>, SharedInstanceRpcClientError> {
        match &self.endpoint {
            SharedInstanceRpcEndpoint::Tcp { control_port } => {
                let address = ("127.0.0.1", *control_port);
                let mut stream = TcpStream::connect(address)
                    .await
                    .map_err(|error| self.io_error(SharedInstanceRpcClientPhase::Connect, error))?;
                self.exchange_stream_with_dialect(&mut stream, request, dialect)
                    .await
            }
            #[cfg(any(target_os = "linux", target_os = "android"))]
            SharedInstanceRpcEndpoint::AbstractUnix { socket_path } => {
                let mut stream = connect_abstract_rpc(socket_path)
                    .map_err(|error| self.io_error(SharedInstanceRpcClientPhase::Connect, error))?;
                self.exchange_stream_with_dialect(&mut stream, request, dialect)
                    .await
            }
        }
    }

    async fn exchange_negotiated(
        &self,
        request: &RnsRpcRequest,
    ) -> Result<Vec<u8>, SharedInstanceRpcClientError> {
        match self.cached_dialect() {
            Some(dialect) => self.exchange_without_timeout(request, dialect).await,
            None => {
                // Never discover the dialect by replaying the caller's
                // operation. RNS 1.3.3 can leave a rejected MessagePack
                // connection open, so use a short, read-only probe and drop
                // that connection before retrying pickle.
                let (probe_reply, dialect) = self.negotiate().await?;
                if matches!(request, RnsRpcRequest::LinkCount) {
                    Ok(probe_reply)
                } else {
                    self.exchange_without_timeout(request, dialect).await
                }
            }
        }
    }

    async fn negotiate(&self) -> Result<(Vec<u8>, RpcDialect), SharedInstanceRpcClientError> {
        let probe = RnsRpcRequest::LinkCount;
        match tokio::time::timeout(
            DIALECT_PROBE_TIMEOUT,
            self.exchange_without_timeout(&probe, RpcDialect::Msgpack),
        )
        .await
        {
            Ok(Ok(reply)) => {
                self.cache_dialect(RpcDialect::Msgpack);
                Ok((reply, RpcDialect::Msgpack))
            }
            Ok(Err(error)) if dialect_fallback_candidate(&error) => {
                let reply = self
                    .exchange_without_timeout(&probe, RpcDialect::Pickle)
                    .await?;
                self.cache_dialect(RpcDialect::Pickle);
                Ok((reply, RpcDialect::Pickle))
            }
            Err(_) => {
                let reply = self
                    .exchange_without_timeout(&probe, RpcDialect::Pickle)
                    .await?;
                self.cache_dialect(RpcDialect::Pickle);
                Ok((reply, RpcDialect::Pickle))
            }
            Ok(Err(error)) => Err(error),
        }
    }

    fn cached_dialect(&self) -> Option<RpcDialect> {
        match self.dialect.load(Ordering::Relaxed) {
            DIALECT_MSGPACK => Some(RpcDialect::Msgpack),
            DIALECT_PICKLE => Some(RpcDialect::Pickle),
            _ => None,
        }
    }

    fn cache_dialect(&self, dialect: RpcDialect) {
        self.dialect.store(
            match dialect {
                RpcDialect::Msgpack => DIALECT_MSGPACK,
                RpcDialect::Pickle => DIALECT_PICKLE,
            },
            Ordering::Relaxed,
        );
    }

    #[cfg(test)]
    async fn exchange_stream<S>(
        &self,
        stream: &mut S,
        request: RnsRpcRequest,
    ) -> Result<Vec<u8>, SharedInstanceRpcClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.exchange_stream_with_dialect(stream, &request, RpcDialect::Msgpack)
            .await
    }

    async fn exchange_stream_with_dialect<S>(
        &self,
        stream: &mut S,
        request: &RnsRpcRequest,
        dialect: RpcDialect,
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
        let request = match dialect {
            RpcDialect::Msgpack => request.encode_message_pack(),
            RpcDialect::Pickle => request.encode_pickle(),
        }
        .map_err(|_| SharedInstanceRpcClientError::RequestEncode)?;
        write_frame(stream, &request)
            .await
            .map_err(|error| self.io_error(SharedInstanceRpcClientPhase::WriteRequest, error))?;
        let reply = read_frame(stream)
            .await
            .map_err(|error| self.io_error(SharedInstanceRpcClientPhase::ReadReply, error))?;
        match dialect {
            RpcDialect::Msgpack => Ok(reply),
            RpcDialect::Pickle => legacy::decode_reply(&reply)
                .map_err(|_| SharedInstanceRpcClientError::LegacyReplyDecode),
        }
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

fn duration_from_seconds(
    seconds: f64,
    operation: RpcVerb,
) -> Result<Duration, SharedInstanceRpcClientError> {
    if seconds < 0.0 || !seconds.is_finite() {
        return Err(SharedInstanceRpcClientError::UnexpectedScalarReply {
            operation,
            reply: RnsRpcScalarReply::Float(seconds),
        });
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn dialect_fallback_candidate(error: &SharedInstanceRpcClientError) -> bool {
    matches!(
        error,
        SharedInstanceRpcClientError::Io {
            phase: SharedInstanceRpcClientPhase::WriteRequest
                | SharedInstanceRpcClientPhase::ReadReply,
            ..
        }
    )
}

fn packet_hash_argument(packet_hash: PacketHash) -> PacketHashArgument {
    PacketHashArgument::new(packet_hash.as_bytes().to_vec())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn connect_abstract_rpc(socket_path: &str) -> std::io::Result<tokio::net::UnixStream> {
    #[cfg(target_os = "android")]
    use std::os::android::net::SocketAddrExt;
    #[cfg(target_os = "linux")]
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}/rpc");
    let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
    let stream = std::os::unix::net::UnixStream::connect_addr(&address)?;
    stream.set_nonblocking(true)?;
    tokio::net::UnixStream::from_std(stream)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::vec;

    use prns_core::identity::IdentityHash;
    use prns_core::interfaces::shared_instance::rns_rpc::{
        RpcAuthenticationKey, RpcRequest, RpcVerb,
    };
    use serde_pickle::{HashableValue, Value};
    use tokio::net::TcpListener;

    use super::*;
    use crate::shared_instance::rns_rpc::authentication::{
        answer_client_challenge, deliver_our_challenge, SharedInstanceCredentials,
    };
    use crate::shared_instance::rns_rpc::framing::{read_frame, write_frame};

    fn credentials(key: u8) -> SharedInstanceCredentials {
        SharedInstanceCredentials::new(
            RpcAuthenticationKey::new(vec![key; 32]),
            IdentityHash::new([0x42; 16]),
        )
    }

    #[test]
    fn fractional_timeout_seconds_decode_without_truncation() {
        assert_eq!(
            duration_from_seconds(18.013, RpcVerb::GetFirstHopTimeout),
            Ok(Duration::from_millis(18_013)),
        );
        assert!(duration_from_seconds(-0.001, RpcVerb::GetMediumPathTimeout).is_err());
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

    async fn accept_authenticated_request(
        listener: &TcpListener,
        credentials: &SharedInstanceCredentials,
    ) -> (TcpStream, Vec<u8>) {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert!(deliver_our_challenge(&mut stream, credentials.rpc_key())
            .await
            .unwrap());
        assert!(answer_client_challenge(&mut stream, credentials.rpc_key())
            .await
            .unwrap());
        let request = read_frame(&mut stream).await.unwrap();
        (stream, request)
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

    #[tokio::test]
    async fn negotiates_pickle_when_a_legacy_peer_keeps_the_rejected_msgpack_connection_open() {
        let credentials = credentials(0x31);
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, request) = accept_authenticated_request(&listener, &credentials).await;
            assert_eq!(
                RpcRequest::decode(&request),
                Ok(RpcRequest::Msgpack(RnsRpcRequest::LinkCount))
            );
            let _held_rejected_connection = tokio::spawn(async move {
                tokio::time::sleep(DIALECT_PROBE_TIMEOUT.saturating_mul(4)).await;
                drop(stream);
            });

            let (mut stream, request) = accept_authenticated_request(&listener, &credentials).await;
            let decoded = RpcRequest::decode(&request).unwrap();
            assert_eq!(decoded.dialect(), RpcDialect::Pickle);
            assert_eq!(decoded.verb(), RpcVerb::GetLinkCount);
            write_frame(&mut stream, b"I3\n.").await.unwrap();

            let (mut stream, request) = accept_authenticated_request(&listener, &credentials).await;
            let decoded = RpcRequest::decode(&request).unwrap();
            assert_eq!(decoded.dialect(), RpcDialect::Pickle);
            assert_eq!(decoded.verb(), RpcVerb::GetLinkCount);
            write_frame(&mut stream, b"I4\n.").await.unwrap();
        });
        let client = SharedInstanceRpcClient::new(
            SharedInstanceRpcEndpoint::tcp(port),
            RpcAuthenticationKey::new(vec![0x31; 32]),
            Duration::from_secs(2),
        );

        assert_eq!(client.link_count().await, Ok(3));
        assert_eq!(client.link_count().await, Ok(4));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn mutation_negotiation_uses_a_read_only_probe_and_sends_the_mutation_once() {
        let credentials = credentials(0x32);
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let destination = DestinationHash::new([0x52; 16]);
        let server = tokio::spawn(async move {
            let (stream, request) = accept_authenticated_request(&listener, &credentials).await;
            assert_eq!(
                RpcRequest::decode(&request),
                Ok(RpcRequest::Msgpack(RnsRpcRequest::LinkCount))
            );
            drop(stream);

            let (mut stream, request) = accept_authenticated_request(&listener, &credentials).await;
            let decoded = RpcRequest::decode(&request).unwrap();
            assert_eq!(decoded.dialect(), RpcDialect::Pickle);
            assert_eq!(decoded.verb(), RpcVerb::GetLinkCount);
            write_frame(&mut stream, b"I0\n.").await.unwrap();

            let (mut stream, request) = accept_authenticated_request(&listener, &credentials).await;
            let decoded = RpcRequest::decode(&request).unwrap();
            assert_eq!(decoded.dialect(), RpcDialect::Pickle);
            assert_eq!(decoded.verb(), RpcVerb::DropPath);
            assert_eq!(decoded.legacy_destination_hash(), Some(destination));
            let Value::Dict(fields) =
                serde_pickle::value_from_slice(&request, serde_pickle::DeOptions::new()).unwrap()
            else {
                panic!("legacy request must be a dictionary");
            };
            assert_eq!(
                fields,
                BTreeMap::from([
                    (
                        HashableValue::String(String::from("destination_hash")),
                        Value::Bytes(vec![0x52; 16]),
                    ),
                    (
                        HashableValue::String(String::from("drop")),
                        Value::String(String::from("path")),
                    ),
                ])
            );
            write_frame(&mut stream, b"I01\n.").await.unwrap();
        });
        let client = SharedInstanceRpcClient::new(
            SharedInstanceRpcEndpoint::tcp(port),
            RpcAuthenticationKey::new(vec![0x32; 32]),
            Duration::from_secs(2),
        );

        assert_eq!(client.drop_path(destination).await, Ok(true));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn authentication_rejection_does_not_trigger_a_legacy_retry() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(
                !deliver_our_challenge(&mut stream, credentials(0x42).rpc_key())
                    .await
                    .unwrap()
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let client = SharedInstanceRpcClient::new(
            SharedInstanceRpcEndpoint::tcp(port),
            RpcAuthenticationKey::new(vec![0x41; 32]),
            Duration::from_secs(1),
        );

        assert_eq!(
            client.link_count().await,
            Err(SharedInstanceRpcClientError::CredentialsRejected)
        );
        server.await.unwrap();
    }
}
