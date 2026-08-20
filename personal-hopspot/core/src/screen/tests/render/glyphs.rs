use super::*;

fn charging(percent: u8) -> PowerSnapshot {
    PowerSnapshot::new(
        Some(BatteryPercent::saturating(percent)),
        ExternalPowerState::Present {
            charging: ChargingState::Charging,
        },
    )
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
fn unknown_battery_dash_is_symmetric() {
    let mut display = MockDisplay::new();

    draw_battery(&mut display, 2, 0, PowerSnapshot::UNKNOWN, true);

    assert_eq!(display.get_pixel(Point::new(5, 4)), None);
    for x in 6..=12 {
        assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
    }
    assert_eq!(display.get_pixel(Point::new(13, 4)), None);
}

#[test]
fn charging_battery_blinks_the_current_tier() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, charging(62), true);

    assert_eq!(display.get_pixel(Point::new(7, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(10, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(13, 4)), Some(BinaryColor::Off));
}

#[test]
fn charging_battery_hides_only_the_current_tier_on_the_off_phase() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, charging(62), false);

    assert_eq!(display.get_pixel(Point::new(7, 4)), None);
    assert_eq!(display.get_pixel(Point::new(10, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(13, 4)), Some(BinaryColor::Off));
}

#[test]
fn charging_battery_draws_right_side_plug_until_full() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, charging(62), true);

    for x in 17..=20 {
        assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
    }
    assert_eq!(display.get_pixel(Point::new(21, 3)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(23, 4)), None);
}

#[test]
fn full_charging_battery_uses_a_steady_filled_shape_without_the_plug() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, charging(100), false);

    assert_eq!(display.get_pixel(Point::new(2, 0)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(16, 8)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(0, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(21, 3)), None);
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
