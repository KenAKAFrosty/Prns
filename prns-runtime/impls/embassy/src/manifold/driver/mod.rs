mod egress;
mod fixed_topology;
mod host;
mod interface_seam;
mod interface_status;
mod packet_phy;
mod pooled_topology;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(any(test, feature = "std"))]
pub use super::grant_lane::leaked_grant_lane;
pub use super::grant_lane::{embassy_grant_lane, EmbassyGrantConsumer, EmbassyGrantProducer};
pub use egress::{EgressOutcome, EmbassyEgress, ManifoldEgress, PooledEgress};
pub use fixed_topology::{run, run_with_deciders, run_with_store, ManifoldWiring};
pub use host::EmbassyHost;
pub(crate) use host::ResumableHost;
pub use interface_seam::EmbassyInterfaceSeam;
pub use interface_status::EmbassyInterfaceStatus;
pub(crate) use pooled_topology::run_pooled;
pub use pooled_topology::{InterfaceLifecycle, PooledWiring};
