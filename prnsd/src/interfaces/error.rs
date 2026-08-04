use std::fmt;
use std::io;
use std::path::PathBuf;

use prns_config::editing::{
    ConfigEditError, ConfigFileError, ConfigRepairError, InterfaceConfigKeyError,
    InterfaceDefinitionError, InterfaceNameError, InterfaceSettingInputError,
    RNodeMultiRadioDefinitionError,
};
use prns_config::InterfaceKind;
use prnsd_control::{ServiceError, StateDirectoryError};

#[derive(Debug)]
pub(super) enum InterfacesError {
    Usage(InterfacesUsageError),
    InterfaceNotFound(String),
    UntypedInterface(String),
    UnsupportedRepairSetting(String),
    RepairPathNotInterface(String),
    RepairPathMissingName(String),
    InapplicableSetting {
        key: &'static str,
        kind: InterfaceKind,
    },
    UnavailableInBuild(InterfaceKind),
    InvalidPort(InterfaceKind),
    UnknownSettingKey(&'static str),
    RestartRequired,
    NotInterfaceOwner,
    ReloadRejected,
    ReloadRolledBack {
        rollback_failed: bool,
    },
    NoManagedDaemon,
    CommandContext(crate::command_context::CommandContextError),
    Io {
        operation: InterfacesIoOperation,
        path: Option<PathBuf>,
        source: io::Error,
    },
    ConfigFile(ConfigFileError),
    ConfigRollback {
        apply: Box<InterfacesError>,
        rollback: ConfigFileError,
    },
    ConfigEdit(ConfigEditError),
    ConfigRepair(ConfigRepairError),
    InterfaceDefinition(InterfaceDefinitionError),
    InterfaceName(InterfaceNameError),
    InterfaceConfigKey(InterfaceConfigKeyError),
    InterfaceSettingInput(InterfaceSettingInputError),
    RNodeMultiRadioDefinition(RNodeMultiRadioDefinitionError),
    StateDirectory(StateDirectoryError),
    Control(ServiceError),
}

impl InterfacesError {
    pub(super) const fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_)
            | Self::InapplicableSetting { .. }
            | Self::UnavailableInBuild(_)
            | Self::InvalidPort(_)
            | Self::UnknownSettingKey(_) => 2,
            Self::NoManagedDaemon => 3,
            Self::InterfaceNotFound(_)
            | Self::UntypedInterface(_)
            | Self::UnsupportedRepairSetting(_)
            | Self::RepairPathNotInterface(_)
            | Self::RepairPathMissingName(_)
            | Self::RestartRequired
            | Self::NotInterfaceOwner
            | Self::ReloadRejected
            | Self::ReloadRolledBack { .. }
            | Self::Io { .. }
            | Self::CommandContext(_)
            | Self::ConfigFile(_)
            | Self::ConfigRollback { .. }
            | Self::ConfigEdit(_)
            | Self::ConfigRepair(_)
            | Self::InterfaceDefinition(_)
            | Self::InterfaceName(_)
            | Self::InterfaceConfigKey(_)
            | Self::InterfaceSettingInput(_)
            | Self::RNodeMultiRadioDefinition(_)
            | Self::StateDirectory(_)
            | Self::Control(_) => 1,
        }
    }
}

impl fmt::Display for InterfacesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(error) => error.fmt(formatter),
            Self::InterfaceNotFound(name) => write!(formatter, "interface {name} was not found"),
            Self::UntypedInterface(name) => write!(
                formatter,
                "interface {name} has an unknown type and cannot be edited as a typed interface"
            ),
            Self::UnsupportedRepairSetting(key) => {
                write!(formatter, "repair cannot change unsupported setting key {key}")
            }
            Self::RepairPathNotInterface(path) => {
                write!(formatter, "repair path is not interface-scoped: {path}")
            }
            Self::RepairPathMissingName(path) => {
                write!(formatter, "repair path has no interface name: {path}")
            }
            Self::InapplicableSetting { key, kind } => write!(
                formatter,
                "--{} does not apply to {}",
                key.replace('_', "-"),
                kind.canonical_name()
            ),
            Self::UnavailableInBuild(kind) => write!(
                formatter,
                "{} is not available in this prnsd build",
                kind.canonical_name()
            ),
            Self::InvalidPort(kind) => write!(
                formatter,
                "--port must be an unsigned 16-bit port number for {}",
                kind.canonical_name()
            ),
            Self::UnknownSettingKey(key) => write!(formatter, "unknown setting key {key}"),
            Self::RestartRequired => formatter.write_str(
                "the configuration includes non-interface changes; restart prnsd to apply it",
            ),
            Self::NotInterfaceOwner => formatter.write_str(
                "this prnsd is a shared-instance client; apply the change in the routing-table owner",
            ),
            Self::ReloadRejected => {
                formatter.write_str("the daemon rejected the interface apply request")
            }
            Self::ReloadRolledBack {
                rollback_failed: false,
            } => formatter.write_str(
                "interface apply failed; the previous runtime interfaces were restored",
            ),
            Self::ReloadRolledBack {
                rollback_failed: true,
            } => formatter.write_str(
                "interface apply failed and the previous runtime interfaces could not be fully restored",
            ),
            Self::NoManagedDaemon => formatter.write_str("no managed daemon is running"),
            Self::Io {
                operation,
                path: Some(path),
                source,
            } => write!(formatter, "could not {operation} {}: {source}", path.display()),
            Self::Io {
                operation,
                path: None,
                source,
            } => write!(formatter, "could not {operation}: {source}"),
            Self::CommandContext(error) => error.fmt(formatter),
            Self::ConfigFile(error) => error.fmt(formatter),
            Self::ConfigRollback { apply, rollback } => write!(
                formatter,
                "{apply}; the saved configuration could not be restored: {rollback}"
            ),
            Self::ConfigEdit(error) => error.fmt(formatter),
            Self::ConfigRepair(error) => error.fmt(formatter),
            Self::InterfaceDefinition(error) => error.fmt(formatter),
            Self::InterfaceName(error) => error.fmt(formatter),
            Self::InterfaceConfigKey(error) => error.fmt(formatter),
            Self::InterfaceSettingInput(error) => error.fmt(formatter),
            Self::RNodeMultiRadioDefinition(error) => error.fmt(formatter),
            Self::StateDirectory(error) => error.fmt(formatter),
            Self::Control(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for InterfacesError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InterfacesIoOperation {
    ReadConfiguration,
    WritePrompt,
    ReadPrompt,
}

impl fmt::Display for InterfacesIoOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReadConfiguration => "read configuration",
            Self::WritePrompt => "write prompt",
            Self::ReadPrompt => "read prompt input",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InterfacesUsageError {
    MissingType,
    MissingName,
    EditNeedsChange,
    RemoveNeedsConfirmation,
    RepairNeedsSafe,
    RepairChoice,
    BooleanValue,
    MissingSubcommand,
    InvalidSelection,
    MissingSelection,
    UnknownGuidedAction,
    UnknownInterfaceType(String),
    ConfirmationValue,
}

impl fmt::Display for InterfacesUsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingType => "TYPE is required without a TTY",
            Self::MissingName => "NAME is required without a TTY",
            Self::EditNeedsChange => "edit requires --rename or at least one typed setting option",
            Self::RemoveNeedsConfirmation => "remove requires --yes without a TTY",
            Self::RepairNeedsSafe => "repair requires --safe when standard input is not a TTY",
            Self::RepairChoice => "choose one of the listed repair actions",
            Self::BooleanValue => "enter yes or no for this setting",
            Self::MissingSubcommand => "a subcommand is required when standard input is not a TTY",
            Self::InvalidSelection => "choose an interface number or a listed command",
            Self::MissingSelection => "the selected interface number does not exist",
            Self::UnknownGuidedAction => "unknown interface action",
            Self::UnknownInterfaceType(value) => {
                return write!(formatter, "unknown interface type {value:?}")
            }
            Self::ConfirmationValue => "answer yes or no",
        })
    }
}
