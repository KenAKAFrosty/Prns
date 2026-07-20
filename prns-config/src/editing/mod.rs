mod catalog;
mod document;
mod interface;
mod repair;
mod store;

pub use crate::diagnostic::SecretDisplay;
pub use document::{
    ConfigEdit, ConfigEditError, ConfiguredInterface, EditedConfig, InterfaceSettingChange,
};
pub use interface::{
    InterfaceConfigKey, InterfaceConfigKeyError, InterfaceDefinition, InterfaceDefinitionError,
    InterfaceName, InterfaceNameError, InterfaceSetting, InterfaceSettingKey,
    InterfaceSettingValue, RNodeMultiRadioDefinition, RNodeMultiRadioDefinitionError,
};
pub use repair::{ConfigRepairError, ConfigRepairReport};
pub use store::{ConfigFile, ConfigFileError, ConfigFileOperation, ConfigWriteReceipt};

#[cfg(test)]
mod tests;
pub use catalog::{
    ConfiguredInterfaceSetting, InterfaceSettingCategory, InterfaceSettingCondition,
    InterfaceSettingInputError, InterfaceSettingInputKind, InterfaceSettingSpec,
    InterfaceSettingTier,
};
