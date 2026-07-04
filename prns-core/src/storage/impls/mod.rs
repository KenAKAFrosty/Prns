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
        Tunnels = A::Tunnels,
        DepartedInterfaces = A::DepartedInterfaces,
        DiscoveryPathRequests = A::DiscoveryPathRequests,
        InterfacePathRequestLimits = A::InterfacePathRequestLimits,
        InterfaceAnnounceLimits = A::InterfaceAnnounceLimits,
        HeldAnnounces = A::HeldAnnounces,
        HeldAnnounceAppData = A::HeldAnnounceAppData,
        AnnounceRates = A::AnnounceRates,
        GroupKeys = A::GroupKeys,
        RequestHandlers = A::RequestHandlers,
        TransportedLinks = A::TransportedLinks,
        Links = A::Links,
        OutgoingResources = A::OutgoingResources,
        IncomingResources = A::IncomingResources,
        IncomingAssemblies = A::IncomingAssemblies,
        OutgoingAssemblies = A::OutgoingAssemblies,
        Channels = A::Channels,
        DirtyInterfaces = A::DirtyInterfaces,
    >,
{
    assert_eq!(A::LIMITS, B::LIMITS);
}

#[cfg(test)]
mod tests {
    use super::assert_same_storage;
    use crate::storage::{Esp32C6, Nrf52840};

    #[test]
    fn esp32c6_and_nrf52840_share_the_full_storage_profile() {
        assert_same_storage::<Esp32C6, Nrf52840>();
    }
}
