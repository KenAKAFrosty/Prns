pub(crate) mod arguments;
mod error;
mod options;

pub use arguments::InterfacesArgs;

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use prns_config::editing::{
    ConfigEdit, ConfigFile, ConfigRepairReport, InterfaceDefinition, InterfaceName,
    InterfaceSetting, InterfaceSettingChange, InterfaceSettingKey, InterfaceSettingValue,
    SecretDisplay,
};
use prns_config::{discover, parse_and_plan_named, ConfigFix, InterfaceKind};
use prnsd_control::{config_digest, request_reload, ReloadResult, ServicePaths};

use crate::daemon::DEFAULT_CONFIG;

use arguments::{
    AddArgs, EditArgs, InterfaceOptions, InterfacesCommand, MutationArgs, NameArgs, RemoveArgs,
    RepairArgs,
};
use error::{InterfacesError, InterfacesIoOperation, InterfacesUsageError};

pub fn run(args: InterfacesArgs) -> ExitCode {
    match execute(args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("prnsd interfaces: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

fn execute(args: InterfacesArgs) -> Result<u8, InterfacesError> {
    let command = match args.command {
        Some(command) => command,
        None => return guided(args.config.as_deref(), args.show_secrets),
    };
    match command {
        InterfacesCommand::List => list(args.config.as_deref()),
        InterfacesCommand::Check => check(args.config.as_deref(), args.show_secrets),
        InterfacesCommand::Add(add) => add_interface(
            args.config.as_deref(),
            args.show_secrets,
            add,
            MutationMode::Scripted,
        ),
        InterfacesCommand::Edit(edit) => edit_interface(
            args.config.as_deref(),
            args.show_secrets,
            edit,
            MutationMode::Scripted,
        ),
        InterfacesCommand::Enable(name) => set_enabled(
            args.config.as_deref(),
            args.show_secrets,
            name,
            true,
            MutationMode::Scripted,
        ),
        InterfacesCommand::Disable(name) => set_enabled(
            args.config.as_deref(),
            args.show_secrets,
            name,
            false,
            MutationMode::Scripted,
        ),
        InterfacesCommand::Remove(remove) => remove_interface(
            args.config.as_deref(),
            args.show_secrets,
            remove,
            MutationMode::Scripted,
        ),
        InterfacesCommand::Repair(repair_args) => repair(
            args.config.as_deref(),
            args.show_secrets,
            repair_args,
            MutationMode::Scripted,
        ),
        InterfacesCommand::Apply => apply(args.config.as_deref()),
    }
}

fn list(config: Option<&Path>) -> Result<u8, InterfacesError> {
    let file = load(config)?;
    let interfaces = file.document().interfaces();
    if interfaces.is_empty() {
        println!(
            "No interface stanzas are configured in {}.",
            file.path().display()
        );
        return Ok(0);
    }
    for (index, interface) in interfaces.iter().enumerate() {
        let configured_type = interface.configured_type().unwrap_or("<missing type>");
        let state = match interface.enabled() {
            Some(true) => "enabled",
            Some(false) => "disabled",
            None => "invalid enabled value",
        };
        println!(
            "{}. {}: {} ({state})",
            index + 1,
            interface.name(),
            configured_type
        );
    }
    Ok(0)
}

fn check(config: Option<&Path>, show_secrets: bool) -> Result<u8, InterfacesError> {
    let file = load(config)?;
    match parse_and_plan_named(file.path().display().to_string(), file.document().source()) {
        Ok(report) => {
            for warning in report.warnings {
                eprintln!("{warning}");
            }
            println!("{} is semantically valid.", file.path().display());
            Ok(0)
        }
        Err(errors) => {
            for diagnostic in errors.diagnostics() {
                let display = if show_secrets {
                    SecretDisplay::Revealed
                } else {
                    SecretDisplay::Redacted
                };
                eprintln!("{}", diagnostic.display_with(display));
            }
            Ok(1)
        }
    }
}

fn add_interface(
    config: Option<&Path>,
    show_secrets: bool,
    args: AddArgs,
    mode: MutationMode,
) -> Result<u8, InterfacesError> {
    let terminal = io::stdin().is_terminal();
    let prompted = args.kind.is_none() || args.name.is_none();
    let kind = match args.kind {
        Some(kind) => kind,
        None if terminal => prompt_kind()?,
        None => return Err(InterfacesError::Usage(InterfacesUsageError::MissingType)),
    };
    let name = required_name(args.name, "Interface name", terminal)?;
    let radios = args.options.rnode_multi_radios.clone();
    if kind != InterfaceKind::RnodeMulti && !radios.is_empty() {
        return Err(InterfacesError::InapplicableSetting { key: "radio", kind });
    }
    let settings = args.options.settings(kind)?;
    let definition = InterfaceDefinition::new_with_rnode_multi_radios(
        name,
        kind,
        !args.disabled,
        settings,
        radios,
    )
    .map_err(InterfacesError::InterfaceDefinition)?;
    mutate(
        config,
        show_secrets,
        ConfigEdit::Add(definition),
        args.mutation,
        mode == MutationMode::Guided || prompted,
    )
}

fn edit_interface(
    config: Option<&Path>,
    show_secrets: bool,
    args: EditArgs,
    mode: MutationMode,
) -> Result<u8, InterfacesError> {
    let terminal = io::stdin().is_terminal();
    let prompted = args.name.is_none();
    let name = required_name(args.name, "Interface name", terminal)?;
    let file = load(config)?;
    let configured = file
        .document()
        .interfaces()
        .into_iter()
        .find(|configured| configured.name() == &name)
        .ok_or_else(|| InterfacesError::InterfaceNotFound(name.to_string()))?;
    let kind = configured
        .kind()
        .ok_or_else(|| InterfacesError::UntypedInterface(name.to_string()))?;
    let radios = args.options.rnode_multi_radios.clone();
    if kind != InterfaceKind::RnodeMulti && !radios.is_empty() {
        return Err(InterfacesError::InapplicableSetting { key: "radio", kind });
    }
    let settings = args.options.settings(kind)?;
    if settings.is_empty() && radios.is_empty() && args.rename.is_none() {
        return Err(InterfacesError::Usage(
            InterfacesUsageError::EditNeedsChange,
        ));
    }
    let mut edits = Vec::new();
    let target = if let Some(replacement) = args.rename {
        let replacement =
            InterfaceName::new(replacement).map_err(InterfacesError::InterfaceName)?;
        edits.push(ConfigEdit::Rename {
            current: name.clone(),
            replacement: replacement.clone(),
        });
        replacement
    } else {
        name
    };
    if !settings.is_empty() {
        edits.push(ConfigEdit::ChangeSettings {
            name: target.clone(),
            changes: settings
                .into_iter()
                .map(InterfaceSettingChange::Set)
                .collect(),
        });
    }
    if !radios.is_empty() {
        edits.push(ConfigEdit::ReplaceRNodeMultiRadios {
            name: target,
            radios,
        });
    }
    mutate_loaded(
        file,
        show_secrets,
        ConfigEdit::Batch(edits),
        args.mutation,
        mode == MutationMode::Guided || prompted,
    )
}

fn set_enabled(
    config: Option<&Path>,
    show_secrets: bool,
    args: NameArgs,
    enabled: bool,
    mode: MutationMode,
) -> Result<u8, InterfacesError> {
    let terminal = io::stdin().is_terminal();
    let prompted = args.name.is_none();
    let name = required_name(args.name, "Interface name", terminal)?;
    mutate(
        config,
        show_secrets,
        ConfigEdit::SetEnabled { name, enabled },
        args.mutation,
        mode == MutationMode::Guided || prompted,
    )
}

fn remove_interface(
    config: Option<&Path>,
    show_secrets: bool,
    args: RemoveArgs,
    mode: MutationMode,
) -> Result<u8, InterfacesError> {
    let terminal = io::stdin().is_terminal();
    let prompted = args.name.is_none() || !args.yes;
    let name = required_name(args.name, "Interface name", terminal)?;
    if !args.yes {
        if !terminal {
            return Err(InterfacesError::Usage(
                InterfacesUsageError::RemoveNeedsConfirmation,
            ));
        }
        if !confirm(&format!("Remove interface {name}?"), false)? {
            println!("No changes saved.");
            return Ok(0);
        }
    }
    mutate(
        config,
        show_secrets,
        ConfigEdit::Remove(name),
        args.mutation,
        mode == MutationMode::Guided || prompted,
    )
}

fn repair(
    config: Option<&Path>,
    show_secrets: bool,
    args: RepairArgs,
    mode: MutationMode,
) -> Result<u8, InterfacesError> {
    let file = load(config)?;
    let report = ConfigRepairReport::analyze(file.document().source())
        .map_err(InterfacesError::ConfigRepair)?;
    if report.diagnostics().is_empty() {
        println!("No semantic repairs are needed.");
        return Ok(0);
    }
    for diagnostic in report.diagnostics() {
        let display = if show_secrets {
            SecretDisplay::Revealed
        } else {
            SecretDisplay::Redacted
        };
        eprintln!("{}", diagnostic.display_with(display));
    }
    let interactive = io::stdin().is_terminal();
    let edit = if args.safe {
        report.safe_edit()
    } else if interactive {
        guided_repairs(&report)?
    } else {
        return Err(InterfacesError::Usage(
            InterfacesUsageError::RepairNeedsSafe,
        ));
    };
    let Some(edit) = edit else {
        return Err(InterfacesError::NoCompleteRepair);
    };
    mutate_loaded(
        file,
        show_secrets,
        edit,
        args.mutation,
        mode == MutationMode::Guided || !args.safe,
    )
}

fn guided_repairs(report: &ConfigRepairReport) -> Result<Option<ConfigEdit>, InterfacesError> {
    let mut edits = Vec::new();
    for diagnostic in report.diagnostics() {
        let fixes = diagnostic.fixes();
        if fixes.is_empty() {
            continue;
        }
        println!("{}", diagnostic.path());
        let has_value = fixes.iter().any(|fix| {
            matches!(
                fix,
                ConfigFix::InsertValue { .. }
                    | ConfigFix::ReplaceValue { .. }
                    | ConfigFix::ResolveAliases { .. }
            )
        });
        let has_type = fixes
            .iter()
            .any(|fix| matches!(fix, ConfigFix::ChooseInterfaceType { .. }));
        let has_remove = fixes
            .iter()
            .any(|fix| matches!(fix, ConfigFix::RemoveValue { .. }));
        let has_disable = fixes
            .iter()
            .any(|fix| matches!(fix, ConfigFix::DisableInterface { .. }));
        let mut choices = Vec::new();
        if has_value {
            choices.push("value");
        }
        if has_type {
            choices.push("type");
        }
        if has_remove {
            choices.push("remove");
        }
        if has_disable {
            choices.push("disable");
        }
        choices.push("skip");
        let default = if has_disable { "disable" } else { "skip" };
        let action = prompt(&format!(
            "Action [{}] (default {default})",
            choices.join("/")
        ))?;
        let action = if action.is_empty() {
            default
        } else {
            action.as_str()
        };
        match action.to_ascii_lowercase().as_str() {
            "disable" if has_disable => {
                let name = fixes.iter().find_map(|fix| match fix {
                    ConfigFix::DisableInterface { name } => Some(name.clone()),
                    _ => None,
                });
                if let Some(name) = name {
                    edits.push(ConfigEdit::SetEnabled {
                        name: InterfaceName::new(name).map_err(InterfacesError::InterfaceName)?,
                        enabled: false,
                    });
                }
            }
            "type" if has_type => {
                let name = fixes.iter().find_map(|fix| match fix {
                    ConfigFix::ChooseInterfaceType { name } => Some(name.clone()),
                    _ => None,
                });
                if let Some(name) = name {
                    edits.push(ConfigEdit::SetType {
                        name: InterfaceName::new(name).map_err(InterfacesError::InterfaceName)?,
                        kind: prompt_kind()?,
                    });
                }
            }
            "value" if has_value => {
                let target = fixes.iter().find_map(|fix| match fix {
                    ConfigFix::InsertValue { path, .. } | ConfigFix::ReplaceValue { path, .. } => {
                        Some((path.as_str(), &[][..]))
                    }
                    ConfigFix::ResolveAliases { path, aliases } => {
                        Some((path.as_str(), aliases.as_slice()))
                    }
                    _ => None,
                });
                if let Some((path, aliases)) = target {
                    edits.push(value_repair(path, diagnostic.accepted(), aliases)?);
                }
            }
            "remove" if has_remove => {
                let path = fixes.iter().find_map(|fix| match fix {
                    ConfigFix::RemoveValue { path, .. } => Some(path.as_str()),
                    _ => None,
                });
                if let Some(path) = path {
                    let (name, key) = interface_target(path)?;
                    let key = InterfaceSettingKey::parse(key).ok_or_else(|| {
                        InterfacesError::UnsupportedRepairSetting(key.to_string())
                    })?;
                    edits.push(ConfigEdit::ChangeSettings {
                        name,
                        changes: vec![InterfaceSettingChange::Remove(key)],
                    });
                }
            }
            "skip" => {}
            _ => {
                return Err(InterfacesError::Usage(InterfacesUsageError::RepairChoice));
            }
        }
    }
    if edits.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ConfigEdit::Batch(edits)))
    }
}

fn value_repair(
    path: &str,
    accepted: Option<&str>,
    aliases: &[String],
) -> Result<ConfigEdit, InterfacesError> {
    let (name, key) = interface_target(path)?;
    let suffix = accepted
        .map(|accepted| format!(" ({accepted})"))
        .unwrap_or_default();
    let value = prompt(&format!("New value for {key}{suffix}"))?;
    if key == "interface_enabled" {
        let enabled = parse_prompt_bool(&value)?;
        return Ok(ConfigEdit::SetEnabled { name, enabled });
    }
    let setting_key = InterfaceSettingKey::parse(key)
        .ok_or_else(|| InterfacesError::UnsupportedRepairSetting(key.to_string()))?;
    let mut changes = vec![InterfaceSettingChange::Set(InterfaceSetting::new(
        setting_key,
        InterfaceSettingValue::Text(value),
    ))];
    for alias in aliases {
        if let Some(alias) = InterfaceSettingKey::parse(alias) {
            changes.push(InterfaceSettingChange::Remove(alias));
        }
    }
    Ok(ConfigEdit::ChangeSettings { name, changes })
}

fn interface_target(path: &str) -> Result<(InterfaceName, &str), InterfacesError> {
    let start = path
        .find("[[")
        .ok_or_else(|| InterfacesError::RepairPathNotInterface(path.to_string()))?
        + 2;
    let rest = &path[start..];
    let end = rest
        .find("]]")
        .ok_or_else(|| InterfacesError::RepairPathMissingName(path.to_string()))?;
    let key = path.rsplit(" > ").next().unwrap_or_default().trim();
    let name = InterfaceName::new(rest[..end].trim()).map_err(InterfacesError::InterfaceName)?;
    Ok((name, key))
}

fn parse_prompt_bool(value: &str) -> Result<bool, InterfacesError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Ok(true),
        "no" | "false" | "off" | "0" => Ok(false),
        _ => Err(InterfacesError::Usage(InterfacesUsageError::BooleanValue)),
    }
}

fn mutate(
    config: Option<&Path>,
    show_secrets: bool,
    edit: ConfigEdit,
    mutation: MutationArgs,
    interactive: bool,
) -> Result<u8, InterfacesError> {
    mutate_loaded(load(config)?, show_secrets, edit, mutation, interactive)
}

fn mutate_loaded(
    file: ConfigFile,
    show_secrets: bool,
    edit: ConfigEdit,
    mutation: MutationArgs,
    interactive: bool,
) -> Result<u8, InterfacesError> {
    let edited = file
        .document()
        .edit(&edit)
        .map_err(InterfacesError::ConfigEdit)?;
    let display = if show_secrets {
        SecretDisplay::Revealed
    } else {
        SecretDisplay::Redacted
    };
    print!("{}", edited.diff(display));
    if mutation.dry_run {
        println!("Dry run: no changes saved.");
        return Ok(0);
    }
    if interactive && !confirm("Save this configuration?", true)? {
        println!("No changes saved.");
        return Ok(0);
    }
    let receipt = file.write(&edited).map_err(InterfacesError::ConfigFile)?;
    println!("Saved {}.", receipt.path().display());
    if let Some(backup) = receipt.backup() {
        println!("Previous configuration: {}", backup.display());
    }
    let should_apply = if mutation.apply {
        true
    } else if interactive {
        confirm("Apply this interface change to the running daemon?", true)?
    } else {
        false
    };
    if should_apply {
        apply_path(receipt.path())
    } else {
        Ok(0)
    }
}

fn apply(config: Option<&Path>) -> Result<u8, InterfacesError> {
    let file = load(config)?;
    apply_path(file.path())
}

fn apply_path(path: &Path) -> Result<u8, InterfacesError> {
    let bytes = std::fs::read(path).map_err(|source| InterfacesError::Io {
        operation: InterfacesIoOperation::ReadConfiguration,
        path: Some(path.to_path_buf()),
        source,
    })?;
    let paths = ServicePaths::discover().map_err(InterfacesError::StateDirectory)?;
    let Some(result) =
        request_reload(&paths, config_digest(&bytes)).map_err(InterfacesError::Control)?
    else {
        return Err(InterfacesError::NoManagedDaemon);
    };
    match result {
        ReloadResult::Applied => {
            println!("Interface changes applied without restarting prnsd.");
            Ok(0)
        }
        ReloadResult::Unchanged => {
            println!("The running interface plan already matches the configuration.");
            Ok(0)
        }
        ReloadResult::RestartRequired => Err(InterfacesError::RestartRequired),
        ReloadResult::NotInterfaceOwner => Err(InterfacesError::NotInterfaceOwner),
        ReloadResult::Rejected => Err(InterfacesError::ReloadRejected),
        ReloadResult::RolledBack { rollback_failed } => {
            Err(InterfacesError::ReloadRolledBack { rollback_failed })
        }
    }
}

fn guided(config: Option<&Path>, show_secrets: bool) -> Result<u8, InterfacesError> {
    if !io::stdin().is_terminal() {
        return Err(InterfacesError::Usage(
            InterfacesUsageError::MissingSubcommand,
        ));
    }
    loop {
        let file = load(config)?;
        println!("Interfaces in {}:", file.path().display());
        for (index, interface) in file.document().interfaces().iter().enumerate() {
            println!(
                "  {}. {} ({})",
                index + 1,
                interface.name(),
                interface.configured_type().unwrap_or("missing type")
            );
        }
        println!("  a. Add   c. Check   r. Repair   p. Apply   q. Quit");
        let selection = prompt("Selection")?;
        match selection.trim().to_ascii_lowercase().as_str() {
            "a" | "add" => {
                let kind = prompt_kind()?;
                let name = prompt("Interface name")?;
                add_interface(
                    config,
                    show_secrets,
                    AddArgs {
                        kind: Some(kind),
                        name: Some(name),
                        disabled: false,
                        options: InterfaceOptions::default(),
                        mutation: MutationArgs {
                            dry_run: false,
                            apply: false,
                        },
                    },
                    MutationMode::Guided,
                )?;
            }
            "c" | "check" => {
                check(config, show_secrets)?;
            }
            "r" | "repair" => {
                repair(
                    config,
                    show_secrets,
                    RepairArgs {
                        safe: false,
                        mutation: MutationArgs {
                            dry_run: false,
                            apply: false,
                        },
                    },
                    MutationMode::Guided,
                )?;
            }
            "p" | "apply" => {
                apply(config)?;
            }
            "q" | "quit" | "" => return Ok(0),
            value => guided_interface(config, show_secrets, &file, value)?,
        }
    }
}

fn guided_interface(
    config: Option<&Path>,
    show_secrets: bool,
    file: &ConfigFile,
    value: &str,
) -> Result<(), InterfacesError> {
    let index = value
        .parse::<usize>()
        .map_err(|_| InterfacesError::Usage(InterfacesUsageError::InvalidSelection))?;
    let interfaces = file.document().interfaces();
    let selected = interfaces
        .get(index.saturating_sub(1))
        .ok_or(InterfacesError::Usage(
            InterfacesUsageError::MissingSelection,
        ))?;
    let action = prompt("Action [enable/disable/remove]")?;
    let name = selected.name().as_str().to_string();
    let mutation = MutationArgs {
        dry_run: false,
        apply: false,
    };
    match action.trim().to_ascii_lowercase().as_str() {
        "enable" => set_enabled(
            config,
            show_secrets,
            NameArgs {
                name: Some(name),
                mutation,
            },
            true,
            MutationMode::Guided,
        )?,
        "disable" => set_enabled(
            config,
            show_secrets,
            NameArgs {
                name: Some(name),
                mutation,
            },
            false,
            MutationMode::Guided,
        )?,
        "remove" => remove_interface(
            config,
            show_secrets,
            RemoveArgs {
                name: Some(name),
                yes: false,
                mutation,
            },
            MutationMode::Guided,
        )?,
        _ => {
            return Err(InterfacesError::Usage(
                InterfacesUsageError::UnknownGuidedAction,
            ))
        }
    };
    Ok(())
}

fn load(config: Option<&Path>) -> Result<ConfigFile, InterfacesError> {
    let discovered = discover(config).map_err(InterfacesError::Discovery)?;
    let path = discovered
        .config
        .unwrap_or_else(|| discovered.dir.join("config"));
    ConfigFile::load(path, DEFAULT_CONFIG).map_err(InterfacesError::ConfigFile)
}

fn required_name(
    value: Option<String>,
    label: &str,
    interactive: bool,
) -> Result<InterfaceName, InterfacesError> {
    let value = match value {
        Some(value) => value,
        None if interactive => prompt(label)?,
        None => return Err(InterfacesError::Usage(InterfacesUsageError::MissingName)),
    };
    InterfaceName::new(value).map_err(InterfacesError::InterfaceName)
}

fn prompt_kind() -> Result<InterfaceKind, InterfacesError> {
    println!("Interface types:");
    for (index, canonical) in InterfaceKind::CANONICAL_NAMES.iter().enumerate() {
        println!("  {}. {canonical}", index + 1);
    }
    let value = prompt("Type")?;
    if let Ok(index) = value.parse::<usize>() {
        if let Some(kind) = InterfaceKind::CANONICAL_NAMES
            .get(index.saturating_sub(1))
            .and_then(|canonical| InterfaceKind::parse(canonical))
        {
            return Ok(kind);
        }
    }
    InterfaceKind::parse_cli(&value)
        .ok_or({ InterfacesError::Usage(InterfacesUsageError::UnknownInterfaceType(value)) })
}

fn prompt(label: &str) -> Result<String, InterfacesError> {
    print!("{label}: ");
    io::stdout().flush().map_err(|source| InterfacesError::Io {
        operation: InterfacesIoOperation::WritePrompt,
        path: None,
        source,
    })?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|source| InterfacesError::Io {
            operation: InterfacesIoOperation::ReadPrompt,
            path: None,
            source,
        })?;
    Ok(value.trim().to_string())
}

fn confirm(label: &str, default: bool) -> Result<bool, InterfacesError> {
    let suffix = if default { "[Y/n]" } else { "[y/N]" };
    let answer = prompt(&format!("{label} {suffix}"))?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "" => Ok(default),
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => Err(InterfacesError::Usage(
            InterfacesUsageError::ConfirmationValue,
        )),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MutationMode {
    Scripted,
    Guided,
}
