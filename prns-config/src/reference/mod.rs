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

pub use interface_type::InterfaceKind;
pub(crate) use interpret::cleaned_number;
pub use parse::{parse, parse_named};
pub use types::{
    RNodeRadio, RNodeSubinterface, ReferenceAnnounceRateTarget, ReferenceBlackholeExchange,
    ReferenceConfig, ReferenceConfigParams, ReferenceDiscoveryConfig, ReferenceInterface,
    ReferenceInterfaceDiscovery, ReferenceMode, ReferencePrnsConfig, ReferenceRemoteManagement,
    ReferenceValue,
};
pub(crate) use validation::supported_websocket_target;

pub(crate) fn announce_rate_target_is_explicit_off(text: &str) -> bool {
    matches!(
        text.trim().to_ascii_lowercase().as_str(),
        "off" | "no" | "false"
    )
}

#[cfg(test)]
mod tests;
