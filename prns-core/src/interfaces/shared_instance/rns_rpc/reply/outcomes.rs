use crate::engine::{DropRouteOutcome, DropRoutesViaOutcome};
use crate::identity::{
    MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome,
};
use crate::interfaces::BitrateBps;
use crate::interfaces::PacketPhyStats;
use crate::routing::timing::DEFAULT_FIRST_HOP_TIMEOUT_MS;
use crate::routing::{BlackholeIdentityOutcome, BlackholedIdentity, UnblackholeIdentityOutcome};

use super::RnsRpcReply;

pub enum RpcOperationOutcome<T> {
    Completed(T),
    Unavailable,
}

impl RnsRpcReply {
    pub fn first_hop_timeout() -> Self {
        Self::timeout_millis(DEFAULT_FIRST_HOP_TIMEOUT_MS)
    }

    pub fn timeout_millis(milliseconds: u64) -> Self {
        if milliseconds.is_multiple_of(1_000) {
            Self::integer(i64::try_from(milliseconds / 1_000).unwrap_or(i64::MAX))
        } else {
            Self::float(milliseconds as f64 / 1_000.0)
        }
    }

    pub fn lowest_interface_bitrate(bitrate: Option<BitrateBps>) -> Self {
        match bitrate {
            Some(bitrate) => Self::integer(i64::try_from(bitrate.get()).unwrap_or(i64::MAX)),
            None => Self::none(),
        }
    }

    pub fn packet_rssi(stats: Option<PacketPhyStats>) -> Self {
        match stats.and_then(|stats| stats.rssi) {
            Some(rssi) => Self::integer(i64::from(rssi.get())),
            None => Self::none(),
        }
    }

    pub fn packet_snr(stats: Option<PacketPhyStats>) -> Self {
        match stats.and_then(|stats| stats.snr) {
            Some(snr) => Self::float(f64::from(snr.quarters()) / 4.0),
            None => Self::none(),
        }
    }

    pub fn packet_quality(stats: Option<PacketPhyStats>) -> Self {
        match stats.and_then(|stats| stats.quality) {
            Some(quality) => Self::float(f64::from(quality.tenths_percent()) / 10.0),
            None => Self::none(),
        }
    }

    pub fn blackholed_identities<Reason, Entries>(outcome: RpcOperationOutcome<Entries>) -> Self
    where
        Reason: AsRef<str>,
        Entries: IntoIterator<Item = BlackholedIdentity<Reason>>,
    {
        match outcome {
            RpcOperationOutcome::Completed(entries) => Self::blackhole_table(entries),
            RpcOperationOutcome::Unavailable => Self::empty_blackhole_table(),
        }
    }

    pub const fn is_blackholed(outcome: RpcOperationOutcome<bool>) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(blackholed) => Self::boolean(blackholed),
            RpcOperationOutcome::Unavailable => Self::boolean(false),
        }
    }

    pub const fn drop_path(outcome: RpcOperationOutcome<DropRouteOutcome>) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(DropRouteOutcome::Dropped) => Self::boolean(true),
            RpcOperationOutcome::Completed(DropRouteOutcome::NotFound)
            | RpcOperationOutcome::Unavailable => Self::boolean(false),
        }
    }

    pub fn drop_all_via(outcome: RpcOperationOutcome<DropRoutesViaOutcome>) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(outcome) => {
                Self::integer(i64::from(outcome.dropped_routes))
            }
            RpcOperationOutcome::Unavailable => Self::integer(0),
        }
    }

    pub const fn drop_announce_queues() -> Self {
        Self::none()
    }

    pub const fn blackhole_identity(
        outcome: RpcOperationOutcome<BlackholeIdentityOutcome>,
    ) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(BlackholeIdentityOutcome::Added) => Self::boolean(true),
            RpcOperationOutcome::Completed(BlackholeIdentityOutcome::AlreadyPresent) => {
                Self::none()
            }
            RpcOperationOutcome::Unavailable => Self::boolean(false),
        }
    }

    pub const fn unblackhole_identity(
        outcome: RpcOperationOutcome<UnblackholeIdentityOutcome>,
    ) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(UnblackholeIdentityOutcome::Removed) => {
                Self::boolean(true)
            }
            RpcOperationOutcome::Completed(UnblackholeIdentityOutcome::NotFound) => Self::none(),
            RpcOperationOutcome::Unavailable => Self::boolean(false),
        }
    }

    pub const fn mark_destination_used(
        outcome: RpcOperationOutcome<MarkDestinationUsedOutcome>,
    ) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(
                MarkDestinationUsedOutcome::Recorded | MarkDestinationUsedOutcome::Refreshed,
            ) => Self::boolean(true),
            RpcOperationOutcome::Completed(
                MarkDestinationUsedOutcome::Retained | MarkDestinationUsedOutcome::NotFound,
            )
            | RpcOperationOutcome::Unavailable => Self::boolean(false),
        }
    }

    pub const fn retain_destination(
        outcome: RpcOperationOutcome<RetainDestinationOutcome>,
    ) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(
                RetainDestinationOutcome::Retained | RetainDestinationOutcome::AlreadyRetained,
            ) => Self::boolean(true),
            RpcOperationOutcome::Completed(RetainDestinationOutcome::NotFound)
            | RpcOperationOutcome::Unavailable => Self::boolean(false),
        }
    }

    pub const fn release_destination(
        outcome: RpcOperationOutcome<ReleaseDestinationOutcome>,
    ) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(
                ReleaseDestinationOutcome::Released
                | ReleaseDestinationOutcome::UseRecorded
                | ReleaseDestinationOutcome::UseRefreshed,
            ) => Self::boolean(true),
            RpcOperationOutcome::Completed(ReleaseDestinationOutcome::NotFound)
            | RpcOperationOutcome::Unavailable => Self::boolean(false),
        }
    }

    pub const fn retain_identity(outcome: RpcOperationOutcome<RetainIdentityOutcome>) -> Self {
        match outcome {
            RpcOperationOutcome::Completed(outcome)
                if outcome.newly_retained_destination_count != 0
                    || outcome.already_retained_destination_count != 0 =>
            {
                Self::boolean(true)
            }
            RpcOperationOutcome::Completed(_) | RpcOperationOutcome::Unavailable => {
                Self::boolean(false)
            }
        }
    }
}
