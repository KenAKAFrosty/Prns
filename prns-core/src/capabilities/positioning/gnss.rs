use super::GeographicPosition;

const NMEA_SENTENCE_BYTES: usize = 96;

/// A host command for a controllable GNSS receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GnssReceiverCommand {
    /// Power or wake the receiver and begin acquiring a fix.
    Enable,
    /// Stop acquisition and place the receiver in its board-defined disabled state.
    Disable,
}

/// Provider-specific quality metadata accompanying a valid GNSS position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GnssFix {
    position: GeographicPosition,
    satellites: u8,
    hdop_hundredths: Option<u16>,
}

impl GnssFix {
    /// Combine a validated position with GNSS-specific quality metadata.
    #[must_use]
    pub const fn new(
        position: GeographicPosition,
        satellites: u8,
        hdop_hundredths: Option<u16>,
    ) -> Self {
        Self {
            position,
            satellites,
            hdop_hundredths,
        }
    }

    /// Return the source-neutral geographic position.
    #[must_use]
    pub const fn position(self) -> GeographicPosition {
        self.position
    }

    /// Return the number of satellites used by the receiver.
    #[must_use]
    pub const fn satellites(self) -> u8 {
        self.satellites
    }

    /// Return horizontal dilution of precision times 100 when reported.
    #[must_use]
    pub const fn hdop_hundredths(self) -> Option<u16> {
        self.hdop_hundredths
    }
}

/// One coherent GNSS provider state. A fix carries a source-neutral [`GeographicPosition`]; the
/// lifecycle-only states cannot accidentally carry stale coordinates. The explicit discriminant
/// keeps `Disabled` all-zero so statically stored receiver state remains in BSS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum GnssSnapshot {
    Disabled,
    Starting,
    Searching { satellites: u8 },
    Fixed(GnssFix),
    Error,
}

impl GnssSnapshot {
    /// Satellite count reported by states for which the receiver supplies one.
    #[must_use]
    pub const fn satellites(self) -> Option<u8> {
        match self {
            Self::Searching { satellites } => Some(satellites),
            Self::Fixed(fix) => Some(fix.satellites()),
            Self::Disabled | Self::Starting | Self::Error => None,
        }
    }

    /// The valid fix carried by this snapshot, when positioning has completed.
    #[must_use]
    pub const fn fix(self) -> Option<GnssFix> {
        match self {
            Self::Fixed(fix) => Some(fix),
            Self::Disabled | Self::Starting | Self::Searching { .. } | Self::Error => None,
        }
    }
}

/// Incremental, allocation-free NMEA parser. It accepts any talker ID and currently publishes the
/// GGA fix/search fields common to NMEA GNSS receivers.
pub struct NmeaParser {
    sentence: [u8; NMEA_SENTENCE_BYTES],
    len: usize,
    collecting: bool,
}

impl NmeaParser {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sentence: [0; NMEA_SENTENCE_BYTES],
            len: 0,
            collecting: false,
        }
    }

    /// Feed one UART byte. A coherent snapshot is returned only after a checksummed GGA sentence.
    pub fn feed(&mut self, byte: u8) -> Option<GnssSnapshot> {
        if byte == b'$' {
            self.sentence[0] = byte;
            self.len = 1;
            self.collecting = true;
            return None;
        }
        if !self.collecting {
            return None;
        }
        if byte == b'\r' {
            return None;
        }
        if byte == b'\n' {
            self.collecting = false;
            let sentence = &self.sentence[..self.len];
            return valid_checksum(sentence)
                .then(|| parse_gga(sentence))
                .flatten();
        }
        self.push_sentence_byte(byte);
        None
    }

    fn push_sentence_byte(&mut self, byte: u8) {
        if self.len == self.sentence.len() {
            self.collecting = false;
            self.len = 0;
            return;
        }
        self.sentence[self.len] = byte;
        self.len += 1;
    }
}

impl Default for NmeaParser {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_gga(sentence: &[u8]) -> Option<GnssSnapshot> {
    if sentence.get(3..7) != Some(b"GGA,") {
        return None;
    }
    let satellites = match nmea_field(sentence, 7)? {
        [] => 0,
        field => parse_unsigned(field)?.min(u32::from(u8::MAX)) as u8,
    };
    let quality = parse_unsigned(nmea_field(sentence, 6)?)?;
    if quality == 0 {
        return Some(GnssSnapshot::Searching { satellites });
    }

    let latitude_e7 = parse_coordinate(
        nmea_field(sentence, 2)?,
        nmea_field(sentence, 3)?,
        CoordinateAxis::Latitude,
    )?;
    let longitude_e7 = parse_coordinate(
        nmea_field(sentence, 4)?,
        nmea_field(sentence, 5)?,
        CoordinateAxis::Longitude,
    )?;
    let altitude_mm = parse_signed_decimal_scaled(nmea_field(sentence, 9)?, 1_000);
    let hdop_hundredths = parse_decimal_scaled(nmea_field(sentence, 8)?, 100)
        .and_then(|value| u16::try_from(value).ok());
    let position = GeographicPosition::try_from_e7(latitude_e7, longitude_e7, altitude_mm).ok()?;
    Some(GnssSnapshot::Fixed(GnssFix::new(
        position,
        satellites,
        hdop_hundredths,
    )))
}

#[derive(Clone, Copy)]
enum CoordinateAxis {
    Latitude,
    Longitude,
}

#[derive(Clone, Copy)]
enum CoordinateSign {
    Positive,
    Negative,
}

impl CoordinateAxis {
    const fn degree_digits(self) -> usize {
        match self {
            Self::Latitude => 2,
            Self::Longitude => 3,
        }
    }

    const fn max_degrees(self) -> u32 {
        match self {
            Self::Latitude => 90,
            Self::Longitude => 180,
        }
    }

    fn sign(self, hemisphere: &[u8]) -> Option<CoordinateSign> {
        match (self, hemisphere) {
            (Self::Latitude, b"N") | (Self::Longitude, b"E") => Some(CoordinateSign::Positive),
            (Self::Latitude, b"S") | (Self::Longitude, b"W") => Some(CoordinateSign::Negative),
            _ => None,
        }
    }
}

fn parse_coordinate(value: &[u8], hemisphere: &[u8], axis: CoordinateAxis) -> Option<i32> {
    let degree_digits = axis.degree_digits();
    if value.len() < degree_digits + 2 {
        return None;
    }
    match value.iter().position(|byte| *byte == b'.') {
        Some(decimal) if decimal == degree_digits + 2 => {}
        None if value.len() == degree_digits + 2 => {}
        Some(_) | None => return None,
    }
    if !value[degree_digits..degree_digits + 2]
        .iter()
        .all(u8::is_ascii_digit)
    {
        return None;
    }
    let degrees = parse_unsigned(&value[..degree_digits])?;
    let minutes_e7 = parse_decimal_scaled(&value[degree_digits..], 10_000_000)?;
    coordinate_e7(degrees, minutes_e7, axis.sign(hemisphere)?, axis)
}

fn coordinate_e7(
    degrees: u32,
    minutes_e7: u32,
    sign: CoordinateSign,
    axis: CoordinateAxis,
) -> Option<i32> {
    const MINUTES_PER_DEGREE_E7: u32 = 60 * 10_000_000;

    let max_degrees = axis.max_degrees();
    if minutes_e7 >= MINUTES_PER_DEGREE_E7
        || degrees > max_degrees
        || (degrees == max_degrees && minutes_e7 != 0)
    {
        return None;
    }
    let coordinate = i64::from(degrees) * 10_000_000 + i64::from(minutes_e7) / 60;
    let signed = match sign {
        CoordinateSign::Positive => coordinate,
        CoordinateSign::Negative => -coordinate,
    };
    i32::try_from(signed).ok()
}

fn parse_unsigned(value: &[u8]) -> Option<u32> {
    if value.is_empty() {
        return None;
    }
    value.iter().try_fold(0u32, |parsed, byte| {
        byte.is_ascii_digit()
            .then(|| parsed.checked_mul(10)?.checked_add(u32::from(byte - b'0')))
            .flatten()
    })
}

fn parse_decimal_scaled(value: &[u8], scale: u32) -> Option<u32> {
    let (whole, fraction) = value
        .iter()
        .position(|byte| *byte == b'.')
        .map_or((value, &[][..]), |dot| (&value[..dot], &value[dot + 1..]));
    let whole = parse_unsigned(whole)?;
    let mut fraction_value = 0u32;
    let mut fraction_scale = 1u32;
    for byte in fraction {
        if !byte.is_ascii_digit() {
            return None;
        }
        if fraction_scale <= scale / 10 {
            fraction_value = fraction_value
                .checked_mul(10)?
                .checked_add(u32::from(byte - b'0'))?;
            fraction_scale *= 10;
        }
    }
    whole
        .checked_mul(scale)?
        .checked_add(fraction_value.checked_mul(scale / fraction_scale)?)
}

fn parse_signed_decimal_scaled(value: &[u8], scale: u32) -> Option<i32> {
    let (sign, magnitude) = match value.first() {
        Some(b'-') => (-1i64, &value[1..]),
        Some(b'+') => (1i64, &value[1..]),
        Some(_) => (1i64, value),
        None => return None,
    };
    i32::try_from(i64::from(parse_decimal_scaled(magnitude, scale)?) * sign).ok()
}

fn nmea_field(sentence: &[u8], wanted: usize) -> Option<&[u8]> {
    let end = sentence.iter().position(|byte| *byte == b'*')?;
    let mut field = 0usize;
    let mut start = 0usize;
    for (index, byte) in sentence[..end].iter().enumerate() {
        if *byte != b',' {
            continue;
        }
        if field == wanted {
            return Some(&sentence[start..index]);
        }
        field += 1;
        start = index + 1;
    }
    (field == wanted).then_some(&sentence[start..end])
}

fn valid_checksum(sentence: &[u8]) -> bool {
    let Some(star) = sentence.iter().position(|byte| *byte == b'*') else {
        return false;
    };
    if sentence.first() != Some(&b'$') || star + 3 != sentence.len() {
        return false;
    }
    let mut actual = 0u8;
    for byte in &sentence[1..star] {
        actual ^= byte;
    }
    let Some(high) = hex_nibble(sentence[star + 1]) else {
        return false;
    };
    let Some(low) = hex_nibble(sentence[star + 2]) else {
        return false;
    };
    actual == high << 4 | low
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::positioning::AltitudeMillimeters;

    fn parse(sentence: &[u8]) -> Option<GnssSnapshot> {
        let mut parser = NmeaParser::new();
        sentence
            .iter()
            .copied()
            .filter_map(|byte| parser.feed(byte))
            .next()
    }

    fn checksummed(payload: &str) -> std::string::String {
        let checksum = payload
            .as_bytes()
            .iter()
            .copied()
            .fold(0, |checksum, byte| checksum ^ byte);
        std::format!("${payload}*{checksum:02X}\r\n")
    }

    fn parsed_fix(sentence: &[u8]) -> GnssFix {
        parse(sentence)
            .and_then(GnssSnapshot::fix)
            .expect("sentence carries a valid fix")
    }

    #[test]
    fn parses_a_checksummed_gga_fix_without_floats() {
        let fix =
            parsed_fix(b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n");
        let position = fix.position();
        assert_eq!(position.latitude().get(), 481_173_000);
        assert_eq!(position.longitude().get(), 115_166_666);
        assert_eq!(
            position.altitude().map(AltitudeMillimeters::get),
            Some(545_400)
        );
        assert_eq!(fix.satellites(), 8);
        assert_eq!(fix.hdop_hundredths(), Some(90));
    }

    #[test]
    fn ignores_corrupt_and_non_gga_sentences() {
        assert!(
            parse(b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*00\r\n")
                .is_none()
        );
        assert!(parse(b"$GPRMC,123519,A,4807.038,N,01131.000,E,0,0,230394,,,A*68\r\n").is_none());

        for malformed in [
            "GPGGAX,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,",
            "GPGGA,123519,4807.038,N,01131.000,E,X,08,0.9,545.4,M,46.9,M,,",
        ] {
            let malformed = checksummed(malformed);
            assert_eq!(parse(malformed.as_bytes()), None);
        }
    }

    #[test]
    fn reports_searching_and_signed_southern_western_fixes() {
        let searching = checksummed("GNGGA,123519,,,,,0,03,4.2,,,,,,");
        assert_eq!(
            parse(searching.as_bytes()),
            Some(GnssSnapshot::Searching { satellites: 3 })
        );

        let fixed = checksummed("GNGGA,123519,3456.789,S,05822.123,W,1,12,1.25,-3.4,M,0,M,,");
        let fixed = parsed_fix(fixed.as_bytes());
        let position = fixed.position();
        assert_eq!(position.latitude().get(), -349_464_833);
        assert_eq!(position.longitude().get(), -583_687_166);
        assert_eq!(
            position.altitude().map(AltitudeMillimeters::get),
            Some(-3_400)
        );
        assert_eq!(fixed.hdop_hundredths(), Some(125));
        assert_eq!(fixed.satellites(), 12);
    }

    #[test]
    fn accepts_coordinate_edges_and_rejects_impossible_or_mismatched_coordinates() {
        for payload in [
            "GNGGA,123519,9000.000,N,18000.000,E,1,12,1.0,0,M,0,M,,",
            "GNGGA,123519,9000.000,S,18000.000,W,1,12,1.0,0,M,0,M,,",
        ] {
            let fixed = checksummed(payload);
            let position = parsed_fix(fixed.as_bytes()).position();
            assert_eq!(position.latitude().get().unsigned_abs(), 900_000_000);
            assert_eq!(position.longitude().get().unsigned_abs(), 1_800_000_000);
        }

        for payload in [
            "GNGGA,123519,9000.001,N,00000.000,E,1,12,1.0,0,M,0,M,,",
            "GNGGA,123519,0000.000,N,18000.001,E,1,12,1.0,0,M,0,M,,",
            "GNGGA,123519,1260.000,N,00000.000,E,1,12,1.0,0,M,0,M,,",
            "GNGGA,123519,1200.000,E,00000.000,E,1,12,1.0,0,M,0,M,,",
            "GNGGA,123519,1200.000,N,00000.000,N,1,12,1.0,0,M,0,M,,",
            "GNGGA,123519,121.000,N,00000.000,E,1,12,1.0,0,M,0,M,,",
        ] {
            let invalid = checksummed(payload);
            assert_eq!(parse(invalid.as_bytes()), None, "accepted {payload}");
        }
    }

    #[test]
    fn recovers_after_overlong_noise_and_an_embedded_sentence_start() {
        let mut parser = NmeaParser::new();
        for byte in core::iter::once(b'$').chain(core::iter::repeat_n(b'X', 120)) {
            assert_eq!(parser.feed(byte), None);
        }
        for byte in b"garbage$partial" {
            assert_eq!(parser.feed(*byte), None);
        }

        let known = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,*47\r\n";
        let recovered = known
            .iter()
            .copied()
            .find_map(|byte| parser.feed(byte))
            .and_then(GnssSnapshot::fix)
            .expect("a new sentence start resynchronizes the stream");
        assert_eq!(recovered.position().latitude().get(), 481_173_000);
        assert_eq!(recovered.position().longitude().get(), 115_166_666);
    }

    #[test]
    fn streams_search_fix_and_fix_loss_with_lowercase_checksum_support() {
        let payloads = [
            "GNGGA,123519,,,,,0,03,4.2,,,,,,",
            "GNGGA,123520,3456.789,S,05822.123,W,1,12,1.25,-3.4,M,0,M,,",
            "GNGGA,123521,,,,,0,02,5.1,,,,,,",
        ];
        let mut stream = std::string::String::new();
        for payload in payloads {
            let checksum = payload
                .as_bytes()
                .iter()
                .copied()
                .fold(0, |checksum, byte| checksum ^ byte);
            use core::fmt::Write as _;
            let _ = write!(stream, "${payload}*{checksum:02x}\r\n");
        }

        let mut parser = NmeaParser::new();
        let snapshots = stream
            .bytes()
            .filter_map(|byte| parser.feed(byte))
            .collect::<std::vec::Vec<_>>();
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0], GnssSnapshot::Searching { satellites: 3 });
        assert!(matches!(snapshots[1], GnssSnapshot::Fixed(_)));
        assert_eq!(snapshots[2], GnssSnapshot::Searching { satellites: 2 });
    }

    #[test]
    fn saturates_large_satellite_counts_and_drops_overflowing_optional_numbers() {
        let fixed =
            checksummed("GNGGA,123519,3456.789,N,05822.123,E,1,999,9999999999,9999999999,M,0,M,,");
        let fixed = parsed_fix(fixed.as_bytes());
        assert_eq!(fixed.satellites(), u8::MAX);
        assert_eq!(fixed.hdop_hundredths(), None);
        assert_eq!(fixed.position().altitude(), None);
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn sentence_byte_append_preserves_the_buffer_bound() {
        let mut parser = NmeaParser {
            sentence: kani::any(),
            len: kani::any(),
            collecting: true,
        };
        kani::assume(parser.len <= NMEA_SENTENCE_BYTES);

        parser.push_sentence_byte(kani::any());

        assert!(parser.len <= NMEA_SENTENCE_BYTES);
    }

    #[kani::proof]
    fn every_scaled_coordinate_output_respects_its_axis_bound() {
        let axis = if kani::any() {
            CoordinateAxis::Latitude
        } else {
            CoordinateAxis::Longitude
        };
        let sign = if kani::any() {
            CoordinateSign::Positive
        } else {
            CoordinateSign::Negative
        };
        let coordinate = coordinate_e7(kani::any(), kani::any(), sign, axis);

        if let Some(coordinate) = coordinate {
            assert!(coordinate.unsigned_abs() <= axis.max_degrees() * 10_000_000);
        }
    }
}
