pub mod configobj;
pub mod definition;
pub mod discovery;
pub mod reference;

pub use definition::{InterfaceDefinition, OwnedInterfaceKind};
pub use discovery::{discover, DiscoveredConfigs};
pub use reference::{
    RNodeRadio, RNodeSubinterface, ReferenceConfig, ReferenceInterface, ReferenceMode,
    ReferenceParams, ReferenceValue,
};
