mod announce_completion;
mod decrypt_resume;
mod delivery;
mod held_announce_release;
mod link_handshake_completion;
mod packet_dispatch;
mod relay;
mod remote_control_pairing;
#[cfg(test)]
mod test_manifold;
#[cfg(test)]
pub(crate) use test_manifold::drive_packet_to_quiescence;

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
