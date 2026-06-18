use std::env;
use std::io::IsTerminal;

type Rgb = (u8, u8, u8);

const ACCENT: Rgb = (0x6e, 0xe7, 0xb7);
const ACCENT_STRONG: Rgb = (0x34, 0xd3, 0x99);
const PAPER: Rgb = (0xf4, 0xf6, 0xfa);
const SOFT: Rgb = (0x6c, 0x74, 0x80);

const CANVAS_ROWS: usize = 28;
const MARK_DOTS: usize = 27;
const GUTTER_DOTS: usize = 5;
const CAP_TOP: f32 = 4.0;
const X_TOP: f32 = 13.5;
const BASELINE: f32 = 24.0;
const NAME: &str = "Prns";
const RESET: &str = "\x1b[0m";

struct Glyph {
    advance: usize,
    color: Rgb,
    covers: fn(f32, f32) -> bool,
}

fn glyph(letter: char) -> Glyph {
    match letter {
        'P' => Glyph {
            advance: 12,
            color: ACCENT,
            covers: p_covers,
        },
        'r' => Glyph {
            advance: 10,
            color: PAPER,
            covers: r_covers,
        },
        'n' => Glyph {
            advance: 14,
            color: PAPER,
            covers: n_covers,
        },
        's' => Glyph {
            advance: 10,
            color: PAPER,
            covers: s_covers,
        },
        _ => Glyph {
            advance: 10,
            color: PAPER,
            covers: |_, _| false,
        },
    }
}

fn in_band(dx: f32, dy: f32, inner: f32, outer: f32) -> bool {
    let distance = (dx * dx + dy * dy).sqrt();
    (inner..=outer).contains(&distance)
}

fn p_covers(x: f32, y: f32) -> bool {
    if (0.0..3.0).contains(&x) && (CAP_TOP..=BASELINE).contains(&y) {
        return true;
    }
    if (0.0..5.0).contains(&x) && ((4.0..7.0).contains(&y) || (12.0..15.0).contains(&y)) {
        return true;
    }
    let dx = x - 5.0;
    let dy = y - 9.5;
    dx >= 0.0 && in_band(dx, dy, 2.5, 5.5)
}

fn r_covers(x: f32, y: f32) -> bool {
    if (0.0..3.0).contains(&x) && (X_TOP..=BASELINE).contains(&y) {
        return true;
    }
    let dx = x - 3.0;
    let dy = y - (X_TOP + 5.0);
    dx >= 0.0 && dy <= 0.0 && in_band(dx, dy, 2.0, 5.0)
}

fn n_covers(x: f32, y: f32) -> bool {
    if (0.0..3.0).contains(&x) && (X_TOP..=BASELINE).contains(&y) {
        return true;
    }
    if (8.0..11.0).contains(&x) && (X_TOP + 5.0..=BASELINE).contains(&y) {
        return true;
    }
    let dx = x - 5.5;
    let dy = y - (X_TOP + 5.5);
    dy <= 0.0 && in_band(dx, dy, 2.5, 5.5)
}

fn s_covers(x: f32, y: f32) -> bool {
    let bar = 2.2;
    if (1.0..9.0).contains(&x) && (X_TOP..X_TOP + bar).contains(&y) {
        return !(x < 2.4 && y < X_TOP + 0.9);
    }
    if (0.0..bar).contains(&x) && (X_TOP + 0.9..X_TOP + 5.2).contains(&y) {
        return true;
    }
    if (1.0..8.0).contains(&x) && (X_TOP + 4.1..X_TOP + 4.1 + bar).contains(&y) {
        return true;
    }
    if (9.0 - bar..9.0).contains(&x) && (X_TOP + 5.2..BASELINE - 1.0).contains(&y) {
        return true;
    }
    (0.0..8.0).contains(&x) && (BASELINE - 2.1..=BASELINE).contains(&y)
}

fn mark_dot(x: usize, y: usize) -> Option<Rgb> {
    let center = (MARK_DOTS as f32 - 1.0) / 2.0;
    let dx = x as f32 - center;
    let dy = y as f32 - center;
    let distance = (dx * dx + dy * dy).sqrt();

    if distance <= 2.5 {
        return Some(ACCENT_STRONG);
    }
    if (9.3..=10.7).contains(&distance) {
        return Some(ACCENT);
    }
    let on_tick =
        ((dx - dy).abs() <= 0.9 || (dx + dy).abs() <= 0.9) && (11.6..=15.5).contains(&distance);
    if on_tick {
        return Some(ACCENT);
    }
    let in_wave_cone = dx.abs() >= 0.7 * distance;
    if in_wave_cone && (4.6..=6.2).contains(&distance) {
        return Some(ACCENT);
    }
    None
}

fn canvas() -> Vec<Vec<Option<Rgb>>> {
    let letters: Vec<Glyph> = NAME.chars().map(glyph).collect();
    let text_dots: usize = letters.iter().map(|g| g.advance).sum();
    let width = MARK_DOTS + GUTTER_DOTS + text_dots;
    let mut grid = vec![vec![None; width]; CANVAS_ROWS];

    for (y, row) in grid.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate().take(MARK_DOTS) {
            if y < MARK_DOTS {
                *cell = mark_dot(x, y);
            }
        }
    }

    let mut origin = MARK_DOTS + GUTTER_DOTS;
    for letter in &letters {
        for (y, row) in grid.iter_mut().enumerate() {
            for local_x in 0..letter.advance {
                if (letter.covers)(local_x as f32, y as f32) {
                    row[origin + local_x] = Some(letter.color);
                }
            }
        }
        origin += letter.advance;
    }
    grid
}

const DOT_BITS: [[u32; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

fn braille_lines(grid: &[Vec<Option<Rgb>>]) -> Vec<String> {
    let width = grid[0].len();
    let cell_cols = width.div_ceil(2);
    let cell_rows = grid.len().div_ceil(4);
    let mut lines = Vec::new();
    for cell_row in 0..cell_rows {
        let mut line = String::new();
        for cell_col in 0..cell_cols {
            let mut bits = 0u32;
            let mut color: Option<Rgb> = None;
            for (sub_row, row_bits) in DOT_BITS.iter().enumerate() {
                for (sub_col, bit) in row_bits.iter().enumerate() {
                    let y = cell_row * 4 + sub_row;
                    let x = cell_col * 2 + sub_col;
                    if y < grid.len() && x < width {
                        if let Some(dot_color) = grid[y][x] {
                            bits |= bit;
                            color.get_or_insert(dot_color);
                        }
                    }
                }
            }
            match color {
                Some(cell_color) if bits != 0 => {
                    line.push_str(&foreground(cell_color));
                    line.push(char::from_u32(0x2800 + bits).expect("braille range"));
                    line.push_str(RESET);
                }
                _ => line.push(' '),
            }
        }
        lines.push(line);
    }
    lines
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

fn foreground(color: Rgb) -> String {
    if truecolor_capable() {
        format!("\x1b[38;2;{};{};{}m", color.0, color.1, color.2)
    } else {
        format!("\x1b[38;5;{}m", nearest_xterm256(color))
    }
}

fn fancy() -> bool {
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if env::var_os("CLICOLOR_FORCE").is_some_and(|value| value != "0") {
        return true;
    }
    std::io::stderr().is_terminal()
}

pub fn print(subtitle: &str) {
    if !fancy() {
        eprintln!("{NAME} — {subtitle}");
        eprintln!();
        return;
    }

    eprintln!();
    for line in braille_lines(&canvas()) {
        eprintln!("  {line}");
    }
    eprintln!();
    eprintln!("  {}{subtitle}{RESET}", foreground(SOFT));
    eprintln!();
}
