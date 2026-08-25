#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::capabilities::positioning::gnss::{GnssFix, GnssSnapshot, NmeaParser};
use prns_core::capabilities::positioning::GeographicPosition;

const KNOWN_FIX_SENTENCE: &[u8] =
    b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n";
fn known_fix() -> GnssSnapshot {
    let position = GeographicPosition::try_from_e7(481_173_000, 115_166_666, Some(545_400))
        .expect("known position is in range");
    GnssSnapshot::Fixed(GnssFix::new(position, 8, Some(90)))
}

fn assert_snapshot_invariants(snapshot: GnssSnapshot) {
    match snapshot {
        GnssSnapshot::Fixed(fix) => {
            let position = fix.position();
            let latitude = position.latitude().get();
            let longitude = position.longitude().get();
            assert!(latitude.unsigned_abs() <= 900_000_000);
            assert!(longitude.unsigned_abs() <= 1_800_000_000);
        }
        GnssSnapshot::Searching { .. } => {}
        GnssSnapshot::Disabled | GnssSnapshot::Starting | GnssSnapshot::Error => {
            panic!("the NMEA parser emitted a lifecycle-only status")
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut first = NmeaParser::new();
    let mut second = NmeaParser::new();

    for byte in data.iter().copied() {
        let first_snapshot = first.feed(byte);
        let second_snapshot = second.feed(byte);
        assert_eq!(first_snapshot, second_snapshot);
        if let Some(snapshot) = first_snapshot {
            assert_snapshot_invariants(snapshot);
        }
    }

    // A fresh '$' is an unconditional synchronization point. Arbitrary UART noise, partial
    // sentences, and overflow recovery must never poison the following valid observation.
    let mut recovered = false;
    for byte in KNOWN_FIX_SENTENCE.iter().copied() {
        if let Some(snapshot) = first.feed(byte) {
            assert_snapshot_invariants(snapshot);
            recovered |= snapshot == known_fix();
        }
    }
    assert!(recovered);
});
