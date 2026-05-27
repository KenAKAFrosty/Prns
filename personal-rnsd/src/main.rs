//! Linux smoke: drive the pure engine against a real monotonic clock via StdHost.

use personal_rns::engine::EngineState;
use personal_rns::host::Host;
use personal_rns::runtime::drive_once;
use personal_rnsd::StdHost;

fn main() {
    let mut state = EngineState::default();
    let mut host = StdHost::new();
    let mut buffer = [0u8; 64];

    // No transport yet, so every tick is Idle. This proves the engine advances
    // deterministically against a real monotonic clock on a real host; the
    // sleep lets wall-clock time actually move between ticks.
    let start = host.now_millis().expect("std clock is readable");
    for _ in 0..5 {
        drive_once(&mut state, &mut host, &mut buffer)
            .expect("clock-only drive_once cannot fail");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let end = host.now_millis().expect("std clock is readable");

    println!(
        "personal-rnsd: {} ticks; real monotonic clock advanced {} ms",
        state.tick_count(),
        end.0.saturating_sub(start.0)
    );
    println!("RNSD_SMOKE_OK");
}
