use super::*;

fn charging(percent: u8) -> PowerSnapshot {
    PowerSnapshot::new(
        Some(BatteryPercent::saturating(percent)),
        ExternalPowerState::Present {
            charging: ChargingState::Charging,
        },
    )
}

fn battery_display() -> MockDisplay<BinaryColor> {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);
    display
}

#[test]
fn usb_icon_draws_full_width_tongue() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_interface_icon(&mut display, 0, 0, CardKind::Usb, BinaryColor::On);

    display.assert_pattern(&[
        "    #    ",
        "    #    ",
        "#########",
        "#       #",
        "#       #",
        "#########",
        "#       #",
        "#########",
    ]);
}

#[test]
fn ble_icon_reads_as_bluetooth_rune() {
    let mut display = MockDisplay::new();

    draw_interface_icon(&mut display, 0, 0, CardKind::Ble, BinaryColor::On);

    display.assert_pattern(&[
        "    #    ",
        "    ##   ",
        "  # # #  ",
        "   ###   ",
        "    #    ",
        "   ###   ",
        "  # # #  ",
        "    ##   ",
        "    #    ",
    ]);
}

#[test]
fn level_battery_keeps_the_silhouette_and_shows_the_exact_number() {
    let mut display = battery_display();

    draw_battery(
        &mut display,
        2,
        0,
        PowerSnapshot::new(
            Some(BatteryPercent::saturating(97)),
            ExternalPowerState::Absent,
        ),
    );

    // The original 15x9 silhouette and left terminal remain.
    assert_eq!(display.get_pixel(Point::new(2, 0)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(16, 8)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(0, 4)), Some(BinaryColor::Off));
    // "97" is centered inside: the 9 starts at x=6 and the 7 at x=10.
    for x in 6..=8 {
        assert_eq!(display.get_pixel(Point::new(x, 2)), Some(BinaryColor::Off));
    }
    assert_eq!(display.get_pixel(Point::new(6, 3)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(7, 3)), None);
    assert_eq!(display.get_pixel(Point::new(8, 3)), Some(BinaryColor::Off));
    for x in 10..=12 {
        assert_eq!(display.get_pixel(Point::new(x, 2)), Some(BinaryColor::Off));
    }
    assert_eq!(display.get_pixel(Point::new(10, 5)), None);
    assert_eq!(display.get_pixel(Point::new(12, 5)), Some(BinaryColor::Off));
}

#[test]
fn one_and_three_digit_levels_stay_centered_inside_the_battery() {
    let mut one_digit = battery_display();

    draw_battery(
        &mut one_digit,
        2,
        0,
        PowerSnapshot::new(
            Some(BatteryPercent::saturating(7)),
            ExternalPowerState::Absent,
        ),
    );

    // A single 7 occupies x=8..10 in the center, with no implied trailing zero.
    for x in 8..=10 {
        assert_eq!(
            one_digit.get_pixel(Point::new(x, 2)),
            Some(BinaryColor::Off)
        );
    }
    assert_eq!(one_digit.get_pixel(Point::new(8, 5)), None);
    assert_eq!(
        one_digit.get_pixel(Point::new(10, 5)),
        Some(BinaryColor::Off)
    );

    let mut three_digits = battery_display();
    draw_battery(
        &mut three_digits,
        2,
        0,
        PowerSnapshot::new(
            Some(BatteryPercent::saturating(100)),
            ExternalPowerState::Absent,
        ),
    );

    // "100" fills eleven of the thirteen interior pixels without touching the shell edge.
    assert_eq!(three_digits.get_pixel(Point::new(3, 4)), None);
    assert_eq!(
        three_digits.get_pixel(Point::new(5, 2)),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        three_digits.get_pixel(Point::new(8, 2)),
        Some(BinaryColor::Off)
    );
    assert_eq!(
        three_digits.get_pixel(Point::new(14, 6)),
        Some(BinaryColor::Off)
    );
    assert_eq!(three_digits.get_pixel(Point::new(15, 4)), None);
}

#[test]
fn unknown_battery_keeps_the_shell_and_reads_as_dashes() {
    let mut display = battery_display();

    draw_battery(&mut display, 2, 0, PowerSnapshot::UNKNOWN);

    assert_eq!(display.get_pixel(Point::new(2, 0)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(16, 8)), Some(BinaryColor::Off));
    for x in 6..=8 {
        assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
    }
    for x in 10..=12 {
        assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
    }
    assert_eq!(display.get_pixel(Point::new(9, 4)), None);
}

#[test]
fn charging_battery_shows_the_steady_plug() {
    let mut display = battery_display();

    draw_battery(&mut display, 2, 0, charging(62));

    // The original plug returns at the right edge of the battery body.
    for x in 17..=20 {
        assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
    }
    assert_eq!(display.get_pixel(Point::new(21, 3)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(23, 4)), None);
}

#[test]
fn externally_powered_battery_keeps_the_plug_steady_when_charging_is_unknown() {
    let mut display = battery_display();

    draw_battery(
        &mut display,
        2,
        0,
        PowerSnapshot::new(
            Some(BatteryPercent::saturating(53)),
            ExternalPowerState::from_presence(true),
        ),
    );

    // Presence-only hosts cannot distinguish charging from idle. External power is the only fact
    // the steady plug communicates, so that is sufficient to keep it visible.
    assert_eq!(display.get_pixel(Point::new(17, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(21, 3)), Some(BinaryColor::Off));
}

#[test]
fn full_charging_battery_keeps_100_and_the_plug() {
    let mut display = battery_display();

    draw_battery(&mut display, 2, 0, charging(100));

    assert_eq!(display.get_pixel(Point::new(2, 0)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(5, 2)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(14, 6)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(17, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(21, 3)), Some(BinaryColor::Off));
}

#[test]
fn person_icon_reads_as_destination_count_glyph() {
    let mut display = MockDisplay::new();

    draw_person(&mut display, 0, 0);

    display.assert_pattern(&[
        "   ###   ",
        "  #   #  ",
        "  #   #  ",
        "   ###   ",
        "  #   #  ",
        " #     # ",
    ]);
}

#[test]
fn link_icon_reads_as_chain_glyph() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_link(&mut display, 0, 0);

    display.assert_pattern(&[
        " ##  ## ", "#      #", "#   #  #", "#  #   #", "#      #", " ##  ## ",
    ]);
}

#[test]
fn clock_icon_reads_as_activity_age_glyph() {
    let mut display = MockDisplay::new();

    draw_clock(&mut display, 0, 0);

    display.assert_pattern(&[
        "  ###  ", " #   # ", "#  #  #", "#  ## #", "#     #", " #   # ", "  ###  ",
    ]);
}

#[test]
fn wifi_icon_reads_as_status_arc_glyph() {
    let mut display = MockDisplay::new();

    draw_interface_icon(&mut display, 0, 0, CardKind::Wifi, BinaryColor::On);

    display.assert_pattern(&[
        "  #####  ",
        " #     # ",
        "#       #",
        "         ",
        "   ###   ",
        "  #   #  ",
        "         ",
        "    #    ",
        "   ###   ",
    ]);
}

#[test]
fn lora_icon_reads_as_long_range_radio_glyph() {
    let mut display = MockDisplay::new();

    draw_interface_icon(&mut display, 0, 0, CardKind::LoRa, BinaryColor::On);

    display.assert_pattern(&[
        "#   #   #",
        " #  #  # ",
        "  # # #  ",
        "   ###   ",
        "    #    ",
        "    #    ",
        "    #    ",
        "   ###   ",
        "  #####  ",
    ]);
}

#[test]
fn esp_now_icon_reads_as_omni_broadcast_glyph() {
    let mut display = MockDisplay::new();

    draw_interface_icon(&mut display, 0, 0, CardKind::EspNow, BinaryColor::On);

    display.assert_pattern(&[
        "         ",
        "#       #",
        " #     # ",
        "  # # #  ",
        "   ###   ",
        "  # # #  ",
        " #     # ",
        "#       #",
    ]);
}
