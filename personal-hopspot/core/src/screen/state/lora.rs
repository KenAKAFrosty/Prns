use personal_rns::interfaces::lora::{
    Frequency, ModemPreset, Modulation, RadioProfile, Region, TxPower,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum LoRaScreen {
    Region { cursor: usize },
    Preset { cursor: usize },
    Frequency { cursor: FreqRow, edit: EditMode },
    Custom { cursor: CustomRow, edit: EditMode },
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

pub(in crate::screen) const PRESET_CHOICES: [PresetChoice; 7] = [
    PresetChoice::Preset(ModemPreset::ShortFast),
    PresetChoice::Preset(ModemPreset::MediumFast),
    PresetChoice::Preset(ModemPreset::LongFast),
    PresetChoice::Preset(ModemPreset::LongSlow),
    PresetChoice::Preset(ModemPreset::Montreal),
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

pub(in crate::screen) const LORA_REGION_CANCEL: usize = Region::ALL.len();
pub(in crate::screen) const LORA_REGION_COUNT: usize = Region::ALL.len() + 1;

pub(in crate::screen) fn region_index(region: Region) -> usize {
    Region::ALL
        .iter()
        .position(|&candidate| candidate == region)
        .unwrap_or(0)
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

fn clamp_freq_to_region(hz: u32, region: Region) -> u32 {
    let (low, high) = region.band();
    hz.clamp(low, high)
}

fn apply_region(profile: RadioProfile, region: Region) -> RadioProfile {
    let mut next = profile;
    if region != profile.region {
        next.frequency = region.default_frequency();
    }
    next.region = region;
    if next.tx_power.dbm() > region.max_tx_power().dbm() {
        next.tx_power = region.max_tx_power();
    }
    next
}

fn apply_preset(profile: RadioProfile, preset: ModemPreset) -> RadioProfile {
    let mut next = profile;
    next.modulation = preset.modulation();
    next
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
    } = profile.modulation;
    let mut next = profile;
    match row {
        CustomRow::SpreadingFactor => {
            next.modulation = Modulation::Lora {
                spreading_factor: spreading_factor.next(),
                bandwidth,
                coding_rate,
            }
        }
        CustomRow::Bandwidth => {
            next.modulation = Modulation::Lora {
                spreading_factor,
                bandwidth: bandwidth.next(),
                coding_rate,
            }
        }
        CustomRow::CodingRate => {
            next.modulation = Modulation::Lora {
                spreading_factor,
                bandwidth,
                coding_rate: coding_rate.next(),
            }
        }
        CustomRow::TxPower => {
            let dbm = profile.tx_power.dbm();
            let ceiling = profile.region.max_tx_power().dbm();
            next.tx_power = TxPower::new(if dbm >= ceiling {
                LORA_TX_POWER_MIN_DBM
            } else {
                dbm + 1
            });
        }
        CustomRow::FreqMhz | CustomRow::FreqKhz | CustomRow::Save | CustomRow::Back => {}
    }
    next
}

pub(in crate::screen) enum LoRaHold {
    Stay {
        screen: LoRaScreen,
        profile: RadioProfile,
    },
    Commit(RadioProfile),
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
    let mut next = profile;
    next.frequency = Frequency::new(bump_freq_place(profile.frequency.hz(), place));
    next
}

fn channel_bandwidth_hz(profile: &RadioProfile) -> u32 {
    let Modulation::Lora { bandwidth, .. } = profile.modulation;
    bandwidth.hz()
}

pub(in crate::screen) fn channel_count(profile: &RadioProfile) -> u32 {
    let (low, high) = profile.region.band();
    ((high - low) / channel_bandwidth_hz(profile)).max(1)
}

fn channel_center_hz(profile: &RadioProfile, channel: u32) -> u32 {
    let (low, _) = profile.region.band();
    let bandwidth = channel_bandwidth_hz(profile);
    low + bandwidth / 2 + channel * bandwidth
}

pub(in crate::screen) fn current_channel(profile: &RadioProfile) -> u32 {
    let (low, _) = profile.region.band();
    let hz = profile.frequency.hz();
    if hz <= low {
        0
    } else {
        ((hz - low) / channel_bandwidth_hz(profile)).min(channel_count(profile) - 1)
    }
}

fn step_freq_channel(profile: RadioProfile) -> RadioProfile {
    let next_channel = (current_channel(&profile) + 1) % channel_count(&profile);
    let mut next = profile;
    next.frequency = Frequency::new(channel_center_hz(&profile, next_channel));
    next
}

fn advance_freq_place(profile: RadioProfile, place: FreqPlace) -> FreqStep {
    match place.next_within_row() {
        Some(next_place) => FreqStep::Place(next_place),
        None => {
            let mut next = profile;
            next.frequency =
                Frequency::new(clamp_freq_to_region(profile.frequency.hz(), profile.region));
            FreqStep::Done(next)
        }
    }
}

pub(in crate::screen) fn lora_editor_tap(
    screen: LoRaScreen,
    profile: RadioProfile,
) -> (LoRaScreen, RadioProfile) {
    match screen {
        LoRaScreen::Region { cursor } => (
            LoRaScreen::Region {
                cursor: (cursor + 1) % LORA_REGION_COUNT,
            },
            profile,
        ),
        LoRaScreen::Preset { cursor } => (
            LoRaScreen::Preset {
                cursor: (cursor + 1) % PRESET_CHOICES.len(),
            },
            profile,
        ),
        LoRaScreen::Frequency { cursor, edit } => match edit {
            EditMode::Freq { place } => (
                LoRaScreen::Frequency { cursor, edit },
                bump_freq(profile, place),
            ),
            EditMode::Field => (
                LoRaScreen::Frequency { cursor, edit },
                step_freq_channel(profile),
            ),
            EditMode::Browsing => (
                LoRaScreen::Frequency {
                    cursor: cursor.next(),
                    edit,
                },
                profile,
            ),
        },
        LoRaScreen::Custom { cursor, edit } => match edit {
            EditMode::Browsing => (
                LoRaScreen::Custom {
                    cursor: cursor.next(),
                    edit,
                },
                profile,
            ),
            EditMode::Field => (
                LoRaScreen::Custom { cursor, edit },
                step_custom_row(profile, cursor),
            ),
            EditMode::Freq { place } => (
                LoRaScreen::Custom { cursor, edit },
                bump_freq(profile, place),
            ),
        },
    }
}

pub(in crate::screen) fn lora_editor_hold(screen: LoRaScreen, profile: RadioProfile) -> LoRaHold {
    match screen {
        LoRaScreen::Region { cursor } => {
            if cursor == LORA_REGION_CANCEL {
                return LoRaHold::Cancel;
            }
            let region = Region::ALL[cursor.min(Region::ALL.len() - 1)];
            let profile = apply_region(profile, region);
            LoRaHold::Stay {
                screen: LoRaScreen::Preset {
                    cursor: preset_cursor_for(profile.modulation),
                },
                profile,
            }
        }
        LoRaScreen::Preset { cursor } => {
            match PRESET_CHOICES[cursor.min(PRESET_CHOICES.len() - 1)] {
                PresetChoice::Preset(preset) => LoRaHold::Stay {
                    screen: LoRaScreen::Frequency {
                        cursor: FreqRow::FIRST,
                        edit: EditMode::Browsing,
                    },
                    profile: apply_preset(profile, preset),
                },
                PresetChoice::Custom => LoRaHold::Stay {
                    screen: LoRaScreen::Custom {
                        cursor: CustomRow::FIRST,
                        edit: EditMode::Browsing,
                    },
                    profile,
                },
                PresetChoice::Back => LoRaHold::Stay {
                    screen: LoRaScreen::Region {
                        cursor: region_index(profile.region),
                    },
                    profile,
                },
            }
        }
        LoRaScreen::Frequency { cursor, edit } => lora_frequency_hold(cursor, edit, profile),
        LoRaScreen::Custom { cursor, edit } => lora_custom_hold(cursor, edit, profile),
    }
}

fn lora_frequency_hold(cursor: FreqRow, edit: EditMode, profile: RadioProfile) -> LoRaHold {
    match edit {
        EditMode::Freq { place } => match advance_freq_place(profile, place) {
            FreqStep::Place(next_place) => LoRaHold::Stay {
                screen: LoRaScreen::Frequency {
                    cursor,
                    edit: EditMode::Freq { place: next_place },
                },
                profile,
            },
            FreqStep::Done(profile) => LoRaHold::Stay {
                screen: LoRaScreen::Frequency {
                    cursor,
                    edit: EditMode::Browsing,
                },
                profile,
            },
        },
        EditMode::Field => LoRaHold::Stay {
            screen: LoRaScreen::Frequency {
                cursor,
                edit: EditMode::Browsing,
            },
            profile,
        },
        EditMode::Browsing => match cursor {
            FreqRow::Save => LoRaHold::Commit(profile),
            FreqRow::Back => LoRaHold::Stay {
                screen: LoRaScreen::Preset {
                    cursor: preset_cursor_for(profile.modulation),
                },
                profile,
            },
            FreqRow::Channel => LoRaHold::Stay {
                screen: LoRaScreen::Frequency {
                    cursor,
                    edit: EditMode::Field,
                },
                profile,
            },
            FreqRow::Mhz | FreqRow::Khz => LoRaHold::Stay {
                screen: LoRaScreen::Frequency {
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

fn lora_custom_hold(cursor: CustomRow, edit: EditMode, profile: RadioProfile) -> LoRaHold {
    match edit {
        EditMode::Browsing => match cursor {
            CustomRow::Save => LoRaHold::Commit(profile),
            CustomRow::Back => LoRaHold::Stay {
                screen: LoRaScreen::Preset {
                    cursor: preset_cursor_for(profile.modulation),
                },
                profile,
            },
            _ => LoRaHold::Stay {
                screen: LoRaScreen::Custom {
                    cursor,
                    edit: match cursor.freq_first_place() {
                        Some(place) => EditMode::Freq { place },
                        None => EditMode::Field,
                    },
                },
                profile,
            },
        },
        EditMode::Field => LoRaHold::Stay {
            screen: LoRaScreen::Custom {
                cursor,
                edit: EditMode::Browsing,
            },
            profile,
        },
        EditMode::Freq { place } => match advance_freq_place(profile, place) {
            FreqStep::Place(next_place) => LoRaHold::Stay {
                screen: LoRaScreen::Custom {
                    cursor,
                    edit: EditMode::Freq { place: next_place },
                },
                profile,
            },
            FreqStep::Done(profile) => LoRaHold::Stay {
                screen: LoRaScreen::Custom {
                    cursor,
                    edit: EditMode::Browsing,
                },
                profile,
            },
        },
    }
}
