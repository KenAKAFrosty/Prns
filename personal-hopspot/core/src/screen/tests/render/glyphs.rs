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

/// What the readout should look like: the same text draw the implementation makes, nothing else.
fn expected_battery_text(text: &str, x: i32, y: i32) -> MockDisplay<BinaryColor> {
    use embedded_graphics::mono_font::iso_8859_1::FONT_5X8;
    use embedded_graphics::mono_font::MonoTextStyle;
    use embedded_graphics::text::{Baseline, Text};
    let mut display = MockDisplay::new();
    let small = MonoTextStyle::new(&FONT_5X8, BinaryColor::Off);
    let _ = Text::with_baseline(text, Point::new(x, y), small, Baseline::Top).draw(&mut display);
    display
}

#[test]
fn level_battery_shows_the_exact_percent() {
    let mut display = MockDisplay::new();

    draw_battery(
        &mut display,
        2,
        0,
        PowerSnapshot::new(
            Some(BatteryPercent::saturating(97)),
            ExternalPowerState::Absent,
        ),
        true,
    );

    // Three characters, right-aligned in the 20px zone: text begins at 2 + 20 - 15.
    assert_eq!(display, expected_battery_text("97%", 7, 0));
}

#[test]
fn percent_stays_right_aligned_as_digits_shrink() {
    let mut display = MockDisplay::new();

    draw_battery(
        &mut display,
        2,
        0,
        PowerSnapshot::new(
            Some(BatteryPercent::saturating(7)),
            ExternalPowerState::Absent,
        ),
        true,
    );

    // Two characters: text begins at 2 + 20 - 10. A drained cell reads "7%", never "70%".
    assert_eq!(display, expected_battery_text("7%", 12, 0));
}

#[test]
fn unknown_battery_reads_as_dashes_not_a_number() {
    let mut display = MockDisplay::new();

    draw_battery(&mut display, 2, 0, PowerSnapshot::UNKNOWN, true);

    assert_eq!(display, expected_battery_text("--%", 7, 0));
}

#[test]
fn charging_battery_shows_the_plug_on_the_visible_phase() {
    let mut display = MockDisplay::new();
    display.set_allow_overdraw(true);

    draw_battery(&mut display, 2, 0, charging(62), true);

    // The plug body sits at the left edge of the zone, prongs pointing at the digits.
    assert_eq!(display.get_pixel(Point::new(2, 4)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(3, 2)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(6, 3)), Some(BinaryColor::Off));
    assert_eq!(display.get_pixel(Point::new(6, 5)), Some(BinaryColor::Off));
}

#[test]
fn charging_battery_hides_the_plug_on_the_off_phase() {
    let mut display = MockDisplay::new();

    draw_battery(&mut display, 2, 0, charging(62), false);

    // Off phase is the text alone; the blink carries the "charging" signal.
    assert_eq!(display, expected_battery_text("62%", 7, 0));
}

#[test]
fn full_charging_battery_drops_the_plug_for_the_full_width_number() {
    let mut display = MockDisplay::new();

    draw_battery(&mut display, 2, 0, charging(100), true);

    // "100%" fills the whole zone; a full cell reads as done, no cue needed.
    assert_eq!(display, expected_battery_text("100%", 2, 0));
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
