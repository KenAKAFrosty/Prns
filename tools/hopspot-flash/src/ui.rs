use std::env;
use std::io::{self, IsTerminal, Write};

use dialoguer::{theme::ColorfulTheme, Select};

type Rgb = (u8, u8, u8);

const ACCENT_STRONG: Rgb = (0x34, 0xd3, 0x99);
const SOFT: Rgb = (0x9a, 0xa3, 0xb2);
const MUTED: Rgb = (0x6c, 0x74, 0x80);
const RESET: &str = "\x1b[0m";

pub fn interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn print_header() {
    crate::splash::print("Personal Hopspot flasher · build, flash, and field-recover firmware");
}

pub fn print_section(title: &str) {
    if fancy() {
        println!("{}{}{}", fg(ACCENT_STRONG), title, RESET);
    } else {
        println!("{title}");
    }
}

pub fn print_note(message: &str) {
    if fancy() {
        println!("{}{}{}", fg(SOFT), message, RESET);
    } else {
        println!("{message}");
    }
}

pub fn print_key_value(key: &str, value: &str) {
    if fancy() {
        println!("  {}{key:<12}{} {value}", fg(MUTED), RESET);
    } else {
        println!("  {key:<12} {value}");
    }
}

pub fn pause(prompt: &str) -> Result<(), String> {
    if !interactive_terminal() {
        return Ok(());
    }

    print!("{prompt}");
    io::stdout()
        .flush()
        .map_err(|err| format!("failed to flush stdout: {err}"))?;

    let mut input = String::new();
    let bytes_read = io::stdin()
        .read_line(&mut input)
        .map_err(|err| format!("failed to read input: {err}"))?;
    if bytes_read == 0 {
        return Err("no input received".to_string());
    }
    Ok(())
}

pub fn status_chip(label: &str, ready: bool) -> String {
    if !fancy() {
        return label.to_string();
    }
    let color = if ready { ACCENT_STRONG } else { MUTED };
    format!("{}{}{}", fg(color), label, RESET)
}

pub fn select(prompt: &str, choices: &[String], default: usize) -> Result<Option<usize>, String> {
    if interactive_terminal() {
        let theme = ColorfulTheme::default();
        Select::with_theme(&theme)
            .with_prompt(prompt)
            .items(choices)
            .default(default)
            .interact_opt()
            .map_err(|err| format!("failed to read selection: {err}"))
    } else {
        select_numbered(prompt, choices, default)
    }
}

fn select_numbered(
    prompt: &str,
    choices: &[String],
    default: usize,
) -> Result<Option<usize>, String> {
    println!("{prompt}");
    for (index, choice) in choices.iter().enumerate() {
        let marker = if index == default { "*" } else { " " };
        println!("  {} {}. {}", marker, index + 1, choice);
    }
    print!("Select [{}]: ", default + 1);
    io::stdout()
        .flush()
        .map_err(|err| format!("failed to flush stdout: {err}"))?;

    let mut input = String::new();
    let bytes_read = io::stdin()
        .read_line(&mut input)
        .map_err(|err| format!("failed to read choice: {err}"))?;
    if bytes_read == 0 {
        return Err("no input received; use a subcommand for noninteractive use".to_string());
    }

    let input = input.trim();
    if input.is_empty() {
        return Ok(Some(default));
    }
    if input.eq_ignore_ascii_case("q") || input.eq_ignore_ascii_case("quit") {
        return Ok(None);
    }

    let selected = input
        .parse::<usize>()
        .map_err(|_| format!("{input:?} is not a number"))?;
    if selected == 0 || selected > choices.len() {
        return Err(format!("choice must be between 1 and {}", choices.len()));
    }
    Ok(Some(selected - 1))
}

fn fancy() -> bool {
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0") {
        return true;
    }
    io::stdout().is_terminal()
}

fn truecolor_capable() -> bool {
    env::var("COLORTERM").is_ok_and(|value| value.contains("truecolor") || value.contains("24bit"))
}

fn nearest_xterm256(color: Rgb) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |channel: u8| -> u8 {
        let mut best = 0u8;
        let mut best_distance = u16::MAX;
        for (index, level) in LEVELS.iter().enumerate() {
            let distance = channel.abs_diff(*level) as u16;
            if distance < best_distance {
                best_distance = distance;
                best = index as u8;
            }
        }
        best
    };
    16 + 36 * nearest(color.0) + 6 * nearest(color.1) + nearest(color.2)
}

fn fg(color: Rgb) -> String {
    if truecolor_capable() {
        format!("\x1b[38;2;{};{};{}m", color.0, color.1, color.2)
    } else {
        format!("\x1b[38;5;{}m", nearest_xterm256(color))
    }
}
