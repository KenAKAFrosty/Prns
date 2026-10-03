use personal_rns::interfaces::lora::{
    Frequency, ModemPreset, Modulation, RadioProfile, RadioProfileError, TxPower,
};
use personal_rns::interfaces::subghz::{
    supported_modes, RegulatoryRegion, SubGConfiguration, SubGMode, SubGRegion,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum SubGScreen {
    Region { cursor: usize },
    Mode { cursor: usize },
    Preset { cursor: usize },
    Frequency { cursor: FreqRow, edit: EditMode },
    Custom { cursor: CustomRow, edit: EditMode },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum SubGModeChoice {
    AutoLoRa,
    ManualLoRa,
    Back,
}

const MANUAL_LORA_MODE_CHOICES: [SubGModeChoice; 2] =
    [SubGModeChoice::ManualLoRa, SubGModeChoice::Back];
const AUTO_LORA_MODE_CHOICES: [SubGModeChoice; 3] = [
    SubGModeChoice::AutoLoRa,
    SubGModeChoice::ManualLoRa,
    SubGModeChoice::Back,
];

pub(in crate::screen) fn subg_mode_choices(region: SubGRegion) -> &'static [SubGModeChoice] {
    if supported_modes(region).supports(SubGMode::AutoLoRa) {
        &AUTO_LORA_MODE_CHOICES
    } else {
        &MANUAL_LORA_MODE_CHOICES
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum EditMode {
    Browsing,
    Field,
    Freq { place: FreqPlace },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum PresetChoice {
    Preset(ModemPreset),
    Custom,
    Back,
}

pub(in crate::screen) const PRESET_CHOICES: [PresetChoice; 6] = [
    PresetChoice::Preset(ModemPreset::ShortFast),
    PresetChoice::Preset(ModemPreset::MediumFast),
    PresetChoice::Preset(ModemPreset::LongFast),
    PresetChoice::Preset(ModemPreset::LongSlow),
    PresetChoice::Custom,
    PresetChoice::Back,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum FreqPlace {
    Hundreds,
    Tens,
    Ones,
    Tenths,
    Hundredths,
    Thousandths,
}

impl FreqPlace {
    fn digit_step_hz(self) -> u32 {
        match self {
            Self::Hundreds => 100_000_000,
            Self::Tens => 10_000_000,
            Self::Ones => 1_000_000,
            Self::Tenths => 100_000,
            Self::Hundredths => 10_000,
            Self::Thousandths => 1_000,
        }
    }

    fn next_within_row(self) -> Option<Self> {
        match self {
            Self::Hundreds => Some(Self::Tens),
            Self::Tens => Some(Self::Ones),
            Self::Ones => None,
            Self::Tenths => Some(Self::Hundredths),
            Self::Hundredths => Some(Self::Thousandths),
            Self::Thousandths => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum CustomRow {
    SpreadingFactor,
    Bandwidth,
    CodingRate,
    FreqMhz,
    FreqKhz,
    TxPower,
    Save,
    Back,
}

pub(in crate::screen) const CUSTOM_ROWS: [CustomRow; 8] = [
    CustomRow::SpreadingFactor,
    CustomRow::Bandwidth,
    CustomRow::CodingRate,
    CustomRow::FreqMhz,
    CustomRow::FreqKhz,
    CustomRow::TxPower,
    CustomRow::Save,
    CustomRow::Back,
];

impl CustomRow {
    const FIRST: Self = Self::SpreadingFactor;

    fn next(self) -> Self {
        match self {
            Self::SpreadingFactor => Self::Bandwidth,
            Self::Bandwidth => Self::CodingRate,
            Self::CodingRate => Self::FreqMhz,
            Self::FreqMhz => Self::FreqKhz,
            Self::FreqKhz => Self::TxPower,
            Self::TxPower => Self::Save,
            Self::Save => Self::Back,
            Self::Back => Self::SpreadingFactor,
        }
    }

    fn freq_first_place(self) -> Option<FreqPlace> {
        match self {
            Self::FreqMhz => Some(FreqPlace::Hundreds),
            Self::FreqKhz => Some(FreqPlace::Tenths),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum FreqRow {
    Channel,
    Mhz,
    Khz,
    Save,
    Back,
}

pub(in crate::screen) const FREQ_ROWS: [FreqRow; 5] = [
    FreqRow::Channel,
    FreqRow::Mhz,
    FreqRow::Khz,
    FreqRow::Save,
    FreqRow::Back,
];

impl FreqRow {
    const FIRST: Self = Self::Channel;

    fn next(self) -> Self {
        match self {
            Self::Channel => Self::Mhz,
            Self::Mhz => Self::Khz,
            Self::Khz => Self::Save,
            Self::Save => Self::Back,
            Self::Back => Self::Channel,
        }
    }

    fn freq_first_place(self) -> Option<FreqPlace> {
        match self {
            Self::Mhz => Some(FreqPlace::Hundreds),
            Self::Khz => Some(FreqPlace::Tenths),
            _ => None,
        }
    }
}

const LORA_TX_POWER_MIN_DBM: i8 = -9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum SubGRegionChoice {
    Region(SubGRegion),
    Cancel,
}

const CUSTOM_REGION_INDEX: usize = RegulatoryRegion::ALL.len();
pub(in crate::screen) const SUBG_REGION_CANCEL: usize = CUSTOM_REGION_INDEX + 1;
pub(in crate::screen) const SUBG_REGION_COUNT: usize = SUBG_REGION_CANCEL + 1;

pub(in crate::screen) fn subg_region_choice(index: usize) -> SubGRegionChoice {
    if index < RegulatoryRegion::ALL.len() {
        return SubGRegionChoice::Region(SubGRegion::Regulated(RegulatoryRegion::ALL[index]));
    }
    if index == CUSTOM_REGION_INDEX {
        return SubGRegionChoice::Region(SubGRegion::Custom);
    }
    SubGRegionChoice::Cancel
}

pub(in crate::screen) fn region_index(region: SubGRegion) -> usize {
    match region {
        SubGRegion::Regulated(region) => RegulatoryRegion::ALL
            .iter()
            .position(|candidate| *candidate == region)
            .unwrap_or(0),
        SubGRegion::Custom => CUSTOM_REGION_INDEX,
    }
}

fn bump_freq_place(hz: u32, place: FreqPlace) -> u32 {
    let step = place.digit_step_hz();
    let decade = step * 10;
    let above = (hz / decade) * decade;
    let within = hz % decade;
    let lower = within % step;
    let digit = within / step;
    above + ((digit + 1) % 10) * step + lower
}

fn valid_center_range(profile: &RadioProfile) -> (u32, u32) {
    let range = profile.region().frequency_range();
    let bandwidth = channel_bandwidth_hz(profile);
    let lower_half = bandwidth / 2;
    let upper_half = bandwidth - lower_half;
    (
        range.minimum().hz() + lower_half,
        range.maximum().hz() - upper_half,
    )
}

fn clamp_freq_to_region(hz: u32, profile: &RadioProfile) -> u32 {
    let (low, high) = valid_center_range(profile);
    hz.clamp(low, high)
}

fn apply_region(
    profile: RadioProfile,
    region: SubGRegion,
) -> Result<RadioProfile, RadioProfileError> {
    if region == profile.region() {
        Ok(profile)
    } else {
        let defaults = region.manual_lora_defaults();
        RadioProfile::new(
            region,
            defaults.frequency(),
            defaults.modulation(),
            defaults.tx_power(),
            defaults.preamble(),
        )
    }
}

fn apply_preset(profile: RadioProfile, preset: ModemPreset) -> RadioProfile {
    profile
        .with_modulation(preset.modulation())
        .unwrap_or(profile)
}

pub(in crate::screen) fn scroll_start(cursor: usize, count: usize, visible: usize) -> usize {
    if count <= visible || cursor < visible {
        0
    } else {
        (cursor + 1 - visible).min(count - visible)
    }
}

pub(in crate::screen) fn step_custom_row(profile: RadioProfile, row: CustomRow) -> RadioProfile {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation();
    match row {
        CustomRow::SpreadingFactor => profile
            .with_modulation(Modulation::Lora {
                spreading_factor: spreading_factor.next(),
                bandwidth,
                coding_rate,
            })
            .unwrap_or(profile),
        CustomRow::Bandwidth => profile
            .with_modulation(Modulation::Lora {
                spreading_factor,
                bandwidth: bandwidth.next(),
                coding_rate,
            })
            .unwrap_or(profile),
        CustomRow::CodingRate => profile
            .with_modulation(Modulation::Lora {
                spreading_factor,
                bandwidth,
                coding_rate: coding_rate.next(),
            })
            .unwrap_or(profile),
        CustomRow::TxPower => {
            let dbm = profile.tx_power().dbm();
            let ceiling = profile.region().max_tx_power().dbm();
            profile
                .with_tx_power(TxPower::new(if dbm >= ceiling {
                    LORA_TX_POWER_MIN_DBM
                } else {
                    dbm + 1
                }))
                .unwrap_or(profile)
        }
        CustomRow::FreqMhz | CustomRow::FreqKhz | CustomRow::Save | CustomRow::Back => profile,
    }
}

pub(in crate::screen) enum SubGEditorOutcome {
    Stay {
        screen: SubGScreen,
        profile: RadioProfile,
    },
    Commit(SubGConfiguration),
    Cancel,
}

fn preset_cursor_for(modulation: Modulation) -> usize {
    let target = match ModemPreset::matching(modulation) {
        Some(preset) => PresetChoice::Preset(preset),
        None => PresetChoice::Custom,
    };
    PRESET_CHOICES
        .iter()
        .position(|&choice| choice == target)
        .unwrap_or(0)
}

enum FreqStep {
    Place(FreqPlace),
    Done(RadioProfile),
}

fn bump_freq(profile: RadioProfile, place: FreqPlace) -> RadioProfile {
    let hz = bump_freq_place(profile.frequency().hz(), place);
    let hz = clamp_freq_to_region(hz, &profile);
    profile
        .with_frequency(Frequency::new(hz))
        .unwrap_or(profile)
}

fn channel_bandwidth_hz(profile: &RadioProfile) -> u32 {
    let Modulation::Lora { bandwidth, .. } = profile.modulation();
    bandwidth.hz()
}

pub(in crate::screen) fn channel_count(profile: &RadioProfile) -> u32 {
    let range = profile.region().frequency_range();
    let low = range.minimum().hz();
    let high = range.maximum().hz();
    ((high - low) / channel_bandwidth_hz(profile)).max(1)
}

fn channel_center_hz(profile: &RadioProfile, channel: u32) -> u32 {
    let low = profile.region().frequency_range().minimum().hz();
    let bandwidth = channel_bandwidth_hz(profile);
    low + bandwidth / 2 + channel * bandwidth
}

pub(in crate::screen) fn current_channel(profile: &RadioProfile) -> u32 {
    let low = profile.region().frequency_range().minimum().hz();
    let hz = profile.frequency().hz();
    if hz <= low {
        0
    } else {
        ((hz - low) / channel_bandwidth_hz(profile)).min(channel_count(profile) - 1)
    }
}

fn step_freq_channel(profile: RadioProfile) -> RadioProfile {
    let next_channel = (current_channel(&profile) + 1) % channel_count(&profile);
    profile
        .with_frequency(Frequency::new(channel_center_hz(&profile, next_channel)))
        .unwrap_or(profile)
}

fn advance_freq_place(profile: RadioProfile, place: FreqPlace) -> FreqStep {
    match place.next_within_row() {
        Some(next_place) => FreqStep::Place(next_place),
        None => {
            let frequency =
                Frequency::new(clamp_freq_to_region(profile.frequency().hz(), &profile));
            FreqStep::Done(profile.with_frequency(frequency).unwrap_or(profile))
        }
    }
}

pub(in crate::screen) fn subg_editor_tap(
    screen: SubGScreen,
    profile: RadioProfile,
) -> (SubGScreen, RadioProfile) {
    match screen {
        SubGScreen::Region { cursor } => (
            SubGScreen::Region {
                cursor: (cursor + 1) % SUBG_REGION_COUNT,
            },
            profile,
        ),
        SubGScreen::Mode { cursor } => (
            SubGScreen::Mode {
                cursor: (cursor + 1) % subg_mode_choices(profile.region()).len(),
            },
            profile,
        ),
        SubGScreen::Preset { cursor } => (
            SubGScreen::Preset {
                cursor: (cursor + 1) % PRESET_CHOICES.len(),
            },
            profile,
        ),
        SubGScreen::Frequency { cursor, edit } => match edit {
            EditMode::Freq { place } => (
                SubGScreen::Frequency { cursor, edit },
                bump_freq(profile, place),
            ),
            EditMode::Field => (
                SubGScreen::Frequency { cursor, edit },
                step_freq_channel(profile),
            ),
            EditMode::Browsing => (
                SubGScreen::Frequency {
                    cursor: cursor.next(),
                    edit,
                },
                profile,
            ),
        },
        SubGScreen::Custom { cursor, edit } => match edit {
            EditMode::Browsing => (
                SubGScreen::Custom {
                    cursor: cursor.next(),
                    edit,
                },
                profile,
            ),
            EditMode::Field => (
                SubGScreen::Custom { cursor, edit },
                step_custom_row(profile, cursor),
            ),
            EditMode::Freq { place } => (
                SubGScreen::Custom { cursor, edit },
                bump_freq(profile, place),
            ),
        },
    }
}

pub(in crate::screen) fn subg_editor_hold(
    screen: SubGScreen,
    profile: RadioProfile,
) -> SubGEditorOutcome {
    match screen {
        SubGScreen::Region { cursor } => {
            let SubGRegionChoice::Region(region) = subg_region_choice(cursor) else {
                return SubGEditorOutcome::Cancel;
            };
            let Ok(profile) = apply_region(profile, region) else {
                return SubGEditorOutcome::Cancel;
            };
            SubGEditorOutcome::Stay {
                screen: SubGScreen::Mode { cursor: 0 },
                profile,
            }
        }
        SubGScreen::Mode { cursor } => subg_mode_hold(cursor, profile),
        SubGScreen::Preset { cursor } => {
            match PRESET_CHOICES[cursor.min(PRESET_CHOICES.len() - 1)] {
                PresetChoice::Preset(preset) => SubGEditorOutcome::Stay {
                    screen: SubGScreen::Frequency {
                        cursor: FreqRow::FIRST,
                        edit: EditMode::Browsing,
                    },
                    profile: apply_preset(profile, preset),
                },
                PresetChoice::Custom => SubGEditorOutcome::Stay {
                    screen: SubGScreen::Custom {
                        cursor: CustomRow::FIRST,
                        edit: EditMode::Browsing,
                    },
                    profile,
                },
                PresetChoice::Back => SubGEditorOutcome::Stay {
                    screen: SubGScreen::Mode { cursor: 0 },
                    profile,
                },
            }
        }
        SubGScreen::Frequency { cursor, edit } => lora_frequency_hold(cursor, edit, profile),
        SubGScreen::Custom { cursor, edit } => lora_custom_hold(cursor, edit, profile),
    }
}

fn subg_mode_hold(cursor: usize, profile: RadioProfile) -> SubGEditorOutcome {
    let choices = subg_mode_choices(profile.region());
    match choices[cursor.min(choices.len() - 1)] {
        SubGModeChoice::AutoLoRa => match SubGConfiguration::auto_lora_for(profile.region()) {
            Ok(configuration) => SubGEditorOutcome::Commit(configuration),
            Err(_) => SubGEditorOutcome::Stay {
                screen: SubGScreen::Mode { cursor },
                profile,
            },
        },
        SubGModeChoice::ManualLoRa => SubGEditorOutcome::Stay {
            screen: SubGScreen::Preset {
                cursor: preset_cursor_for(profile.modulation()),
            },
            profile,
        },
        SubGModeChoice::Back => SubGEditorOutcome::Stay {
            screen: SubGScreen::Region {
                cursor: region_index(profile.region()),
            },
            profile,
        },
    }
}

fn manual_configuration(profile: RadioProfile) -> SubGConfiguration {
    SubGConfiguration::manual_lora(profile)
}

fn lora_frequency_hold(
    cursor: FreqRow,
    edit: EditMode,
    profile: RadioProfile,
) -> SubGEditorOutcome {
    match edit {
        EditMode::Freq { place } => match advance_freq_place(profile, place) {
            FreqStep::Place(next_place) => SubGEditorOutcome::Stay {
                screen: SubGScreen::Frequency {
                    cursor,
                    edit: EditMode::Freq { place: next_place },
                },
                profile,
            },
            FreqStep::Done(profile) => SubGEditorOutcome::Stay {
                screen: SubGScreen::Frequency {
                    cursor,
                    edit: EditMode::Browsing,
                },
                profile,
            },
        },
        EditMode::Field => SubGEditorOutcome::Stay {
            screen: SubGScreen::Frequency {
                cursor,
                edit: EditMode::Browsing,
            },
            profile,
        },
        EditMode::Browsing => match cursor {
            FreqRow::Save => SubGEditorOutcome::Commit(manual_configuration(profile)),
            FreqRow::Back => SubGEditorOutcome::Stay {
                screen: SubGScreen::Preset {
                    cursor: preset_cursor_for(profile.modulation()),
                },
                profile,
            },
            FreqRow::Channel => SubGEditorOutcome::Stay {
                screen: SubGScreen::Frequency {
                    cursor,
                    edit: EditMode::Field,
                },
                profile,
            },
            FreqRow::Mhz | FreqRow::Khz => SubGEditorOutcome::Stay {
                screen: SubGScreen::Frequency {
                    cursor,
                    edit: match cursor.freq_first_place() {
                        Some(place) => EditMode::Freq { place },
                        None => EditMode::Browsing,
                    },
                },
                profile,
            },
        },
    }
}

fn lora_custom_hold(cursor: CustomRow, edit: EditMode, profile: RadioProfile) -> SubGEditorOutcome {
    match edit {
        EditMode::Browsing => match cursor {
            CustomRow::Save => SubGEditorOutcome::Commit(manual_configuration(profile)),
            CustomRow::Back => SubGEditorOutcome::Stay {
                screen: SubGScreen::Preset {
                    cursor: preset_cursor_for(profile.modulation()),
                },
                profile,
            },
            _ => SubGEditorOutcome::Stay {
                screen: SubGScreen::Custom {
                    cursor,
                    edit: match cursor.freq_first_place() {
                        Some(place) => EditMode::Freq { place },
                        None => EditMode::Field,
                    },
                },
                profile,
            },
        },
        EditMode::Field => SubGEditorOutcome::Stay {
            screen: SubGScreen::Custom {
                cursor,
                edit: EditMode::Browsing,
            },
            profile,
        },
        EditMode::Freq { place } => match advance_freq_place(profile, place) {
            FreqStep::Place(next_place) => SubGEditorOutcome::Stay {
                screen: SubGScreen::Custom {
                    cursor,
                    edit: EditMode::Freq { place: next_place },
                },
                profile,
            },
            FreqStep::Done(profile) => SubGEditorOutcome::Stay {
                screen: SubGScreen::Custom {
                    cursor,
                    edit: EditMode::Browsing,
                },
                profile,
            },
        },
    }
}
