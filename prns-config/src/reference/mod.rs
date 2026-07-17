mod interpret;
mod parse;
mod schema;
mod types;
mod validation;

pub use parse::{parse, parse_named};
pub use types::{
    RNodeRadio, RNodeSubinterface, ReferenceConfig, ReferenceDiscoveryConfig, ReferenceInterface,
    ReferenceInterfaceDiscovery, ReferenceMode, ReferenceParams, ReferenceValue,
};

#[cfg(test)]
mod tests;
