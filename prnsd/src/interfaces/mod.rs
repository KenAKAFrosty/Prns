pub(crate) mod arguments;
mod error;
mod guided;
mod options;
mod presentation;

pub use arguments::InterfacesArgs;

use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use prns_config::editing::{
    ConfigEdit, ConfigFile, ConfigRepairReport, InterfaceConfigKey, InterfaceDefinition,
    InterfaceName, InterfaceSetting, InterfaceSettingChange, InterfaceSettingKey,
    InterfaceSettingValue, SecretDisplay,
};
use prns_config::{discover, parse_and_plan_named, ConfigFix, InterfaceKind};
use prnsd_control::{
    config_digest, request_reload, running as managed_running, ReloadResult, ServicePaths,
    ServiceState,
};

use crate::daemon::DEFAULT_CONFIG;

use arguments::{
    AddArgs, EditArgs, InterfaceOptions, InterfacesCommand, MutationArgs, NameArgs, RemoveArgs,
    RepairArgs,
};
use error::{InterfacesError, InterfacesIoOperation, InterfacesUsageError};
use presentation::{friendly_kind, ApplyStatus, Presentation, RuntimeStatus, ValidationState};

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
        InterfacesCommand::Validate(options) => {
            validate(args.config.as_deref(), args.show_secrets, options.details)
        }
        InterfacesCommand::Add(add) => add_interface(
            args.config.as_deref(),
            args.show_secrets,
            add,
            MutationMode::Scripted,
            None,
        ),
        InterfacesCommand::Edit(edit) => edit_interface(
            args.config.as_deref(),
            args.show_secrets,
            edit,
            MutationMode::Scripted,
            None,
        ),
        InterfacesCommand::Enable(name) => set_enabled(
            args.config.as_deref(),
            args.show_secrets,
            name,
            true,
            MutationMode::Scripted,
            None,
        ),
        InterfacesCommand::Disable(name) => set_enabled(
            args.config.as_deref(),
            args.show_secrets,
            name,
            false,
            MutationMode::Scripted,
            None,
        ),
        InterfacesCommand::Remove(remove) => remove_interface(
            args.config.as_deref(),
            args.show_secrets,
            remove,
            MutationMode::Scripted,
            None,
        ),
        InterfacesCommand::Repair(repair_args) => repair(
            args.config.as_deref(),
            args.show_secrets,
            repair_args,
            MutationMode::Scripted,
            None,
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

fn validate(
    config: Option<&Path>,
    show_secrets: bool,
    details: bool,
) -> Result<u8, InterfacesError> {
    let file = load(config)?;
    let result = parse_and_plan_named(file.path().display().to_string(), file.document().source());
    let terminal_output = io::stdout().is_terminal();
    let display = if show_secrets {
        SecretDisplay::Revealed
    } else {
        SecretDisplay::Redacted
    };
    if !terminal_output {
        match result {
            Ok(report) => {
                for warning in report.warnings {
                    eprintln!("{}", warning.display_with(display));
                }
                println!("{} is semantically valid.", file.path().display());
                return Ok(0);
            }
            Err(errors) => {
                for diagnostic in errors.diagnostics() {
                    eprintln!("{}", diagnostic.display_with(display));
                }
                return Ok(1);
            }
        }
    }
    let state = ValidationState::from_result(result);
    let presentation = Presentation::new(crate::terminal::enabled(terminal_output));
    print!(
        "{}",
        presentation.validation(file.path(), &state, details, display)
    );
    Ok(if state.is_valid() { 0 } else { 1 })
}

fn add_interface(
    config: Option<&Path>,
    show_secrets: bool,
    args: AddArgs,
    mode: MutationMode,
    session: Option<&mut ApplySession>,
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
    if terminal && prompted {
        if let Some(definition) = guided::add_interface(kind, name, show_secrets, settings, radios)?
        {
            return mutate(
                config,
                show_secrets,
                ConfigEdit::Add(definition),
                args.mutation,
                true,
                session,
            );
        }
        return Ok(0);
    }
    let source_name = load(config)?.path().display().to_string();
    let definition = InterfaceDefinition::new_named_with_rnode_multi_radios(
        source_name,
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
        session,
    )
}

fn edit_interface(
    config: Option<&Path>,
    show_secrets: bool,
    args: EditArgs,
    mode: MutationMode,
    session: Option<&mut ApplySession>,
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
        if terminal {
            if let Some(edit) = guided::edit_interface(&file, &configured, show_secrets)? {
                return mutate_loaded(file, show_secrets, edit, args.mutation, true, session);
            }
            return Ok(0);
        }
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
        session,
    )
}

fn set_enabled(
    config: Option<&Path>,
    show_secrets: bool,
    args: NameArgs,
    enabled: bool,
    mode: MutationMode,
    session: Option<&mut ApplySession>,
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
        session,
    )
}

fn remove_interface(
    config: Option<&Path>,
    show_secrets: bool,
    args: RemoveArgs,
    mode: MutationMode,
    session: Option<&mut ApplySession>,
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
        session,
    )
}

fn repair(
    config: Option<&Path>,
    show_secrets: bool,
    args: RepairArgs,
    mode: MutationMode,
    session: Option<&mut ApplySession>,
) -> Result<u8, InterfacesError> {
    let file = load(config)?;
    let report = ConfigRepairReport::analyze_named(
        file.path().display().to_string(),
        file.document().source(),
    )
    .map_err(InterfacesError::ConfigRepair)?;
    if report.diagnostics().is_empty() {
        let presentation = Presentation::new(crate::terminal::enabled(io::stdout().is_terminal()));
        println!(
            "{}",
            presentation.success("No semantic repairs are needed.")
        );
        return Ok(0);
    }
    let interactive = io::stdin().is_terminal();
    let display = if show_secrets {
        SecretDisplay::Revealed
    } else {
        SecretDisplay::Redacted
    };
    if interactive {
        let presentation = Presentation::new(crate::terminal::enabled(io::stdout().is_terminal()));
        print!(
            "{}",
            presentation.repair_summary(file.path(), report.diagnostics(), false, display)
        );
        let action = prompt("[Enter] Continue  [D] Details  [B] Back")?;
        match action.trim().to_ascii_lowercase().as_str() {
            "d" | "details" => print!(
                "{}",
                presentation.repair_summary(file.path(), report.diagnostics(), true, display)
            ),
            "b" | "back" => return Ok(0),
            "" => {}
            _ => return Err(InterfacesError::Usage(InterfacesUsageError::RepairChoice)),
        }
    } else {
        for diagnostic in report.diagnostics() {
            eprintln!("{}", diagnostic.display_with(display));
        }
    }
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
        println!("No repairs selected.");
        return Ok(0);
    };
    mutate_loaded(
        file,
        show_secrets,
        edit,
        args.mutation,
        mode == MutationMode::Guided || !args.safe,
        session,
    )
}

fn guided_repairs(report: &ConfigRepairReport) -> Result<Option<ConfigEdit>, InterfacesError> {
    let mut edits = Vec::new();
    let metadata = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| {
            diagnostic.code() == prns_config::ConfigDiagnosticCode::PersistedRuntimeMetadata
        })
        .collect::<Vec<_>>();
    if !metadata.is_empty() {
        println!(
            "{} runtime-only RNS {} can be safely removed:",
            metadata.len(),
            if metadata.len() == 1 {
                "field"
            } else {
                "fields"
            }
        );
        for diagnostic in &metadata {
            println!(
                "  • {}",
                diagnostic
                    .path()
                    .rsplit(" > ")
                    .next()
                    .unwrap_or(diagnostic.path())
            );
        }
        if confirm("Remove these runtime-only fields?", true)? {
            for diagnostic in metadata {
                let (name, key) = interface_target(diagnostic.path())?;
                edits.push(ConfigEdit::RemoveInterfaceValue {
                    name,
                    key: InterfaceConfigKey::new(key)
                        .map_err(InterfacesError::InterfaceConfigKey)?,
                });
            }
        }
        println!();
    }
    let mut unknown_by_interface = BTreeMap::<InterfaceName, Vec<&str>>::new();
    for diagnostic in report.diagnostics().iter().filter(|diagnostic| {
        diagnostic.code() == prns_config::ConfigDiagnosticCode::UnknownKey
            && diagnostic.path().contains("[[")
    }) {
        let (name, key) = interface_target(diagnostic.path())?;
        unknown_by_interface.entry(name).or_default().push(key);
    }
    for (name, keys) in unknown_by_interface {
        println!(
            "{name} contains {} unknown {}:",
            keys.len(),
            if keys.len() == 1 { "field" } else { "fields" }
        );
        for key in &keys {
            println!("  • {key}");
        }
        println!("Prns ignores these fields, but another tool may own them.");
        if confirm("Remove these unknown fields?", false)? {
            for key in keys {
                edits.push(ConfigEdit::RemoveInterfaceValue {
                    name: name.clone(),
                    key: InterfaceConfigKey::new(key)
                        .map_err(InterfacesError::InterfaceConfigKey)?,
                });
            }
        }
        println!();
    }
    for diagnostic in report.diagnostics() {
        if matches!(
            diagnostic.code(),
            prns_config::ConfigDiagnosticCode::PersistedRuntimeMetadata
                | prns_config::ConfigDiagnosticCode::UnknownKey
        ) {
            continue;
        }
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
                    let key = InterfaceConfigKey::new(key)
                        .map_err(InterfacesError::InterfaceConfigKey)?;
                    edits.push(ConfigEdit::RemoveInterfaceValue { name, key });
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
    session: Option<&mut ApplySession>,
) -> Result<u8, InterfacesError> {
    mutate_loaded(
        load(config)?,
        show_secrets,
        edit,
        mutation,
        interactive,
        session,
    )
}

fn mutate_loaded(
    file: ConfigFile,
    show_secrets: bool,
    edit: ConfigEdit,
    mutation: MutationArgs,
    interactive: bool,
    mut session: Option<&mut ApplySession>,
) -> Result<u8, InterfacesError> {
    let edited = file
        .document()
        .edit_named(file.path().display().to_string(), &edit)
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
    let digest = config_digest(edited.candidate().as_bytes());
    if let Some(session) = session.as_mut() {
        session.saved(digest);
    }
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
        let result = apply_path(receipt.path());
        if result.is_ok() {
            if let Some(session) = session.as_mut() {
                session.applied(digest);
            }
        }
        result
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
    let presentation = Presentation::new(crate::terminal::enabled(io::stdout().is_terminal()));
    let mut session = ApplySession::default();
    loop {
        let file = load(config)?;
        let digest = config_digest(file.document().source().as_bytes());
        let validation = ValidationState::from_result(parse_and_plan_named(
            file.path().display().to_string(),
            file.document().source(),
        ));
        let interfaces = file.document().interfaces();
        print!(
            "{}",
            presentation.main_screen(
                file.path(),
                &interfaces,
                &validation,
                managed_runtime_status(),
                session.status(digest),
            )
        );
        let selection = prompt("Selection")?;
        match selection.trim().to_ascii_lowercase().as_str() {
            "a" | "add" => {
                let kind = prompt_kind()?;
                let name = InterfaceName::new(prompt("Interface name")?)
                    .map_err(InterfacesError::InterfaceName)?;
                if let Some(definition) =
                    guided::add_interface(kind, name, show_secrets, Vec::new(), Vec::new())?
                {
                    mutate(
                        config,
                        show_secrets,
                        ConfigEdit::Add(definition),
                        MutationArgs {
                            dry_run: false,
                            apply: false,
                        },
                        true,
                        Some(&mut session),
                    )?;
                }
            }
            "v" | "validate" | "c" | "check" => {
                let code = validate(config, show_secrets, false)?;
                if code != 0 {
                    println!(
                        "{}",
                        presentation.warning("Use Repair to correct the configuration.")
                    );
                }
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
                    Some(&mut session),
                )?;
            }
            "p" | "apply" => {
                apply(config)?;
                session.applied(digest);
            }
            "q" | "quit" | "" => return Ok(0),
            value => guided_interface(config, show_secrets, &file, value, &mut session)?,
        }
    }
}

fn guided_interface(
    config: Option<&Path>,
    show_secrets: bool,
    file: &ConfigFile,
    value: &str,
    session: &mut ApplySession,
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
    let presentation = Presentation::new(crate::terminal::enabled(io::stdout().is_terminal()));
    print!("{}", presentation.interface_header(selected));
    println!("[S] Settings  [N] Rename  [E] Enable  [D] Disable");
    println!("[R] Remove    [B] Back");
    let action = prompt("Action")?;
    let name = selected.name().as_str().to_string();
    let mutation = MutationArgs {
        dry_run: false,
        apply: false,
    };
    match action.trim().to_ascii_lowercase().as_str() {
        "s" | "settings" | "edit" => {
            if let Some(edit) = guided::edit_interface(file, selected, show_secrets)? {
                mutate(config, show_secrets, edit, mutation, true, Some(session))?;
            }
        }
        "n" | "rename" => {
            let replacement = InterfaceName::new(prompt("New interface name")?)
                .map_err(InterfacesError::InterfaceName)?;
            mutate(
                config,
                show_secrets,
                ConfigEdit::Rename {
                    current: selected.name().clone(),
                    replacement,
                },
                mutation,
                true,
                Some(session),
            )?;
        }
        "e" | "enable" => {
            set_enabled(
                config,
                show_secrets,
                NameArgs {
                    name: Some(name.clone()),
                    mutation,
                },
                true,
                MutationMode::Guided,
                Some(session),
            )?;
        }
        "d" | "disable" => {
            set_enabled(
                config,
                show_secrets,
                NameArgs {
                    name: Some(name.clone()),
                    mutation,
                },
                false,
                MutationMode::Guided,
                Some(session),
            )?;
        }
        "r" | "remove" => {
            remove_interface(
                config,
                show_secrets,
                RemoveArgs {
                    name: Some(name),
                    yes: false,
                    mutation,
                },
                MutationMode::Guided,
                Some(session),
            )?;
        }
        "b" | "back" | "" => {}
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
    println!();
    println!("Interface types");
    for (index, canonical) in InterfaceKind::CANONICAL_NAMES.iter().enumerate() {
        if let Some(kind) = InterfaceKind::parse(canonical) {
            println!(
                "  {:>2}. {:<24} {}",
                index + 1,
                friendly_kind(kind),
                canonical
            );
        }
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
    InterfaceKind::parse_cli(&value).ok_or(InterfacesError::Usage(
        InterfacesUsageError::UnknownInterfaceType(value),
    ))
}

fn prompt(label: &str) -> Result<String, InterfacesError> {
    let presentation = Presentation::new(crate::terminal::enabled(io::stdout().is_terminal()));
    print!("{}: ", presentation.prompt(label));
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

fn managed_runtime_status() -> RuntimeStatus {
    let Ok(paths) = ServicePaths::discover() else {
        return RuntimeStatus::Unavailable;
    };
    match managed_running(&paths) {
        Ok(Some(record)) if record.state == ServiceState::Running => RuntimeStatus::Running,
        Ok(Some(_)) => RuntimeStatus::Starting,
        Ok(None) => RuntimeStatus::Stopped,
        Err(_) => RuntimeStatus::Unavailable,
    }
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

#[derive(Default)]
struct ApplySession {
    saved_digest: Option<[u8; 32]>,
    applied_digest: Option<[u8; 32]>,
}

impl ApplySession {
    fn saved(&mut self, digest: [u8; 32]) {
        self.saved_digest = Some(digest);
    }

    fn applied(&mut self, digest: [u8; 32]) {
        self.saved_digest = Some(digest);
        self.applied_digest = Some(digest);
    }

    fn status(&self, digest: [u8; 32]) -> ApplyStatus {
        if self.applied_digest == Some(digest) {
            ApplyStatus::Current
        } else if self.saved_digest == Some(digest) {
            ApplyStatus::Pending
        } else {
            ApplyStatus::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApplySession, ApplyStatus};

    #[test]
    fn apply_session_tracks_saved_applied_and_externally_changed_digests() {
        let first = prnsd_control::config_digest(b"first");
        let second = prnsd_control::config_digest(b"second");
        let mut session = ApplySession::default();

        assert_eq!(session.status(first), ApplyStatus::Unknown);
        session.saved(first);
        assert_eq!(session.status(first), ApplyStatus::Pending);
        session.applied(first);
        assert_eq!(session.status(first), ApplyStatus::Current);
        assert_eq!(session.status(second), ApplyStatus::Unknown);
        session.saved(second);
        assert_eq!(session.status(second), ApplyStatus::Pending);
    }
}
