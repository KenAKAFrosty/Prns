use super::*;

#[test]
fn count_formatter_uses_blank_base_then_metric_suffixes() {
    assert_eq!(fmt_count(0).as_str(), "0");
    assert_eq!(fmt_count(999).as_str(), "999");
    assert_eq!(fmt_count(1_000).as_str(), "1.0K");
    assert_eq!(fmt_count(12_345).as_str(), "12K");
    assert_eq!(fmt_count(999_999).as_str(), "999K");
    assert_eq!(fmt_count(1_000_000).as_str(), "1.0M");
    assert_eq!(fmt_count(1_234_567_890).as_str(), "1.2B");
    assert_eq!(fmt_count(u32::MAX).as_str(), "4.2B");
}

#[test]
fn byte_formatter_uses_byte_base_then_metric_suffixes() {
    assert_eq!(fmt_bytes(0).as_str(), "0B");
    assert_eq!(fmt_bytes(999).as_str(), "999B");
    assert_eq!(fmt_bytes(1_200).as_str(), "1.2K");
    assert_eq!(fmt_bytes(1_234_567).as_str(), "1.2M");
    assert_eq!(fmt_bytes(1_234_567_890).as_str(), "1.2G");
    assert_eq!(fmt_bytes(u64::MAX).as_str(), "18E");
}

#[test]
fn live_stat_formatters_stay_compact() {
    assert_eq!(fmt_rate_bytes_per_sec(0).as_str(), "0B/s");
    assert_eq!(fmt_rate_bytes_per_sec(999).as_str(), "999B/s");
    assert_eq!(fmt_rate_bytes_per_sec(1_200).as_str(), "1.2K/s");
    assert_eq!(fmt_rate_bytes_per_sec(12_000).as_str(), "12K/s");
    assert_eq!(fmt_rate_bytes_per_sec(999_999).as_str(), "999K/s");
    assert_eq!(fmt_rate_bytes_per_sec(1_234_567).as_str(), "1.2M/s");
    assert_eq!(fmt_rate_bytes_per_sec(1_234_567_890).as_str(), "1.2G/s");

    assert_eq!(fmt_activity_age(None).as_str(), "-");
    assert_eq!(fmt_activity_age(Some(0)).as_str(), "now");
    assert_eq!(fmt_activity_age(Some(3)).as_str(), "3s");
    assert_eq!(fmt_activity_age(Some(123)).as_str(), "2m");
    assert_eq!(fmt_activity_age(Some(7200)).as_str(), "2h");
}

#[test]
fn compact_number_draws_decimal_as_single_pixel() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_compact_number(&mut display, "1.2K/s", Point::new(0, 0), BinaryColor::On);

    assert_eq!(compact_numeric_width("1.2K/s"), 25);
    assert_eq!(display.get_pixel(Point::new(5, 6)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(6, 6)), None);
    assert_eq!(display.get_pixel(Point::new(19, 2)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(18, 3)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(17, 4)), Some(BinaryColor::On));
    assert_eq!(display.get_pixel(Point::new(19, 3)), None);
}
