//! Tokio multi-threaded runtime driver for the engine.
//!
//! Drives the synchronous engine step on a spawned task under a multi-thread
//! Tokio runtime, so Send-correctness is structural: this only compiles if
//! `EngineState` and the driver are `Send`. Tokio stays outside the engine.

use std::time::Duration;

use personal_rns::engine::{FixedCapacityEngineState, EngineDriver};

use crate::StdEngineDriver;

/// Drive the engine on a multi-thread tokio runtime until `ticks` have elapsed,
/// returning the final tick count. Bounded form for smoking; the unbounded
/// daemon loop arrives with transport.
pub fn run_multi_thread(ticks: u64, poll: Duration) -> u64 {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .expect("tokio multi-thread runtime builds");

    runtime.block_on(async move {
        tokio::spawn(async move {
            let mut state: FixedCapacityEngineState = FixedCapacityEngineState::default();
            let mut driver = StdEngineDriver::new();
            while state.tick_count() < ticks {
                driver
                    .step(&mut state)
                    .expect("clock-only step cannot fail");
                tokio::time::sleep(poll).await;
            }
            state.tick_count()
        })
        .await
        .expect("engine task joins")
    })
}

#[cfg(test)]
mod tests {
    use super::run_multi_thread;
    use std::time::Duration;

    #[test]
    fn engine_runs_under_multi_thread_tokio() {
        assert_eq!(run_multi_thread(5, Duration::from_millis(1)), 5);
    }
}
