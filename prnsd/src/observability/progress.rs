use std::io::{self, Write};
use std::time::{Duration, Instant};

use personal_rns::runtime::RouteSeedProgress;

const BAR_WIDTH: usize = 24;
const MIN_VISIBLE_TIME: Duration = Duration::from_millis(500);

pub struct StateRestoreProgress {
    started: Instant,
    latest: RouteSeedProgress,
    last_percent: Option<u32>,
    visible: bool,
    finished: bool,
}

impl StateRestoreProgress {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            latest: RouteSeedProgress::default(),
            last_percent: None,
            visible: false,
            finished: false,
        }
    }

    pub fn observe(&mut self, progress: RouteSeedProgress) {
        self.latest = progress;
        if progress.total_count == 0 {
            return;
        }
        let percent = percentage(progress);
        if self.last_percent == Some(percent) {
            return;
        }
        self.last_percent = Some(percent);
        if !self.visible {
            if self.started.elapsed() < MIN_VISIBLE_TIME
                || progress.processed_count >= progress.total_count
            {
                return;
            }
            self.visible = true;
        }
        draw_line(&render_line(progress, BAR_WIDTH), false);
    }

    pub fn finish(mut self) {
        if self.visible {
            draw_line(&render_line(self.latest, BAR_WIDTH), true);
        }
        self.finished = true;
    }
}

impl Drop for StateRestoreProgress {
    fn drop(&mut self) {
        if self.visible && !self.finished {
            clear_line();
        }
    }
}

fn percentage(progress: RouteSeedProgress) -> u32 {
    if progress.total_count == 0 {
        return 0;
    }
    let processed = u64::from(progress.processed_count.min(progress.total_count));
    ((processed * 100) / u64::from(progress.total_count)) as u32
}

fn render_line(progress: RouteSeedProgress, width: usize) -> String {
    let processed = progress.processed_count.min(progress.total_count);
    let filled = if progress.total_count == 0 {
        0
    } else {
        ((processed as u64 * width as u64) / u64::from(progress.total_count)) as usize
    };
    let bar = std::format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
    let percent = percentage(progress);
    std::format!(
        "  Validating saved routes [{bar}] {percent:>3}% ({processed}/{})",
        progress.total_count
    )
}

fn clear_line() {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(b"\r\x1b[2K");
    let _ = stderr.flush();
}

fn draw_line(line: &str, newline: bool) {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(b"\r\x1b[2K");
    let _ = stderr.write_all(line.as_bytes());
    if newline {
        let _ = stderr.write_all(b"\n");
    }
    let _ = stderr.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_tracks_real_route_progress() {
        assert_eq!(
            render_line(
                RouteSeedProgress {
                    processed_count: 2,
                    total_count: 4,
                },
                4,
            ),
            "  Validating saved routes [██░░]  50% (2/4)"
        );
        assert_eq!(
            render_line(
                RouteSeedProgress {
                    processed_count: 4,
                    total_count: 4,
                },
                4,
            ),
            "  Validating saved routes [████] 100% (4/4)"
        );
    }
}
