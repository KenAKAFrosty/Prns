mod announce_completion;
mod deferred_decryption;
mod delivery;
mod held_announce_release;
mod link_handshake_completion;
mod packet_dispatch;
mod relay;

pub use packet_dispatch::{IngestIo, IngestPacketReport};

use crate::engine::Journaled;
use crate::routing::RemovedRoute;

pub(crate) fn journal_route_removal(removed: RemovedRoute) -> Journaled<'static> {
    Journaled::RouteRemoved {
        destination: removed.destination,
        cause: removed.cause,
    }
}

#[cfg(test)]
mod tests;
