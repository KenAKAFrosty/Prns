#![cfg_attr(not(feature = "std"), no_std)]
#![doc = "Thin host seam for driving the Personal Reticulum engine."]

use personal_rns::engine::{tick, DeltaMillis, Input, InstantMillis, State};

/// Platform body for the pure Reticulum engine.
pub trait Host {
    type Error;

    fn now_millis(&mut self) -> Result<InstantMillis, Self::Error>;

    fn receive_packet(&mut self, buffer: &mut [u8]) -> Result<Option<usize>, Self::Error>;

    fn transmit_packet(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
}

/// Drive the pure engine once using caller-owned host I/O buffers.
pub fn drive_once<H: Host>(
    state: &mut State,
    host: &mut H,
    buffer: &mut [u8],
    dt: DeltaMillis,
) -> Result<personal_rns::engine::Effects, H::Error> {
    let now = host.now_millis()?;
    let input = match host.receive_packet(buffer)? {
        Some(len) => Input::InboundPacket {
            now,
            bytes: &buffer[..len],
        },
        None => Input::Idle { now },
    };

    Ok(tick(state, input, dt))
}

#[cfg(test)]
mod tests {
    use super::{drive_once, Host};
    use personal_rns::engine::{DeltaMillis, InstantMillis, State};

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
        let mut state = State::default();
        let mut host = EmptyHost;
        let mut buffer = [0u8; 16];

        let effects = drive_once(&mut state, &mut host, &mut buffer, DeltaMillis(1)).unwrap();

        assert_eq!(state.ticks(), 1);
        assert_eq!(effects.emitted_packets(), 0);
    }
}
