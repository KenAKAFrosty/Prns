use super::request::{self, RnsRpcRequest};

/// The wire codec a client's RPC payload speaks. RNS through 1.3.x carried the request and reply as `multiprocessing.connection`'s pickle (`connection.send`/`recv`); RNS 1.3.5 frames msgpack (`send_bytes(mp.packb(..))` / `mp.unpackb(recv_bytes())`). Both share the same length-prefixed framing and the same auth handshake — only the payload codec differs, so the reply must answer in the dialect the request arrived in or the client mis-decodes it (a pickle `None` reads back as the msgpack integer 78, which a client indexing the result then faults on).
#[derive(Clone, Copy)]
pub(super) enum RpcDialect {
    Pickle,
    Msgpack,
}

#[cfg(feature = "tracing")]
impl RpcDialect {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Pickle => "pickle",
            Self::Msgpack => "msgpack",
        }
    }
}

/// Tell the dialects apart by the request's first byte: every RNS RPC request is a small map, so a msgpack request opens with a fixmap tag (`0x81..=0x8f`), while a pickle stream opens with the PROTO opcode `0x80` (or a protocol-0 opcode) — never `0x81..=0x8f`.
fn dialect_of(request: &[u8]) -> RpcDialect {
    match request.first() {
        Some(0x81..=0x8f | 0xde | 0xdf) => RpcDialect::Msgpack,
        _ => RpcDialect::Pickle,
    }
}

#[derive(Clone, Copy)]
pub(super) enum RpcVerb {
    InterfaceStats,
    PathTable,
    RateTable,
    LinkCount,
    NextHop,
    NextHopIfName,
    FirstHopTimeout,
    PacketRssi,
    PacketSnr,
    PacketQuality,
    BlackholedIdentities,
    IsBlackholed,
    DropPath,
    DropAllVia,
    DropAnnounceQueues,
    BlackholeIdentity,
    UnblackholeIdentity,
    DestinationData,
    IdentityData,
    Unknown,
}

#[cfg(feature = "tracing")]
impl RpcVerb {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::InterfaceStats => "interface_stats",
            Self::PathTable => "path_table",
            Self::RateTable => "rate_table",
            Self::LinkCount => "link_count",
            Self::NextHop => "next_hop",
            Self::NextHopIfName => "next_hop_if_name",
            Self::FirstHopTimeout => "first_hop_timeout",
            Self::PacketRssi => "packet_rssi",
            Self::PacketSnr => "packet_snr",
            Self::PacketQuality => "packet_q",
            Self::BlackholedIdentities => "blackholed_identities",
            Self::IsBlackholed => "is_blackholed",
            Self::DropPath => "drop_path",
            Self::DropAllVia => "drop_all_via",
            Self::DropAnnounceQueues => "drop_announce_queues",
            Self::BlackholeIdentity => "blackhole_identity",
            Self::UnblackholeIdentity => "unblackhole_identity",
            Self::DestinationData => "destination_data",
            Self::IdentityData => "identity_data",
            Self::Unknown => "unknown",
        }
    }
}

pub(super) enum RpcRequest<'a> {
    Pickle(&'a [u8]),
    Msgpack(RnsRpcRequest),
}

impl<'a> RpcRequest<'a> {
    pub(super) fn decode(bytes: &'a [u8]) -> Result<Self, request::DecodeError> {
        match dialect_of(bytes) {
            RpcDialect::Pickle => Ok(Self::Pickle(bytes)),
            RpcDialect::Msgpack => request::decode(bytes).map(Self::Msgpack),
        }
    }

    pub(super) fn dialect(&self) -> RpcDialect {
        match self {
            Self::Pickle(_) => RpcDialect::Pickle,
            Self::Msgpack(_) => RpcDialect::Msgpack,
        }
    }

    pub(super) fn verb(&self) -> RpcVerb {
        match self {
            Self::Pickle(bytes) => classify_pickle_rpc_verb(bytes),
            Self::Msgpack(request) => match request {
                RnsRpcRequest::InterfaceStats => RpcVerb::InterfaceStats,
                RnsRpcRequest::PathTable { .. } => RpcVerb::PathTable,
                RnsRpcRequest::RateTable => RpcVerb::RateTable,
                RnsRpcRequest::LinkCount => RpcVerb::LinkCount,
                RnsRpcRequest::NextHop { .. } => RpcVerb::NextHop,
                RnsRpcRequest::NextHopInterface { .. } => RpcVerb::NextHopIfName,
                RnsRpcRequest::FirstHopTimeout { .. } => RpcVerb::FirstHopTimeout,
                RnsRpcRequest::PacketRssi { .. } => RpcVerb::PacketRssi,
                RnsRpcRequest::PacketSnr { .. } => RpcVerb::PacketSnr,
                RnsRpcRequest::PacketQuality { .. } => RpcVerb::PacketQuality,
                RnsRpcRequest::BlackholedIdentities => RpcVerb::BlackholedIdentities,
                RnsRpcRequest::IsBlackholed { .. } => RpcVerb::IsBlackholed,
                RnsRpcRequest::DropPath { .. } => RpcVerb::DropPath,
                RnsRpcRequest::DropAllVia { .. } => RpcVerb::DropAllVia,
                RnsRpcRequest::DropAnnounceQueues => RpcVerb::DropAnnounceQueues,
                RnsRpcRequest::BlackholeIdentity { .. } => RpcVerb::BlackholeIdentity,
                RnsRpcRequest::UnblackholeIdentity { .. } => RpcVerb::UnblackholeIdentity,
                RnsRpcRequest::DestinationData { .. } => RpcVerb::DestinationData,
                RnsRpcRequest::RetainIdentity { .. } => RpcVerb::IdentityData,
            },
        }
    }
}

fn classify_pickle_rpc_verb(request: &[u8]) -> RpcVerb {
    if contains(request, b"interface_stats") {
        RpcVerb::InterfaceStats
    } else if contains(request, b"rate_table") {
        RpcVerb::RateTable
    } else if contains(request, b"blackholed_identities") {
        RpcVerb::BlackholedIdentities
    } else if contains(request, b"is_blackholed") {
        RpcVerb::IsBlackholed
    } else if contains(request, b"path_table") {
        RpcVerb::PathTable
    } else if contains(request, b"next_hop_if_name") {
        RpcVerb::NextHopIfName
    } else if contains(request, b"next_hop") {
        RpcVerb::NextHop
    } else if contains(request, b"first_hop_timeout") {
        RpcVerb::FirstHopTimeout
    } else if contains(request, b"link_count") {
        RpcVerb::LinkCount
    } else if contains(request, b"packet_rssi") {
        RpcVerb::PacketRssi
    } else if contains(request, b"packet_snr") {
        RpcVerb::PacketSnr
    } else if contains(request, b"packet_q") {
        RpcVerb::PacketQuality
    } else if contains(request, b"drop") && contains(request, b"announce_queues") {
        RpcVerb::DropAnnounceQueues
    } else if contains(request, b"drop") && contains(request, b"all_via") {
        RpcVerb::DropAllVia
    } else if contains(request, b"drop") && contains(request, b"path") {
        RpcVerb::DropPath
    } else if contains(request, b"unblackhole_identity") {
        RpcVerb::UnblackholeIdentity
    } else if contains(request, b"blackhole_identity") {
        RpcVerb::BlackholeIdentity
    } else if contains(request, b"destination_data") {
        RpcVerb::DestinationData
    } else if contains(request, b"identity_data") {
        RpcVerb::IdentityData
    } else {
        RpcVerb::Unknown
    }
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    position_of(haystack, needle).is_some()
}

pub(super) fn position_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
