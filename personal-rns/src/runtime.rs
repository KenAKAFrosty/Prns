//! Runtime driver: the provided loop that ticks the pure engine against a host.
//!
//! Defined in the core so every target body — daemon, microcontroller, SDK —
//! reuses one loop and supplies only a `Host`

use crate::engine::{tick, DeltaMillis, TickOutput, EngineState, TickInput};
use crate::host::Host;

pub fn drive_once<H: Host>(
    state: &mut EngineState,
    host: &mut H,
    buffer: &mut [u8],
    dt: DeltaMillis,
) -> Result<TickOutput, H::Error> {
    let now = host.now_millis()?;
    let input = match host.receive_packet(buffer)? {
        Some(len) => TickInput::InboundPacket {
            now,
            bytes: &buffer[..len],
        },
        None => TickInput::Idle { now },
    };

    Ok(tick(state, input, dt))
}

#[cfg(test)]
mod tests {
    use super::drive_once;
    use crate::engine::{DeltaMillis, EngineState, InstantMillis};
    use crate::host::Host;

    #[derive(Default)]
    struct EmptyHost;

    impl Host for EmptyHost {
        type Error = core::convert::Infallible;

        fn now_millis(&mut self) -> Result<InstantMillis, Self::Error> {
            Ok(InstantMillis(10))
        }

        fn receive_packet(&mut self, _buffer: &mut [u8]) -> Result<Option<usize>, Self::Error> {
            Ok(None)
        }

        fn transmit_packet(&mut self, _bytes: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn host_drives_one_engine_tick() {
        let mut state = EngineState::default();
        let mut host = EmptyHost;
        let mut buffer = [0u8; 16];

        let effects = drive_once(&mut state, &mut host, &mut buffer, DeltaMillis(1)).unwrap();

        assert_eq!(state.tick_count(), 1);
        assert_eq!(effects.emitted_packet_count(), 0);
    }
}
