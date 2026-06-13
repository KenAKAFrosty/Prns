use std::fmt;
use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

mod splash;

#[derive(Parser)]
#[command(version, about = "Reconciliation-gate smoke: clap + inquire + indicatif")]
struct Cli {
    #[arg(long, env = "PRNSD_PREFER", value_enum)]
    prefer: Option<ConfigSource>,

    #[arg(long)]
    non_interactive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ConfigSource {
    Toml,
    Reference,
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigSource::Toml => f.write_str("config.toml"),
            ConfigSource::Reference => f.write_str("reference config"),
        }
    }
}

fn resolve_divergence(cli: &Cli) -> Result<ConfigSource, String> {
    if let Some(source) = cli.prefer {
        return Ok(source);
    }
    if cli.non_interactive || !std::io::stdin().is_terminal() {
        return Err(String::from(
            "config and config.toml diverge; pass --prefer <toml|reference> or set PRNSD_PREFER to resolve headlessly",
        ));
    }
    inquire::Select::new(
        "config and config.toml diverge — which should win?",
        vec![ConfigSource::Toml, ConfigSource::Reference],
    )
    .prompt()
    .map_err(|prompt_error| prompt_error.to_string())
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    splash::print(concat!("Prnsd · daemon gate smoke v", env!("CARGO_PKG_VERSION")));
    eprintln!(
        "  {} found ~/.reticulum/config (reference format)",
        style("✔").green()
    );
    eprintln!("  {} found ~/.reticulum/config.toml", style("✔").green());
    eprintln!(
        "  {} the two diverge on interfaces[0].bitrate",
        style("!").yellow()
    );

    let source = match resolve_divergence(&cli) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("  {} {message}", style("✘").red());
            return ExitCode::FAILURE;
        }
    };
    eprintln!("  {} using {source}", style("✔").green());

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(ProgressStyle::with_template("  {spinner} {msg}").expect("static template parses"));
    spinner.set_message("starting engine");
    spinner.enable_steady_tick(Duration::from_millis(80));
    std::thread::sleep(Duration::from_millis(1200));
    spinner.finish_and_clear();
    eprintln!("  {} engine online", style("✔").green());

    println!("SMOKE_DECISION source={source}");
    ExitCode::SUCCESS
}
