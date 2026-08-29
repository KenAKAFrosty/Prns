use alloc::vec::Vec;

use crate::wire::DestinationHash;

use super::RnsRpcReply;
use crate::interfaces::shared_instance::rns_rpc::RpcVerb;

pub enum LegacyRpcReplyPlan {
    InterfaceStats,
    PathTable,
    NextHopInterfaceName(DestinationHash),
    NextHop(DestinationHash),
    LinkCount,
    FirstHopTimeout(DestinationHash),
    LowestInterfaceBitrate,
    MediumPathTimeout,
    Immediate(RnsRpcReply),
}

impl LegacyRpcReplyPlan {
    pub fn for_request(verb: RpcVerb, destination_hash: Option<DestinationHash>) -> Self {
        match verb {
            RpcVerb::GetInterfaceStats => Self::InterfaceStats,
            RpcVerb::GetPathTable => Self::PathTable,
            RpcVerb::GetRateTable => Self::Immediate(RnsRpcReply::announce_rate_table(
                crate::interfaces::rns_management::RnsAnnounceRateTable::new(Vec::new()),
            )),
            RpcVerb::GetLinkCount => Self::LinkCount,
            RpcVerb::GetNextHop => match destination_hash {
                Some(destination_hash) => Self::NextHop(destination_hash),
                None => Self::Immediate(RnsRpcReply::next_hop(None)),
            },
            RpcVerb::GetNextHopInterfaceName => match destination_hash {
                Some(destination_hash) => Self::NextHopInterfaceName(destination_hash),
                None => Self::Immediate(RnsRpcReply::next_hop_interface_name(None)),
            },
            RpcVerb::GetFirstHopTimeout => match destination_hash {
                Some(destination_hash) => Self::FirstHopTimeout(destination_hash),
                None => Self::Immediate(RnsRpcReply::first_hop_timeout()),
            },
            RpcVerb::GetLowestInterfaceBitrate => Self::LowestInterfaceBitrate,
            RpcVerb::GetMediumPathTimeout => Self::MediumPathTimeout,
            RpcVerb::GetPacketRssi
            | RpcVerb::GetPacketSnr
            | RpcVerb::GetPacketQuality
            | RpcVerb::Unknown => Self::Immediate(RnsRpcReply::none()),
            RpcVerb::GetBlackholedIdentities => {
                Self::Immediate(RnsRpcReply::empty_blackhole_table())
            }
            RpcVerb::CheckIdentityBlackholed => Self::Immediate(RnsRpcReply::boolean(false)),
            RpcVerb::DropPath
            | RpcVerb::BlackholeIdentity
            | RpcVerb::UnblackholeIdentity
            | RpcVerb::UpdateDestinationData
            | RpcVerb::RetainIdentity => Self::Immediate(RnsRpcReply::boolean(false)),
            RpcVerb::DropAllVia => Self::Immediate(RnsRpcReply::integer(0)),
            RpcVerb::DropAnnounceQueues => Self::Immediate(RnsRpcReply::drop_announce_queues()),
        }
    }
}
