use std::vec::Vec;

pub use crate::engine::RouteSnapshot;
use crate::wire::DestinationHash;

pub trait InspectionSource {
    fn link_count(&self) -> impl core::future::Future<Output = u32> + Send;

    fn routes(&self) -> impl core::future::Future<Output = Vec<RouteSnapshot>> + Send;

    fn route(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<Output = Option<RouteSnapshot>> + Send;
}
