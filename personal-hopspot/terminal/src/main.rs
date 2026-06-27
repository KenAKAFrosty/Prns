use std::env;
use std::io::{self, IsTerminal, Write as _};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use embedded_graphics::geometry::{OriginDimensions, Size};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::Pixel;
use personal_hopspot_ui::{
    card_label, draw_with_state, BatteryState, Card, CardKind, InputEvent, Liveness, UiState,
};
use personal_rns::interfaces::InterfaceId;

const PANEL_WIDTH: usize = 64;
const PANEL_HEIGHT: usize = 128;
const PIXEL_COUNT: usize = PANEL_WIDTH * PANEL_HEIGHT;

const LIT_FG: &str = "\x1b[38;2;74;158;255m";
const LIT_BG: &str = "\x1b[48;2;74;158;255m";
const DARK_FG: &str = "\x1b[38;2;0;6;26m";
const DARK_BG: &str = "\x1b[48;2;0;6;26m";
const RESET: &str = "\x1b[0m";

#[derive(Clone, Copy)]
struct Options {
    frames: Option<usize>,
    delay: Duration,
    color: bool,
    mode: RenderMode,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut options = Self {
            frames: if io::stdout().is_terminal() {
                None
            } else {
                Some(1)
            },
            delay: Duration::from_millis(120),
            color: env::var_os("NO_COLOR").is_none(),
            mode: RenderMode::Half,
        };

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--once" => options.frames = Some(1),
                "--plain" => options.color = false,
                "--frames" => {
                    let value = args
                        .next()
                        .ok_or_else(|| String::from("--frames needs a number"))?;
                    options.frames = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid frame count: {value}"))?,
                    );
                }
                "--delay-ms" => {
                    let value = args
                        .next()
                        .ok_or_else(|| String::from("--delay-ms needs a number"))?;
                    let millis = value
                        .parse()
                        .map_err(|_| format!("invalid delay: {value}"))?;
                    options.delay = Duration::from_millis(millis);
                }
                "--mode" => {
                    let value = args
                        .next()
                        .ok_or_else(|| String::from("--mode needs auto, half, or braille"))?;
                    options.mode = RenderMode::parse(&value)?;
                }
                "--help" | "-h" => {
                    return Err(String::from(
                        "usage: personal-hopspot-terminal [--once] [--plain] [--frames N] [--delay-ms N] [--mode auto|half|braille]",
                    ));
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        Ok(options)
    }
}

#[derive(Clone, Copy)]
enum RenderMode {
    Auto,
    Half,
    Braille,
}

impl RenderMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "half" => Ok(Self::Half),
            "braille" => Ok(Self::Braille),
            other => Err(format!("unknown mode: {other}")),
        }
    }

    fn resolve(self, columns: u16, rows: u16) -> PackedMode {
        match self {
            Self::Auto if columns >= 64 && rows >= 66 => PackedMode::Half,
            Self::Auto => PackedMode::Braille,
            Self::Half => PackedMode::Half,
            Self::Braille => PackedMode::Braille,
        }
    }
}

#[derive(Clone, Copy)]
enum PackedMode {
    Half,
    Braille,
}

impl PackedMode {
    fn label(self) -> &'static str {
        match self {
            Self::Half => "half-block",
            Self::Braille => "braille",
        }
    }
}

struct TerminalFrame {
    lit: [bool; PIXEL_COUNT],
}

impl TerminalFrame {
    const fn new() -> Self {
        Self {
            lit: [false; PIXEL_COUNT],
        }
    }

    fn pixel(&self, x: usize, y: usize) -> bool {
        self.lit[y * PANEL_WIDTH + x]
    }

    fn render(&self, color: bool, mode: PackedMode) -> String {
        match mode {
            PackedMode::Half => self.render_half_ansi(color),
            PackedMode::Braille => self.render_braille_ansi(color),
        }
    }

    fn render_half_ansi(&self, color: bool) -> String {
        let mut out = String::with_capacity(PANEL_WIDTH * PANEL_HEIGHT);
        for y in (0..PANEL_HEIGHT).step_by(2) {
            for x in 0..PANEL_WIDTH {
                let top = self.pixel(x, y);
                let bottom = self.pixel(x, y + 1);
                match (top, bottom, color) {
                    (false, false, false) => out.push(' '),
                    (true, false, false) => out.push('▀'),
                    (false, true, false) => out.push('▄'),
                    (true, true, false) => out.push('█'),
                    (false, false, true) => {
                        out.push_str(DARK_BG);
                        out.push(' ');
                    }
                    (true, false, true) => {
                        if self.cell_prefers_lit_background(x, y) {
                            out.push_str(DARK_FG);
                            out.push_str(LIT_BG);
                            out.push('▄');
                        } else {
                            out.push_str(LIT_FG);
                            out.push_str(DARK_BG);
                            out.push('▀');
                        }
                    }
                    (false, true, true) => {
                        if self.cell_prefers_lit_background(x, y) {
                            out.push_str(DARK_FG);
                            out.push_str(LIT_BG);
                            out.push('▀');
                        } else {
                            out.push_str(LIT_FG);
                            out.push_str(DARK_BG);
                            out.push('▄');
                        }
                    }
                    (true, true, true) => {
                        out.push_str(LIT_BG);
                        out.push(' ');
                    }
                }
            }
            if color {
                out.push_str(RESET);
            }
            out.push('\n');
        }
        out
    }

    fn cell_prefers_lit_background(&self, x: usize, y: usize) -> bool {
        let x_start = x.saturating_sub(1);
        let x_end = (x + 1).min(PANEL_WIDTH - 1);
        let y_start = y.saturating_sub(2);
        let y_end = (y + 3).min(PANEL_HEIGHT - 1);
        let mut lit = 0;
        let mut total = 0;

        for yy in y_start..=y_end {
            for xx in x_start..=x_end {
                total += 1;
                if self.pixel(xx, yy) {
                    lit += 1;
                }
            }
        }

        lit * 2 >= total
    }

    fn render_braille_ansi(&self, color: bool) -> String {
        let mut out = String::with_capacity(PANEL_WIDTH * PANEL_HEIGHT / 4);
        for y in (0..PANEL_HEIGHT).step_by(4) {
            for x in (0..PANEL_WIDTH).step_by(2) {
                let bits = self.braille_bits(x, y);
                let glyph = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
                if color {
                    out.push_str(LIT_FG);
                    out.push_str(DARK_BG);
                }
                out.push(glyph);
            }
            if color {
                out.push_str(RESET);
            }
            out.push('\n');
        }
        out
    }

    fn braille_bits(&self, x: usize, y: usize) -> u8 {
        let mut bits = 0;
        for dy in 0..4 {
            for dx in 0..2 {
                if self.pixel(x + dx, y + dy) {
                    bits |= braille_dot(dx, dy);
                }
            }
        }
        bits
    }
}

fn braille_dot(x: usize, y: usize) -> u8 {
    match (x, y) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (1, 3) => 0x80,
        _ => 0,
    }
}

impl Default for TerminalFrame {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginDimensions for TerminalFrame {
    fn size(&self) -> Size {
        Size::new(PANEL_WIDTH as u32, PANEL_HEIGHT as u32)
    }
}

impl DrawTarget for TerminalFrame {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if (0..PANEL_WIDTH as i32).contains(&point.x)
                && (0..PANEL_HEIGHT as i32).contains(&point.y)
            {
                let index = point.y as usize * PANEL_WIDTH + point.x as usize;
                self.lit[index] = color.is_on();
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.lit.fill(color.is_on());
        Ok(())
    }
}

fn main() -> ExitCode {
    let options = match Options::parse() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return if message.starts_with("usage:") {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };

    if let Err(error) = run(options) {
        eprintln!("terminal render failed: {error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn run(options: Options) -> io::Result<()> {
    let mut stdout = io::stdout();
    let interactive = stdout.is_terminal() && options.frames != Some(1);
    let _session = if interactive {
        Some(TerminalSession::enter(&mut stdout)?)
    } else {
        None
    };

    let mut state = UiState::new();
    let mut frame_index = 0usize;
    loop {
        let mut frame = TerminalFrame::new();
        let cards = demo_cards(frame_index as u32);
        draw_with_state(&mut frame, &cards, BatteryState::Charging(75), &state);
        let (columns, rows) = terminal::size().unwrap_or((80, 24));
        let mode = options.mode.resolve(columns, rows);
        let rendered = frame.render(options.color, mode);

        if interactive {
            let rendered = raw_terminal_rows(&rendered);
            queue!(
                stdout,
                MoveTo(0, 0),
                Clear(ClearType::All),
                Print(rendered),
                Print(format!(
                    "{RESET}{} mode | q quit | space next | enter menu\r\n",
                    mode.label()
                ))
            )?;
        } else {
            stdout.write_all(rendered.as_bytes())?;
        }
        stdout.flush()?;

        if options
            .frames
            .is_some_and(|frames| frame_index + 1 >= frames)
        {
            break;
        }

        if interactive {
            if wait_for_input_or_tick(options.delay, &mut state, &cards)? {
                break;
            }
        } else {
            thread::sleep(options.delay);
        }

        if frame_index % 4 == 3 {
            let selected_kind = selected_card_kind(&state, &cards);
            let _ = state.handle_input(InputEvent::ShortPress, cards.len(), selected_kind);
        }
        frame_index = frame_index.wrapping_add(1);
    }

    Ok(())
}

fn raw_terminal_rows(rendered: &str) -> String {
    rendered.replace('\n', "\r\n")
}

struct TerminalSession;

impl TerminalSession {
    fn enter(stdout: &mut io::Stdout) -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(
            stdout,
            EnterAlternateScreen,
            Hide,
            Clear(ClearType::All),
            MoveTo(0, 0)
        )?;
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
    }
}

fn wait_for_input_or_tick(
    delay: Duration,
    state: &mut UiState,
    cards: &[Card],
) -> io::Result<bool> {
    if !event::poll(delay)? {
        return Ok(false);
    }

    loop {
        if let Event::Key(key) = event::read()? {
            if handle_key(key, state, cards) {
                return Ok(true);
            }
        }

        if !event::poll(Duration::ZERO)? {
            return Ok(false);
        }
    }
}

fn selected_card_kind(state: &UiState, cards: &[Card]) -> Option<CardKind> {
    state
        .selected_card(cards.len())
        .and_then(|index| cards.get(index))
        .map(|card| card.kind)
}

fn handle_key(key: KeyEvent, state: &mut UiState, cards: &[Card]) -> bool {
    if key.kind != KeyEventKind::Press {
        return false;
    }
    let selected_kind = selected_card_kind(state, cards);

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyCode::Char(' ') | KeyCode::Char('n') | KeyCode::Down | KeyCode::Right => {
            let _ = state.handle_input(InputEvent::ShortPress, cards.len(), selected_kind);
            false
        }
        KeyCode::Enter | KeyCode::Char('m') | KeyCode::Char('l') => {
            let _ = state.handle_input(InputEvent::LongPress, cards.len(), selected_kind);
            false
        }
        _ => false,
    }
}

fn demo_cards(tick: u32) -> [Card; 4] {
    let pulse = tick as u64;
    [
        Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Usb,
            label: card_label("USB"),
            selected: false,
            liveness: Liveness::Live,
            failure_reason: None,
            tx_bytes: 42_100 + pulse * 321,
            rx_bytes: 73_900 + pulse * 256,
            links: 1,
            destinations: 2,
            rate_bytes_per_sec: 1_200 + tick * 37,
            last_activity_secs: Some(tick % 5),
        },
        Card {
            id: InterfaceId::new([1; 8]),
            kind: CardKind::Wifi,
            label: card_label("WiFi"),
            selected: false,
            liveness: Liveness::Dormant,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        },
        Card {
            id: InterfaceId::new([2; 8]),
            kind: CardKind::Ble,
            label: card_label("BLE"),
            selected: false,
            liveness: if tick % 8 < 4 {
                Liveness::Failed
            } else {
                Liveness::Dormant
            },
            failure_reason: Some("BlueZ GATT Channels >1; set Channels=1"),
            tx_bytes: 12,
            rx_bytes: 8,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: Some(80 + tick),
        },
        Card {
            id: InterfaceId::new([3; 8]),
            kind: CardKind::LoRa,
            label: card_label("LoRa"),
            selected: false,
            liveness: Liveness::Live,
            failure_reason: None,
            tx_bytes: 9_900 + pulse * 17,
            rx_bytes: 21_000 + pulse * 19,
            links: 1,
            destinations: 1,
            rate_bytes_per_sec: 96,
            last_activity_secs: Some(35 + tick),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_half_render_keeps_one_terminal_cell_per_pixel_column() {
        let mut frame = TerminalFrame::new();
        draw_with_state(
            &mut frame,
            &demo_cards(0),
            BatteryState::Charging(75),
            &UiState::new(),
        );
        let rendered = frame.render(true, PackedMode::Half);

        for line in rendered.lines() {
            assert_eq!(visible_width(line), PANEL_WIDTH);
        }
    }

    #[test]
    fn color_braille_render_packs_the_panel_into_short_rows() {
        let mut frame = TerminalFrame::new();
        draw_with_state(
            &mut frame,
            &demo_cards(0),
            BatteryState::Charging(75),
            &UiState::new(),
        );
        let rendered = frame.render(true, PackedMode::Braille);

        assert_eq!(rendered.lines().count(), PANEL_HEIGHT / 4);
        for line in rendered.lines() {
            assert_eq!(visible_width(line), PANEL_WIDTH / 2);
        }
    }

    #[test]
    fn rendered_rows_use_crlf_for_raw_terminal_mode() {
        let frame = TerminalFrame::new();
        let rendered = raw_terminal_rows(&frame.render(false, PackedMode::Braille));

        assert!(rendered.contains("\r\n"));
        assert_eq!(
            rendered.bytes().filter(|byte| *byte == b'\n').count(),
            PANEL_HEIGHT / 4
        );
        assert_eq!(
            rendered.bytes().filter(|byte| *byte == b'\r').count(),
            PANEL_HEIGHT / 4
        );
    }

    fn visible_width(line: &str) -> usize {
        let mut width = 0;
        let mut chars = line.chars();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                width += 1;
                continue;
            }

            if chars.next() != Some('[') {
                continue;
            }

            for escape_ch in chars.by_ref() {
                if ('@'..='~').contains(&escape_ch) {
                    break;
                }
            }
        }
        width
    }
}
