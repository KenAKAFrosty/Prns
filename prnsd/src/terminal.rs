use std::env;

pub(crate) type Rgb = (u8, u8, u8);

pub(crate) const ACCENT: Rgb = (0x6e, 0xe7, 0xb7);
pub(crate) const ACCENT_STRONG: Rgb = (0x34, 0xd3, 0x99);
pub(crate) const PAPER: Rgb = (0xf4, 0xf6, 0xfa);
pub(crate) const MUTED: Rgb = (0x6c, 0x74, 0x80);
pub(crate) const WARNING: Rgb = (0xfb, 0xbf, 0x24);
pub(crate) const ERROR: Rgb = (0xfb, 0x71, 0x85);
pub(crate) const PROMPT: Rgb = (0x67, 0xe8, 0xf9);
pub(crate) const RESET: &str = "\x1b[0m";

pub(crate) fn enabled(is_terminal: bool) -> bool {
    enabled_with(
        env::var_os("NO_COLOR").is_some(),
        env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0"),
        is_terminal,
    )
}

fn enabled_with(no_color: bool, force_color: bool, is_terminal: bool) -> bool {
    if no_color {
        return false;
    }
    if force_color {
        return true;
    }
    is_terminal
}

pub(crate) fn foreground(color: Rgb) -> String {
    if truecolor_capable() {
        format!("\x1b[38;2;{};{};{}m", color.0, color.1, color.2)
    } else {
        format!("\x1b[38;5;{}m", nearest_xterm256(color))
    }
}

pub(crate) fn paint(text: impl AsRef<str>, color: Rgb, styled: bool) -> String {
    if styled {
        format!("{}{}{}", foreground(color), text.as_ref(), RESET)
    } else {
        text.as_ref().to_string()
    }
}

pub(crate) fn bold(text: impl AsRef<str>, styled: bool) -> String {
    if styled {
        format!("\x1b[1m{}{}", text.as_ref(), RESET)
    } else {
        text.as_ref().to_string()
    }
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

#[cfg(test)]
mod tests {
    use super::enabled_with;

    #[test]
    fn no_color_wins_and_forcing_color_overrides_redirection() {
        assert!(!enabled_with(true, true, true));
        assert!(!enabled_with(false, false, false));
        assert!(enabled_with(false, true, false));
        assert!(enabled_with(false, false, true));
    }
}
