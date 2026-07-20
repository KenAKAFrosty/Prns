mod diagnostics;
pub(crate) mod i2p;
mod interface_type;
mod interpret;
pub(crate) mod keys;
mod parse;
mod rnode_multi;
mod schema;
mod types;
mod validation;

pub(crate) use interpret::cleaned_number;
pub use parse::{parse, parse_named};
pub use types::{
    RNodeRadio, RNodeSubinterface, ReferenceBlackholeExchange, ReferenceConfig,
    ReferenceConfigParams, ReferenceDiscoveryConfig, ReferenceInterface,
    ReferenceInterfaceDiscovery, ReferenceMode, ReferenceRemoteManagement, ReferenceValue,
};

#[cfg(test)]
mod tests;
