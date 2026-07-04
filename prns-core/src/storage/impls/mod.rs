mod esp32c6;
#[cfg(feature = "external-alloc")]
mod esp32s3;
#[cfg(feature = "alloc")]
mod growable_heap;
mod nrf52840;
#[cfg(any(test, feature = "test-support"))]
mod test_fixed_storage;

pub use esp32c6::Esp32C6;
#[cfg(feature = "external-alloc")]
pub use esp32s3::Esp32S3;
#[cfg(feature = "alloc")]
pub use growable_heap::GrowableHeap;
pub use nrf52840::Nrf52840;
#[cfg(any(test, feature = "test-support"))]
pub use test_fixed_storage::TestFixedStorage;

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
        RecentPathRequests = A::RecentPathRequests,
        SeenPathRequests = A::SeenPathRequests,
        AnnounceRates = A::AnnounceRates,
        GroupKeys = A::GroupKeys,
        RequestHandlers = A::RequestHandlers,
        TransportedLinks = A::TransportedLinks,
        Links = A::Links,
        OutgoingResources = A::OutgoingResources,
        IncomingResources = A::IncomingResources,
        Channels = A::Channels,
    >,
{
}
