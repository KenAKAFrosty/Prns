use super::InputEvent;
use core::fmt::Write;
use personal_rns::interfaces::lora::{
    Frequency, Ghz24Profile, LoRaConfiguration, LoRaConfigurationState, Modulation,
    PreambleSymbols, TxPower, GHZ24_BALANCED_PROFILE,
};
use personal_rns::interfaces::subghz::{regions::us915::Us915, SubGConfigurationState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioEditorError {
    InvalidPowerLimit,
    ProfileExceedsPowerLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorPage {
    Band(usize),
    Fields(usize),
    Editing(usize),
    Frequency(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::screen) struct RadioEditor {
    page: EditorPage,
    profile: Ghz24Profile,
    subg: SubGConfigurationState,
    maximum_power: TxPower,
}

pub(in crate::screen) enum RadioEditorOutcome {
    Stay(RadioEditor),
    SubG(SubGConfigurationState),
    Save(LoRaConfiguration),
    Cancel,
}

const FIELD_LABELS: [&str; 9] = [
    "Balanced preset",
    "Frequency",
    "Spreading factor",
    "Bandwidth",
    "Coding rate",
    "Power",
    "Preamble",
    "Save",
    "Back",
];
const FREQUENCY_STEPS: [u32; 10] = [
    1_000_000_000,
    100_000_000,
    10_000_000,
    1_000_000,
    100_000,
    10_000,
    1_000,
    100,
    10,
    1,
];

impl RadioEditor {
    pub(super) fn new(
        configuration: LoRaConfigurationState,
        maximum_power: TxPower,
    ) -> Result<Self, RadioEditorError> {
        if !(-18..=13).contains(&maximum_power.dbm()) {
            return Err(RadioEditorError::InvalidPowerLimit);
        }
        let (profile, subg, band) = match configuration {
            LoRaConfigurationState::Unconfigured => (
                GHZ24_BALANCED_PROFILE,
                SubGConfigurationState::Unconfigured,
                0,
            ),
            LoRaConfigurationState::Configured(LoRaConfiguration::SubG(configuration)) => (
                GHZ24_BALANCED_PROFILE,
                SubGConfigurationState::Configured(configuration),
                0,
            ),
            LoRaConfigurationState::Configured(LoRaConfiguration::Ghz24(profile)) => (
                profile,
                SubGConfigurationState::Configured(Us915::auto_lora()),
                1,
            ),
        };
        if profile.tx_power().dbm() > maximum_power.dbm() {
            return Err(RadioEditorError::ProfileExceedsPowerLimit);
        }
        Ok(Self {
            page: EditorPage::Band(band),
            profile,
            subg,
            maximum_power,
        })
    }

    pub(in crate::screen) fn input(mut self, event: InputEvent) -> RadioEditorOutcome {
        match (self.page, event) {
            (EditorPage::Band(cursor), InputEvent::ShortPress) => {
                self.page = EditorPage::Band((cursor + 1) % 3)
            }
            (EditorPage::Band(0), InputEvent::LongPress) => {
                return RadioEditorOutcome::SubG(self.subg)
            }
            (EditorPage::Band(1), InputEvent::LongPress) => self.page = EditorPage::Fields(0),
            (EditorPage::Band(_), InputEvent::LongPress) => return RadioEditorOutcome::Cancel,
            (EditorPage::Fields(cursor), InputEvent::ShortPress) => {
                self.page = EditorPage::Fields((cursor + 1) % FIELD_LABELS.len())
            }
            (EditorPage::Fields(0), InputEvent::LongPress) => {
                if GHZ24_BALANCED_PROFILE.tx_power().dbm() <= self.maximum_power.dbm() {
                    self.profile = GHZ24_BALANCED_PROFILE;
                }
            }
            (EditorPage::Fields(1), InputEvent::LongPress) => self.page = EditorPage::Frequency(0),
            (EditorPage::Fields(7), InputEvent::LongPress) => {
                return RadioEditorOutcome::Save(LoRaConfiguration::Ghz24(self.profile))
            }
            (EditorPage::Fields(8), InputEvent::LongPress) => self.page = EditorPage::Band(1),
            (EditorPage::Fields(cursor), InputEvent::LongPress) => {
                self.page = EditorPage::Editing(cursor)
            }
            (EditorPage::Editing(cursor), InputEvent::LongPress) => {
                self.page = EditorPage::Fields(cursor)
            }
            (EditorPage::Editing(cursor), InputEvent::ShortPress) => self.advance_field(cursor),
            (EditorPage::Frequency(digit), InputEvent::LongPress) => {
                self.page = if digit + 1 == FREQUENCY_STEPS.len() {
                    EditorPage::Fields(1)
                } else {
                    EditorPage::Frequency(digit + 1)
                }
            }
            (EditorPage::Frequency(digit), InputEvent::ShortPress) => {
                let step = FREQUENCY_STEPS[digit];
                let hz = self.profile.frequency().hz();
                let current_digit = hz / step % 10;
                let base = u64::from(hz) - u64::from(current_digit * step);
                for offset in 1..=10 {
                    let next = base + u64::from((current_digit + offset) % 10) * u64::from(step);
                    let Ok(next) = u32::try_from(next) else {
                        continue;
                    };
                    if let Ok(profile) = self.profile.with_frequency(Frequency::new(next)) {
                        self.profile = profile;
                        break;
                    }
                }
            }
        }
        RadioEditorOutcome::Stay(self)
    }

    fn advance_field(&mut self, cursor: usize) {
        let Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        } = self.profile.modulation();
        let changed = match cursor {
            2 => {
                let mut next = spreading_factor.next();
                loop {
                    let candidate = self.profile.with_modulation(Modulation::Lora {
                        spreading_factor: next,
                        bandwidth,
                        coding_rate,
                    });
                    if candidate.is_ok() || next == spreading_factor {
                        break candidate;
                    }
                    next = next.next();
                }
            }
            3 => {
                let mut next = bandwidth.next();
                loop {
                    let candidate = self.profile.with_modulation(Modulation::Lora {
                        spreading_factor,
                        bandwidth: next,
                        coding_rate,
                    });
                    if candidate.is_ok() || next == bandwidth {
                        break candidate;
                    }
                    next = next.next();
                }
            }
            4 => self.profile.with_modulation(Modulation::Lora {
                spreading_factor,
                bandwidth,
                coding_rate: coding_rate.next(),
            }),
            5 => self.profile.with_tx_power(TxPower::new(
                if self.profile.tx_power().dbm() == self.maximum_power.dbm() {
                    -18
                } else {
                    self.profile.tx_power().dbm() + 1
                },
            )),
            6 => self.profile.with_preamble(PreambleSymbols::new(
                if self.profile.preamble().count() == u16::MAX {
                    12
                } else {
                    self.profile.preamble().count() + 1
                },
            )),
            _ => return,
        };
        if let Ok(profile) = changed {
            self.profile = profile;
        }
    }

    pub(in crate::screen) fn lines(self) -> [heapless::String<32>; 5] {
        let mut lines = core::array::from_fn(|_| heapless::String::new());
        match self.page {
            EditorPage::Band(cursor) => {
                let _ = lines[0].push_str("LoRa band");
                for (index, label) in ["Sub GHz", "2.4 GHz", "Back"].iter().enumerate() {
                    let _ = write!(
                        lines[index + 1],
                        "{}{}",
                        if cursor == index { "> " } else { "  " },
                        label
                    );
                }
            }
            EditorPage::Fields(cursor) | EditorPage::Editing(cursor) => {
                let _ = lines[0].push_str("2.4 GHz LoRa");
                let _ = lines[1].push_str(FIELD_LABELS[cursor]);
                let Modulation::Lora {
                    spreading_factor,
                    bandwidth,
                    coding_rate,
                } = self.profile.modulation();
                match cursor {
                    0 => {
                        let _ = lines[2].push_str("2445 / SF7 / BW812");
                        if GHZ24_BALANCED_PROFILE.tx_power().dbm() > self.maximum_power.dbm() {
                            let _ = lines[3].push_str("Exceeds board power limit");
                        }
                    }
                    1 => {
                        let _ = write!(
                            lines[2],
                            "{}.{:06} MHz",
                            self.profile.frequency().hz() / 1_000_000,
                            self.profile.frequency().hz() % 1_000_000
                        );
                    }
                    2 => {
                        let _ = write!(lines[2], "SF{}", spreading_factor as u8);
                    }
                    3 => {
                        let _ = write!(lines[2], "{} kHz", bandwidth.hz() / 1000);
                    }
                    4 => {
                        let _ = write!(lines[2], "4/{}", coding_rate.denominator());
                    }
                    5 => {
                        let _ = write!(lines[2], "{} dBm", self.profile.tx_power().dbm());
                    }
                    6 => {
                        let _ = write!(lines[2], "{} symbols", self.profile.preamble().count());
                    }
                    7 | 8 => {}
                    _ => {}
                }
                let _ = lines[4].push_str(if matches!(self.page, EditorPage::Editing(_)) {
                    "Tap: change Hold: done"
                } else {
                    "Tap: next Hold: select"
                });
            }
            EditorPage::Frequency(digit) => {
                let _ = lines[0].push_str("2.4 GHz frequency");
                let hz = self.profile.frequency().hz();
                let _ = write!(lines[1], "{}.{:06} MHz", hz / 1_000_000, hz % 1_000_000);
                let _ = write!(lines[2], "Digit {} / 10", digit + 1);
                let _ = lines[4].push_str("Tap: change Hold: next");
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::lora::{LoraBandwidth, SpreadingFactor};

    fn editor(profile: Ghz24Profile) -> RadioEditor {
        RadioEditor::new(
            LoRaConfigurationState::Configured(LoRaConfiguration::Ghz24(profile)),
            TxPower::new(11),
        )
        .unwrap()
    }
    fn stay(editor: RadioEditor, event: InputEvent) -> RadioEditor {
        match editor.input(event) {
            RadioEditorOutcome::Stay(editor) => editor,
            _ => panic!("unexpected departure"),
        }
    }

    #[test]
    fn constructor_checks_board_power_and_preserves_band_selection() {
        for maximum in [i8::MIN, -19, 14, i8::MAX] {
            assert_eq!(
                RadioEditor::new(LoRaConfigurationState::Unconfigured, TxPower::new(maximum)),
                Err(RadioEditorError::InvalidPowerLimit)
            );
        }
        assert_eq!(
            RadioEditor::new(LoRaConfigurationState::Unconfigured, TxPower::new(9)),
            Err(RadioEditorError::ProfileExceedsPowerLimit)
        );
        let subg = SubGConfigurationState::Configured(Us915::auto_lora());
        let editor = RadioEditor::new(subg.into(), TxPower::new(11)).unwrap();
        assert_eq!(editor.page, EditorPage::Band(0));
        assert_eq!(
            editor.lines().each_ref().map(|line| line.as_str()),
            ["LoRa band", "> Sub GHz", "  2.4 GHz", "  Back", ""]
        );
        assert!(
            matches!(editor.input(InputEvent::LongPress), RadioEditorOutcome::SubG(found) if found == subg)
        );
        let editor = stay(stay(editor, InputEvent::ShortPress), InputEvent::ShortPress);
        assert!(matches!(
            editor.input(InputEvent::LongPress),
            RadioEditorOutcome::Cancel
        ));
        assert_eq!(
            stay(editor, InputEvent::ShortPress).page,
            EditorPage::Band(0)
        );
    }

    #[test]
    fn every_field_renders_and_navigation_preserves_checked_values() {
        let mut editor = stay(editor(GHZ24_BALANCED_PROFILE), InputEvent::LongPress);
        for (field, label) in FIELD_LABELS.iter().enumerate() {
            assert_eq!(editor.page, EditorPage::Fields(field));
            assert_eq!(editor.lines()[1].as_str(), *label);
            match field {
                0 => editor = stay(editor, InputEvent::LongPress),
                1 => {
                    let mut frequency = stay(editor, InputEvent::LongPress);
                    assert_eq!(frequency.lines()[1].as_str(), "2445.000000 MHz");
                    for digit in 0..10 {
                        assert_eq!(frequency.page, EditorPage::Frequency(digit));
                        frequency = stay(frequency, InputEvent::ShortPress);
                        assert_eq!(frequency.profile.validate(), Ok(()));
                        frequency = stay(frequency, InputEvent::LongPress);
                    }
                    assert_eq!(frequency.page, EditorPage::Fields(1));
                }
                2..=6 => {
                    let mut editing = stay(editor, InputEvent::LongPress);
                    assert_eq!(editing.page, EditorPage::Editing(field));
                    assert_eq!(editing.lines()[4].as_str(), "Tap: change Hold: done");
                    editing = stay(editing, InputEvent::ShortPress);
                    assert_eq!(editing.profile.validate(), Ok(()));
                    assert_eq!(
                        stay(editing, InputEvent::LongPress).page,
                        EditorPage::Fields(field)
                    );
                }
                7 => assert!(
                    matches!(editor.input(InputEvent::LongPress), RadioEditorOutcome::Save(LoRaConfiguration::Ghz24(profile)) if profile == GHZ24_BALANCED_PROFILE)
                ),
                8 => assert_eq!(
                    stay(editor, InputEvent::LongPress).page,
                    EditorPage::Band(1)
                ),
                _ => unreachable!(),
            }
            editor = stay(editor, InputEvent::ShortPress);
        }
        assert_eq!(editor.page, EditorPage::Fields(0));
        let mut maximum = editor;
        maximum.profile = maximum
            .profile
            .with_tx_power(TxPower::new(11))
            .unwrap()
            .with_preamble(PreambleSymbols::new(u16::MAX))
            .unwrap();
        maximum.advance_field(5);
        assert_eq!(maximum.profile.tx_power(), TxPower::new(-18));
        maximum.advance_field(6);
        assert_eq!(maximum.profile.preamble(), PreambleSymbols::new(12));
    }

    #[test]
    fn preset_cannot_exceed_the_board_power_limit() {
        let profile = GHZ24_BALANCED_PROFILE
            .with_tx_power(TxPower::new(9))
            .unwrap();
        let editor = RadioEditor::new(
            LoRaConfigurationState::Configured(LoRaConfiguration::Ghz24(profile)),
            TxPower::new(9),
        )
        .unwrap();
        let editor = stay(editor, InputEvent::LongPress);
        assert_eq!(editor.lines()[3].as_str(), "Exceeds board power limit");
        assert_eq!(stay(editor, InputEvent::LongPress).profile, profile);
    }

    #[test]
    fn decimal_frequency_edits_change_only_the_selected_digit_and_wrap_to_a_valid_value() {
        for hz in [
            2_400_406_000,
            2_401_234_567,
            2_445_000_000,
            2_480_987_654,
            2_483_094_000,
        ] {
            let profile = GHZ24_BALANCED_PROFILE
                .with_frequency(Frequency::new(hz))
                .unwrap();
            for digit in 0..10 {
                let mut editor = editor(profile);
                editor.page = EditorPage::Frequency(digit);
                for _ in 0..10 {
                    let text = std::format!("{}", editor.profile.frequency().hz());
                    let mut digits = text.into_bytes();
                    let current = digits[digit] - b'0';
                    let expected = (1..=10)
                        .find_map(|offset| {
                            digits[digit] = b'0' + (current + offset) % 10;
                            let hz = core::str::from_utf8(&digits).unwrap().parse::<u32>().ok()?;
                            editor.profile.with_frequency(Frequency::new(hz)).ok()
                        })
                        .unwrap();
                    editor = stay(editor, InputEvent::ShortPress);
                    assert_eq!(editor.profile, expected);
                }
            }
        }
    }

    #[test]
    fn field_values_and_displays_match_each_requested_edit() {
        let mut value = editor(GHZ24_BALANCED_PROFILE);
        for (field, expected) in [
            (0, "2445 / SF7 / BW812"),
            (1, "2445.000000 MHz"),
            (2, "SF7"),
            (3, "812 kHz"),
            (4, "4/5"),
            (5, "10 dBm"),
            (6, "18 symbols"),
            (7, ""),
            (8, ""),
        ] {
            value.page = EditorPage::Fields(field);
            assert_eq!(value.lines()[2].as_str(), expected);
        }
        for (field, expected) in [
            (2, "SF8"),
            (3, "203 kHz"),
            (4, "4/6"),
            (5, "11 dBm"),
            (6, "19 symbols"),
        ] {
            let mut changed = editor(GHZ24_BALANCED_PROFILE);
            changed.page = EditorPage::Editing(field);
            changed = stay(changed, InputEvent::ShortPress);
            assert_eq!(changed.lines()[2].as_str(), expected);
        }
        let mut equal_ceiling = RadioEditor::new(
            LoRaConfigurationState::Configured(LoRaConfiguration::Ghz24(GHZ24_BALANCED_PROFILE)),
            TxPower::new(10),
        )
        .unwrap();
        equal_ceiling.page = EditorPage::Fields(0);
        assert_eq!(equal_ceiling.lines()[3].as_str(), "");
    }

    #[test]
    fn bandwidth_tuning_wraps_past_a_wider_invalid_channel_to_a_valid_narrower_one() {
        let profile = GHZ24_BALANCED_PROFILE
            .with_modulation(Modulation::Lora {
                spreading_factor: SpreadingFactor::Sf7,
                bandwidth: LoraBandwidth::Bw406kHz,
                coding_rate: personal_rns::interfaces::lora::CodingRate::Cr45,
            })
            .unwrap()
            .with_frequency(Frequency::new(2_400_250_000))
            .unwrap();
        let mut editor = editor(profile);
        editor.advance_field(3);
        assert_eq!(
            editor.profile.modulation().bandwidth(),
            LoraBandwidth::Bw203kHz
        );
    }

    #[test]
    fn tuning_skips_invalid_next_values_without_getting_stuck() {
        let modulation = Modulation::Lora {
            spreading_factor: SpreadingFactor::Sf12,
            bandwidth: LoraBandwidth::Bw203kHz,
            coding_rate: personal_rns::interfaces::lora::CodingRate::Cr45,
        };
        let profile = GHZ24_BALANCED_PROFILE
            .with_modulation(modulation)
            .unwrap()
            .with_preamble(PreambleSymbols::new(8))
            .unwrap();
        let mut editor = editor(profile);
        editor.advance_field(2);
        assert_eq!(
            editor.profile.modulation().spreading_factor(),
            SpreadingFactor::Sf7
        );
        assert_eq!(editor.profile.preamble(), PreambleSymbols::new(8));
        editor.profile = editor
            .profile
            .with_frequency(Frequency::new(2_400_150_000))
            .unwrap();
        editor.advance_field(3);
        assert_eq!(
            editor.profile.modulation().bandwidth(),
            LoraBandwidth::Bw203kHz
        );
        assert_eq!(editor.profile.validate(), Ok(()));
    }
}
