use super::request::{self, RnsRpcRequest, RpcRequestDecodeError};
use super::wire_names::{argument, dialect, drop_operation, get, selector, verb};
use crate::wire::DestinationHash;

/// The wire codec a client's RPC payload speaks. RNS through 1.3.3 carried the
/// request and reply as `multiprocessing.connection`'s pickle
/// (`connection.send`/`recv`); RNS 1.3.4 and later frame msgpack
/// (`send_bytes(mp.packb(..))` / `mp.unpackb(recv_bytes())`). Both share the
/// same length-prefixed framing and the same auth handshake — only the payload
/// codec differs, so the reply must answer in the dialect the request arrived
/// in or the client mis-decodes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcDialect {
    Pickle,
    Msgpack,
}

impl RpcDialect {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pickle => dialect::PICKLE,
            Self::Msgpack => dialect::MESSAGE_PACK,
        }
    }
}

/// Tell the dialects apart by the request's first byte: every RNS RPC request is a small map, so a msgpack request opens with a fixmap tag (`0x81..=0x8f`), while a pickle stream opens with the PROTO opcode `0x80` (or a protocol-0 opcode) — never `0x81..=0x8f`.
fn dialect_of(request: &[u8]) -> RpcDialect {
    let binary_pickle = matches!(request, [0x80, 0x02..=0x05, ..]);
    let text_pickle =
        matches!(request.first(), Some(b'(' | b'd' | b'}')) && request.last() == Some(&b'.');
    if binary_pickle || text_pickle {
        RpcDialect::Pickle
    } else {
        RpcDialect::Msgpack
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcVerb {
    GetInterfaceStats,
    GetPathTable,
    GetRateTable,
    GetLinkCount,
    GetNextHop,
    GetNextHopInterfaceName,
    GetFirstHopTimeout,
    GetLowestInterfaceBitrate,
    GetMediumPathTimeout,
    GetPacketRssi,
    GetPacketSnr,
    GetPacketQuality,
    GetBlackholedIdentities,
    CheckIdentityBlackholed,
    DropPath,
    DropAllVia,
    DropAnnounceQueues,
    BlackholeIdentity,
    UnblackholeIdentity,
    UpdateDestinationData,
    RetainIdentity,
    Unknown,
}

impl RpcVerb {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GetInterfaceStats => verb::GET_INTERFACE_STATS,
            Self::GetPathTable => verb::GET_PATH_TABLE,
            Self::GetRateTable => verb::GET_RATE_TABLE,
            Self::GetLinkCount => verb::GET_LINK_COUNT,
            Self::GetNextHop => verb::GET_NEXT_HOP,
            Self::GetNextHopInterfaceName => verb::GET_NEXT_HOP_INTERFACE_NAME,
            Self::GetFirstHopTimeout => verb::GET_FIRST_HOP_TIMEOUT,
            Self::GetLowestInterfaceBitrate => verb::GET_LOWEST_INTERFACE_BITRATE,
            Self::GetMediumPathTimeout => verb::GET_MEDIUM_PATH_TIMEOUT,
            Self::GetPacketRssi => verb::GET_PACKET_RSSI,
            Self::GetPacketSnr => verb::GET_PACKET_SNR,
            Self::GetPacketQuality => verb::GET_PACKET_QUALITY,
            Self::GetBlackholedIdentities => verb::GET_BLACKHOLED_IDENTITIES,
            Self::CheckIdentityBlackholed => verb::CHECK_IDENTITY_BLACKHOLED,
            Self::DropPath => verb::DROP_PATH,
            Self::DropAllVia => verb::DROP_ALL_VIA,
            Self::DropAnnounceQueues => verb::DROP_ANNOUNCE_QUEUES,
            Self::BlackholeIdentity => verb::BLACKHOLE_IDENTITY,
            Self::UnblackholeIdentity => verb::UNBLACKHOLE_IDENTITY,
            Self::UpdateDestinationData => verb::UPDATE_DESTINATION_DATA,
            Self::RetainIdentity => verb::RETAIN_IDENTITY,
            Self::Unknown => verb::UNKNOWN,
        }
    }
}

impl RnsRpcRequest {
    pub const fn verb(&self) -> RpcVerb {
        match self {
            Self::InterfaceStats => RpcVerb::GetInterfaceStats,
            Self::PathTable { .. } => RpcVerb::GetPathTable,
            Self::RateTable => RpcVerb::GetRateTable,
            Self::LinkCount => RpcVerb::GetLinkCount,
            Self::NextHop { .. } => RpcVerb::GetNextHop,
            Self::NextHopInterface { .. } => RpcVerb::GetNextHopInterfaceName,
            Self::FirstHopTimeout { .. } => RpcVerb::GetFirstHopTimeout,
            Self::LowestInterfaceBitrate => RpcVerb::GetLowestInterfaceBitrate,
            Self::MediumPathTimeout => RpcVerb::GetMediumPathTimeout,
            Self::PacketRssi { .. } => RpcVerb::GetPacketRssi,
            Self::PacketSnr { .. } => RpcVerb::GetPacketSnr,
            Self::PacketQuality { .. } => RpcVerb::GetPacketQuality,
            Self::BlackholedIdentities => RpcVerb::GetBlackholedIdentities,
            Self::IsBlackholed { .. } => RpcVerb::CheckIdentityBlackholed,
            Self::DropPath { .. } => RpcVerb::DropPath,
            Self::DropAllVia { .. } => RpcVerb::DropAllVia,
            Self::DropAnnounceQueues => RpcVerb::DropAnnounceQueues,
            Self::BlackholeIdentity { .. } => RpcVerb::BlackholeIdentity,
            Self::UnblackholeIdentity { .. } => RpcVerb::UnblackholeIdentity,
            Self::DestinationData { .. } => RpcVerb::UpdateDestinationData,
            Self::RetainIdentity { .. } => RpcVerb::RetainIdentity,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum RpcRequest<'a> {
    Pickle(&'a [u8]),
    Msgpack(RnsRpcRequest),
}

impl<'a> RpcRequest<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, RpcRequestDecodeError> {
        match dialect_of(bytes) {
            RpcDialect::Pickle => {
                let verb = classify_pickle_rpc_verb(bytes);
                if verb == RpcVerb::Unknown {
                    Err(RpcRequestDecodeError::UnknownOperation {
                        selector: dialect::PICKLE,
                        operation: verb.as_str().into(),
                    })
                } else {
                    Ok(Self::Pickle(bytes))
                }
            }
            RpcDialect::Msgpack => request::decode(bytes).map(Self::Msgpack),
        }
    }

    pub const fn dialect(&self) -> RpcDialect {
        match self {
            Self::Pickle(_) => RpcDialect::Pickle,
            Self::Msgpack(_) => RpcDialect::Msgpack,
        }
    }

    pub fn verb(&self) -> RpcVerb {
        match self {
            Self::Pickle(bytes) => classify_pickle_rpc_verb(bytes),
            Self::Msgpack(request) => request.verb(),
        }
    }

    pub fn legacy_destination_hash(&self) -> Option<DestinationHash> {
        let Self::Pickle(request) = self else {
            return None;
        };
        let destination_hash = argument::DESTINATION_HASH.as_bytes();
        let key_end = position_of(request, destination_hash)? + destination_hash.len();
        let tail = &request[key_end..];
        let value_start = tail
            .windows(2)
            .position(|window| matches!(window, [0x43, 0x10]))?
            + 2;
        let bytes: [u8; 16] = tail.get(value_start..value_start + 16)?.try_into().ok()?;
        Some(DestinationHash::new(bytes))
    }
}

fn classify_pickle_rpc_verb(request: &[u8]) -> RpcVerb {
    if contains(request, get::INTERFACE_STATS.as_bytes()) {
        RpcVerb::GetInterfaceStats
    } else if contains(request, get::RATE_TABLE.as_bytes()) {
        RpcVerb::GetRateTable
    } else if contains(request, get::BLACKHOLED_IDENTITIES.as_bytes()) {
        RpcVerb::GetBlackholedIdentities
    } else if contains(request, get::IS_BLACKHOLED.as_bytes()) {
        RpcVerb::CheckIdentityBlackholed
    } else if contains(request, get::PATH_TABLE.as_bytes()) {
        RpcVerb::GetPathTable
    } else if contains(request, get::NEXT_HOP_INTERFACE_NAME.as_bytes()) {
        RpcVerb::GetNextHopInterfaceName
    } else if contains(request, get::NEXT_HOP.as_bytes()) {
        RpcVerb::GetNextHop
    } else if contains(request, get::FIRST_HOP_TIMEOUT.as_bytes()) {
        RpcVerb::GetFirstHopTimeout
    } else if contains(request, get::LOWEST_INTERFACE_BITRATE.as_bytes()) {
        RpcVerb::GetLowestInterfaceBitrate
    } else if contains(request, get::MEDIUM_PATH_TIMEOUT.as_bytes()) {
        RpcVerb::GetMediumPathTimeout
    } else if contains(request, get::LINK_COUNT.as_bytes()) {
        RpcVerb::GetLinkCount
    } else if contains(request, get::PACKET_RSSI.as_bytes()) {
        RpcVerb::GetPacketRssi
    } else if contains(request, get::PACKET_SNR.as_bytes()) {
        RpcVerb::GetPacketSnr
    } else if contains(request, get::PACKET_QUALITY.as_bytes()) {
        RpcVerb::GetPacketQuality
    } else if contains(request, selector::DROP.as_bytes())
        && contains(request, drop_operation::ANNOUNCE_QUEUES.as_bytes())
    {
        RpcVerb::DropAnnounceQueues
    } else if contains(request, selector::DROP.as_bytes())
        && contains(request, drop_operation::ALL_VIA.as_bytes())
    {
        RpcVerb::DropAllVia
    } else if contains(request, selector::DROP.as_bytes())
        && contains(request, drop_operation::PATH.as_bytes())
    {
        RpcVerb::DropPath
    } else if contains(request, selector::UNBLACKHOLE_IDENTITY.as_bytes()) {
        RpcVerb::UnblackholeIdentity
    } else if contains(request, selector::BLACKHOLE_IDENTITY.as_bytes()) {
        RpcVerb::BlackholeIdentity
    } else if contains(request, selector::DESTINATION_DATA.as_bytes()) {
        RpcVerb::UpdateDestinationData
    } else if contains(request, selector::IDENTITY_DATA.as_bytes()) {
        RpcVerb::RetainIdentity
    } else {
        RpcVerb::Unknown
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    position_of(haystack, needle).is_some()
}

fn position_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_name_operations_while_preserving_stock_wire_names() {
        assert_eq!(RpcDialect::Pickle.as_str(), "pickle");
        assert_eq!(RpcDialect::Msgpack.as_str(), "msgpack");
        let cases = [
            (RpcVerb::GetInterfaceStats, "interface_stats"),
            (RpcVerb::GetPathTable, "path_table"),
            (RpcVerb::GetRateTable, "rate_table"),
            (RpcVerb::GetLinkCount, "link_count"),
            (RpcVerb::GetNextHop, "next_hop"),
            (RpcVerb::GetNextHopInterfaceName, "next_hop_if_name"),
            (RpcVerb::GetFirstHopTimeout, "first_hop_timeout"),
            (
                RpcVerb::GetLowestInterfaceBitrate,
                "lowest_interface_bitrate",
            ),
            (RpcVerb::GetMediumPathTimeout, "medium_path_timeout"),
            (RpcVerb::GetPacketRssi, "packet_rssi"),
            (RpcVerb::GetPacketSnr, "packet_snr"),
            (RpcVerb::GetPacketQuality, "packet_q"),
            (RpcVerb::GetBlackholedIdentities, "blackholed_identities"),
            (RpcVerb::CheckIdentityBlackholed, "is_blackholed"),
            (RpcVerb::DropPath, "drop_path"),
            (RpcVerb::DropAllVia, "drop_all_via"),
            (RpcVerb::DropAnnounceQueues, "drop_announce_queues"),
            (RpcVerb::BlackholeIdentity, "blackhole_identity"),
            (RpcVerb::UnblackholeIdentity, "unblackhole_identity"),
            (RpcVerb::UpdateDestinationData, "destination_data"),
            (RpcVerb::RetainIdentity, "identity_data"),
            (RpcVerb::Unknown, "unknown"),
        ];

        for (verb, stock_name) in cases {
            assert_eq!(verb.as_str(), stock_name);
        }
    }

    #[test]
    fn legacy_drop_classification_requires_selector_and_operation() {
        assert_eq!(
            classify_pickle_rpc_verb(selector::DROP.as_bytes()),
            RpcVerb::Unknown,
        );
        assert_eq!(
            classify_pickle_rpc_verb(drop_operation::ANNOUNCE_QUEUES.as_bytes()),
            RpcVerb::Unknown,
        );
    }

    #[test]
    fn legacy_pickle_classification_extracts_the_typed_route_argument() {
        let destination = [0x42; 16];
        let request = b"\x80\x04\x95\x3c\x00\x00\x00\x00\x00\x00\x00}\x94(\x8c\x03get\x94\x8c\x08next_hop\x94\x8c\x10destination_hash\x94C\x10BBBBBBBBBBBBBBBB\x94u.";
        let decoded = RpcRequest::decode(request).unwrap();

        assert_eq!(decoded.dialect(), RpcDialect::Pickle);
        assert_eq!(decoded.verb(), RpcVerb::GetNextHop);
        assert_eq!(
            decoded.legacy_destination_hash(),
            Some(DestinationHash::new(destination))
        );
    }

    #[test]
    fn legacy_destination_hash_searches_only_after_the_argument_key() {
        let mut request = b"padding-padding-padding\x43\x10AAAAAAAA".to_vec();
        request.extend_from_slice(argument::DESTINATION_HASH.as_bytes());
        request.extend_from_slice(b"\x43\x10BBBBBBBBBBBBBBBB");
        assert_eq!(
            RpcRequest::Pickle(&request).legacy_destination_hash(),
            Some(DestinationHash::new([b'B'; 16])),
        );
    }

    #[test]
    fn text_pickle_detection_requires_both_its_opening_and_stop_opcode() {
        assert_eq!(dialect_of(b"(dp0\n."), RpcDialect::Pickle);
        assert_eq!(dialect_of(b"(dp0\n"), RpcDialect::Msgpack);
        assert_eq!(dialect_of(b"\x81."), RpcDialect::Msgpack);
    }

    #[test]
    fn malformed_msgpack_and_unknown_pickle_operations_are_rejected() {
        assert_eq!(
            RpcRequest::decode(&[0xc1]),
            Err(RpcRequestDecodeError::MessagePack)
        );
        assert!(matches!(
            RpcRequest::decode(b"(dp0\nVget\np1\nVfuture\np2\ns."),
            Err(RpcRequestDecodeError::UnknownOperation {
                selector: "pickle",
                operation,
            }) if operation == "unknown"
        ));
    }
}
