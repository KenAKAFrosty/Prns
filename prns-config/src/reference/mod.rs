pub(crate) mod i2p;
mod interpret;
pub(crate) mod keys;
mod parse;
mod schema;
mod types;
mod validation;

pub(crate) use interpret::cleaned_number;
pub use parse::{parse, parse_named};
pub use types::{
    RNodeRadio, RNodeSubinterface, ReferenceConfig, ReferenceDiscoveryConfig, ReferenceInterface,
    ReferenceInterfaceDiscovery, ReferenceMode, ReferenceParams, ReferenceValue,
};

#[cfg(test)]
mod tests;
