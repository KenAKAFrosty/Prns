mod esp32c6;
mod esp32s3;
mod nrf52840;
#[cfg(test)]
mod test_fixed_storage;

pub use esp32c6::Esp32C6;
pub use esp32s3::Esp32S3;
pub use nrf52840::Nrf52840;
#[cfg(test)]
pub(crate) use test_fixed_storage::TestFixedStorage;

#[cfg(feature = "alloc")]
pub use growable_heap::GrowableHeap;
#[cfg(feature = "alloc")]
mod growable_heap;

#[cfg(test)]
fn assert_same_storage<A, B>()
where
    A: crate::storage::StorageLayout,
    B: crate::storage::StorageLayout<
        Routes = A::Routes,
        Announces = A::Announces,
        History = A::History,
        AppData = A::AppData,
        ScheduledAnnounces = A::ScheduledAnnounces,
        UpstreamAppDestinations = A::UpstreamAppDestinations,
        HeldIdentities = A::HeldIdentities,
        SelfRatchets = A::SelfRatchets,
        Receipts = A::Receipts,
        PacketHashes = A::PacketHashes,
        ReverseRoutes = A::ReverseRoutes,
        PendingPathRequests = A::PendingPathRequests,
        SeenPathRequests = A::SeenPathRequests,
        AnnounceRates = A::AnnounceRates,
        GroupKeys = A::GroupKeys,
        RequestHandlers = A::RequestHandlers,
        TransportedLinks = A::TransportedLinks,
        Links = A::Links,
        OutgoingResources = A::OutgoingResources,
        IncomingResources = A::IncomingResources,
    >,
{
}
