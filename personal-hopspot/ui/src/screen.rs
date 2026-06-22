//! The "Personal Hopspot" status screen — portrait (64x128), drawn against any
//! `embedded_graphics` `DrawTarget<Color = BinaryColor>`, so the same pixels land
//! on the S3's SSD1306 OLED and on the Linux debug window's simulator display.
//!
//! A two-line inverted title bar (`Personal` over a **bold** `Hopspot`) above a
//! global menu row and a vertical stack of interface cards. Each card is a name
//! line (icon + label) with its data underneath: stacked up/down Reticulum
//! traffic (3 significant figures, rolling B->K->M->G), a link glyph/count, and
//! a person glyph with the count of destinations the routing table tracks via
//! that interface, followed by live throughput and last-activity age. An interface
//! that's down shows a slashed icon and its traffic line is replaced by
//! `offline`. The glyphs (arrows, link, person, per-interface icon) are drawn
//! primitives, not font characters — the icon mapping is one `match`, the single
//! place to enrich.
//!
//! Portrait puts the global menu and cards down toward the unit's button.
//! [`UiState`] tracks the selected focus item and keeps it visible, so cycling
//! through cards also pages the stack once more interfaces exist than fit on
//! screen. A long press opens either the global dummy menu or the selected
//! interface's dummy menu.

use core::fmt::Write as _;

use embedded_graphics::mono_font::ascii::{FONT_5X8, FONT_6X10, FONT_9X15_BOLD};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use heapless::String as HString;
use personal_rns::interfaces::rns_parity::rnode_lora::core::{
    Frequency, Modulation, RadioProfile, Region, TxPower, DEFAULT_915_PROFILE,
};
use personal_rns::interfaces::InterfaceId;

const WIDTH: i32 = 64;
const HEIGHT: i32 = 128;
const TITLE_H: i32 = 26;
const CARD_TOP: i32 = 27;
const CARD_H: i32 = 40;
const CARD_GAP: i32 = 2;
const CARD_SLOT_STEP: i32 = CARD_H + CARD_GAP;
const GLOBAL_ROW_TOP: i32 = CARD_TOP;
const GLOBAL_ROW_H: i32 = 13;
const GLOBAL_TO_CARD_GAP: i32 = 1;
const FIRST_CARD_WITH_GLOBAL_TOP: i32 = GLOBAL_ROW_TOP + GLOBAL_ROW_H + GLOBAL_TO_CARD_GAP;
const GLOBAL_LABEL: &str = "Menu";
const GLOBAL_ICON_X: i32 = 14;
const GLOBAL_TEXT_X: i32 = GLOBAL_ICON_X + NAME_ICON_W + 2;
const GLOBAL_BACKING_X: i32 = GLOBAL_ICON_X - 2;
const GLOBAL_BACKING_Y: i32 = 1;
const GLOBAL_BACKING_H: u32 = 11;
const INITIAL_VISIBLE_FOCUS_ITEMS: usize = 3;
const SCROLLED_VISIBLE_FOCUS_ITEMS: usize = 2;
const NUMBER_GLYPH_WIDTH: i32 = 5;
const COMPACT_DECIMAL_WIDTH: i32 = 2;
const COMPACT_DECIMAL_Y: i32 = 6;
const COMPACT_SLASH_WIDTH: i32 = 3;
const COMPACT_SLASH_Y: i32 = 2;
const STAT_ICON_X: i32 = 34;
const STAT_TEXT_X: i32 = STAT_ICON_X + 9;
const ACTIVITY_ICON_X: i32 = STAT_ICON_X + 2;
const ACTIVITY_TEXT_X: i32 = ACTIVITY_ICON_X + 9;
const NAME_BACKING_X: i32 = 2;
const NAME_BACKING_Y: i32 = 2;
const NAME_BACKING_H: u32 = 9;
const NAME_ICON_X: i32 = 3;
const NAME_TEXT_X: i32 = 14;
const NAME_LINE_Y: i32 = 2;
const NAME_ICON_W: i32 = 9;
const FONT_6X10_CHAR_W: i32 = 6;
const MENU_ITEM_COUNT: usize = 4;
const MENU_HEADER_Y: i32 = CARD_TOP + 2;
const MENU_SUBTITLE_Y: i32 = CARD_TOP + 13;
const MENU_DIVIDER_Y: i32 = CARD_TOP + 23;
const MENU_ITEM_TOP: i32 = CARD_TOP + 29;
const MENU_ITEM_STEP: i32 = 13;
const MENU_BACKING_X: i32 = 2;
const MENU_BACKING_H: u32 = 10;
const MENU_MARK_X: i32 = 4;
const MENU_TEXT_X: i32 = 12;
const FONT_5X8_CHAR_W: i32 = 5;

/// The card-name font: a fleet member (a [`CardKind::Peer`]) reads one size down, so its id tag
/// fits and it sits visibly under its supervisor.
fn name_font(kind: CardKind) -> &'static MonoFont<'static> {
    match kind {
        CardKind::Peer => &FONT_5X8,
        _ => &FONT_6X10,
    }
}

fn name_char_w(kind: CardKind) -> i32 {
    match kind {
        CardKind::Peer => FONT_5X8_CHAR_W,
        _ => FONT_6X10_CHAR_W,
    }
}

const GLOBAL_MENU_ITEMS: [&str; MENU_ITEM_COUNT] = ["Announce", "Status", "Sleep", "Back"];
const ANNOUNCE_MENU_ITEM: usize = 0;
/// Item 0 of every interface menu is the power toggle; its label is rendered live ("Turn Off" /
/// "Turn On") from the card's [`Liveness`], and long-pressing it emits [`UiAction::ToggleSelectedInterface`].
const POWER_MENU_ITEM: usize = 0;
const USB_MENU_ITEMS: [&str; MENU_ITEM_COUNT] = ["Power", "Serial", "Restart", "Back"];
const WIFI_MENU_ITEMS: [&str; MENU_ITEM_COUNT] = ["Power", "Scan", "Channel", "Back"];
const BLE_MENU_ITEMS: [&str; MENU_ITEM_COUNT] = ["Power", "Pair", "Advert", "Back"];
const LORA_MENU_ITEMS: [&str; MENU_ITEM_COUNT] = ["Power", "Tune", "Reset", "Back"];
const LORA_TUNE_MENU_ITEM: usize = 1;
const LORA_RESET_MENU_ITEM: usize = 2;
const ESP_NOW_MENU_ITEMS: [&str; MENU_ITEM_COUNT] = ["Power", "Peers", "Channel", "Back"];
const TCP_MENU_ITEMS: [&str; MENU_ITEM_COUNT] = ["Power", "Peer", "Drop", "Back"];

/// What interface a card represents — the single source for its icon. Add a
/// variant (and its `match` arm in `draw_interface_icon`) as new interface
/// kinds land; never a wildcard, so the compiler flags the missing glyph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardKind {
    Wifi,
    Usb,
    Ble,
    LoRa,
    EspNow,
    Tcp,
    /// A fleet member a supervisor stood up (a WiFi/USB peer), not an interface a node configured
    /// itself. Renders one font-size down — fits its id tag and reads as subordinate to its parent.
    Peer,
}

/// How alive an interface's card reads. `Live` is a confirmed link — the full
/// card with numbers. `Dormant` is the interface up and watching but with no
/// confirmed link yet (the USB discoverer with nothing plugged): the *live* icon
/// — it is working — over a "Dormant" body, so the card never pretends to carry
/// traffic it has none of. The moment a board connects and handshakes it flips to
/// `Live`; drop every link and it falls back to `Dormant`. `Offline` is a
/// genuinely failed interface — the offline icon and an "Offline" body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Liveness {
    Offline,
    Dormant,
    Live,
    /// Deliberately turned off from the UI: the interface keeps its slot but its driver is dormant.
    /// Distinct from `Offline` (involuntary failure) — it keeps its own interface icon rather than
    /// the failure slash, over an "Off" body, so an interface a user switched off never reads as one
    /// that broke.
    Disabled,
}

impl Liveness {
    fn is_offline(self) -> bool {
        matches!(self, Liveness::Offline)
    }
}

/// The card label's backing buffer — owned, not `&'static str`, so a face can format a runtime tag
/// into it (a discovered peer's id, say) and not just a fixed name. Truncated to the buffer's cap;
/// the panel clips anything past its width.
pub const CARD_LABEL_CAP: usize = 16;
pub type CardLabel = heapless::String<CARD_LABEL_CAP>;

/// Build a [`CardLabel`] from text, truncating to [`CARD_LABEL_CAP`].
#[must_use]
pub fn card_label(text: &str) -> CardLabel {
    let mut label = CardLabel::new();
    for c in text.chars() {
        if label.push(c).is_err() {
            break;
        }
    }
    label
}

/// A TCP client's card name: `TCP ` plus as much of its dial target as fits, so several clients are
/// told apart by where they point (`TCP 162.255.87` vs `TCP schttopup.c`) rather than reading a bare
/// `TCP` on each. Truncates gracefully at the buffer cap; the panel clips the rest.
#[must_use]
pub fn tcp_card_label(target: &str) -> CardLabel {
    let mut label = CardLabel::new();
    let _ = label.push_str("TCP ");
    for c in target.chars() {
        if label.push(c).is_err() {
            break;
        }
    }
    label
}

/// One interface's card. The host fills the identity bits (kind, label) and the
/// live numbers from the interface's status handle.
pub struct Card {
    /// The interface this card stands for, so a face can act on the selected card (turn it off/on)
    /// without a separate index-to-id table.
    pub id: InterfaceId,
    pub kind: CardKind,
    pub label: CardLabel,
    /// Invert the name/icon row for selection or active focus.
    pub selected: bool,
    pub liveness: Liveness,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// Link sessions active on this interface.
    pub links: u32,
    /// Routing-table destinations reachable via this interface.
    pub destinations: u32,
    /// Effective Reticulum throughput over this interface, in bytes per second.
    pub rate_bytes_per_sec: u32,
    /// Age of the most recent observed activity on this interface.
    pub last_activity_secs: Option<u32>,
}

/// What the title-bar battery glyph shows: `Level` (filled segment bars to the
/// given percent) for a present battery, `Charging` (level plus an incoming plug
/// cue), or `Unknown` (a dash) when no plausible battery is detected. Boards
/// without a charge-status signal should keep reporting `Level`/`Unknown`.
#[derive(Clone, Copy)]
pub enum BatteryState {
    Level(u8),
    Charging(u8),
    Unknown,
}

/// Single-button input as interpreted by the board support layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEvent {
    /// Tap/click: advance focus to the next row, card, or open menu item.
    ShortPress,
    /// Hold: open/close the selected global or interface menu.
    LongPress,
}

/// What an input asked the app to do. The UI owns focus and menus; anything
/// that reaches beyond the screen surfaces here for the app to act on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAction {
    None,
    Announce,
    /// Turn the currently selected interface off, or back on if it is already off. The app reads the
    /// selected card's [`id`](Card::id) to know which interface, and flips its enabled state.
    ToggleSelectedInterface,
    OpenLoRaEditor,
    SetLoRaProfile(RadioProfile),
}

/// Lightweight interaction state for the Hopspot card stack.
///
/// The renderer stays data-driven: runtime snapshots become [`Card`]s, while
/// this state only records which focus row/card is selected, which slice of the
/// global/card stack is visible on the 64x128 panel, and whether a menu is open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiState {
    selected_focus: usize,
    visible_start: usize,
    mode: UiMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiMode {
    Cards,
    GlobalMenu {
        selected_item: usize,
    },
    InterfaceMenu {
        selected_item: usize,
        kind: CardKind,
    },
    LoRaEditor {
        cursor: LoRaRow,
        edit: EditMode,
        profile: RadioProfile,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditMode {
    Browsing,
    Field,
    Freq { place: FreqPlace },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FreqPlace {
    Hundreds,
    Tens,
    Ones,
    Tenths,
}

impl FreqPlace {
    const FIRST: Self = Self::Hundreds;

    fn next(self) -> Option<Self> {
        match self {
            Self::Hundreds => Some(Self::Tens),
            Self::Tens => Some(Self::Ones),
            Self::Ones => Some(Self::Tenths),
            Self::Tenths => None,
        }
    }

    fn digit_step_hz(self) -> u32 {
        match self {
            Self::Hundreds => 100_000_000,
            Self::Tens => 10_000_000,
            Self::Ones => 1_000_000,
            Self::Tenths => 100_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoRaRow {
    SpreadingFactor,
    Bandwidth,
    CodingRate,
    Frequency,
    TxPower,
    Region,
    Save,
}

const LORA_ROWS: [LoRaRow; 7] = [
    LoRaRow::SpreadingFactor,
    LoRaRow::Bandwidth,
    LoRaRow::CodingRate,
    LoRaRow::Frequency,
    LoRaRow::TxPower,
    LoRaRow::Region,
    LoRaRow::Save,
];

impl LoRaRow {
    const FIRST: Self = Self::SpreadingFactor;

    fn next(self) -> Self {
        match self {
            Self::SpreadingFactor => Self::Bandwidth,
            Self::Bandwidth => Self::CodingRate,
            Self::CodingRate => Self::Frequency,
            Self::Frequency => Self::TxPower,
            Self::TxPower => Self::Region,
            Self::Region => Self::Save,
            Self::Save => Self::SpreadingFactor,
        }
    }

    fn is_save(self) -> bool {
        matches!(self, Self::Save)
    }
}

const LORA_EU_MIN_HZ: u32 = 863_000_000;
const LORA_EU_MAX_HZ: u32 = 870_000_000;
const LORA_US_MIN_HZ: u32 = 902_000_000;
const LORA_US_MAX_HZ: u32 = 928_000_000;
const LORA_TX_POWER_MIN_DBM: i8 = -9;
const LORA_TX_POWER_MAX_DBM: i8 = 22;

fn freq_hundreds_digit(hz: u32) -> u32 {
    (hz / 100_000_000) % 10
}

fn bump_freq_place(hz: u32, place: FreqPlace) -> u32 {
    let step = place.digit_step_hz();
    let decade = step * 10;
    let above = (hz / decade) * decade;
    let within = hz % decade;
    let lower = within % step;
    let digit = within / step;
    let bumped = match place {
        FreqPlace::Hundreds => {
            if digit == 8 {
                9
            } else {
                8
            }
        }
        _ => (digit + 1) % 10,
    };
    above + bumped * step + lower
}

fn clamp_freq_hz(hz: u32) -> u32 {
    if freq_hundreds_digit(hz) == 9 {
        hz.clamp(LORA_US_MIN_HZ, LORA_US_MAX_HZ)
    } else {
        hz.clamp(LORA_EU_MIN_HZ, LORA_EU_MAX_HZ)
    }
}

fn step_lora_row(profile: RadioProfile, row: LoRaRow) -> RadioProfile {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    let mut next = profile;
    match row {
        LoRaRow::SpreadingFactor => {
            next.modulation = Modulation::Lora {
                spreading_factor: spreading_factor.next(),
                bandwidth,
                coding_rate,
            }
        }
        LoRaRow::Bandwidth => {
            next.modulation = Modulation::Lora {
                spreading_factor,
                bandwidth: bandwidth.next(),
                coding_rate,
            }
        }
        LoRaRow::CodingRate => {
            next.modulation = Modulation::Lora {
                spreading_factor,
                bandwidth,
                coding_rate: coding_rate.next(),
            }
        }
        LoRaRow::Frequency => {}
        LoRaRow::TxPower => {
            let dbm = profile.tx_power.dbm();
            next.tx_power = TxPower::new(if dbm >= LORA_TX_POWER_MAX_DBM {
                LORA_TX_POWER_MIN_DBM
            } else {
                dbm + 1
            });
        }
        LoRaRow::Region => next.region = profile.region.next(),
        LoRaRow::Save => {}
    }
    next
}

impl UiState {
    pub const fn new() -> Self {
        Self {
            selected_focus: 0,
            visible_start: 0,
            mode: UiMode::Cards,
        }
    }

    /// Whether the global action row is selected while browsing.
    pub fn global_selected(&self) -> bool {
        matches!(self.mode, UiMode::Cards) && self.selected_focus == 0
    }

    /// The selected card index, if any cards are present.
    pub fn selected_card(&self, card_count: usize) -> Option<usize> {
        let card_index = self.selected_focus.checked_sub(1)?;
        if card_index < card_count {
            Some(card_index)
        } else {
            None
        }
    }

    /// The first focus item currently visible on the panel.
    ///
    /// `0` is the global action row; `1..` are interface cards shifted by one.
    pub fn visible_start(&self, card_count: usize) -> usize {
        visible_start_for(
            focus_item_count(card_count),
            self.selected_focus,
            self.visible_start,
        )
    }

    /// The selected menu row while any menu is open.
    pub fn menu_selected_item(&self) -> Option<usize> {
        match self.mode {
            UiMode::GlobalMenu { selected_item } | UiMode::InterfaceMenu { selected_item, .. } => {
                Some(selected_item)
            }
            UiMode::Cards | UiMode::LoRaEditor { .. } => None,
        }
    }

    /// The selected menu row while the global menu is open.
    pub fn global_menu_selected_item(&self) -> Option<usize> {
        match self.mode {
            UiMode::GlobalMenu { selected_item } => Some(selected_item),
            UiMode::Cards | UiMode::InterfaceMenu { .. } | UiMode::LoRaEditor { .. } => None,
        }
    }

    /// The selected menu row while an interface menu is open.
    pub fn interface_menu_selected_item(&self) -> Option<usize> {
        match self.mode {
            UiMode::InterfaceMenu { selected_item, .. } => Some(selected_item),
            UiMode::Cards | UiMode::GlobalMenu { .. } | UiMode::LoRaEditor { .. } => None,
        }
    }

    pub fn open_lora_editor(&mut self, profile: RadioProfile) {
        self.mode = UiMode::LoRaEditor {
            cursor: LoRaRow::FIRST,
            edit: EditMode::Browsing,
            profile,
        };
    }

    /// Reconcile selection/window state after the runtime's interface list
    /// changes.
    pub fn sync_card_count(&mut self, card_count: usize) {
        let item_count = focus_item_count(card_count);
        self.selected_focus = self.selected_focus.min(item_count - 1);
        self.visible_start = visible_start_for(item_count, self.selected_focus, self.visible_start);

        match self.mode {
            UiMode::Cards | UiMode::GlobalMenu { .. } | UiMode::LoRaEditor { .. } => {}
            UiMode::InterfaceMenu { .. } if self.selected_card(card_count).is_none() => {
                self.mode = UiMode::Cards;
            }
            UiMode::InterfaceMenu {
                selected_item,
                kind,
            } => {
                self.mode = UiMode::InterfaceMenu {
                    selected_item: selected_item.min(MENU_ITEM_COUNT - 1),
                    kind,
                };
            }
        }
        if let UiMode::GlobalMenu { selected_item } = self.mode {
            self.mode = UiMode::GlobalMenu {
                selected_item: selected_item.min(MENU_ITEM_COUNT - 1),
            };
        }
    }

    /// Apply one single-button event, returning what the app should do about it. `selected_kind` is
    /// the [`CardKind`] of the currently selected card (the app reads it from the card list), so the
    /// interface menu can resolve its kind-specific items.
    pub fn handle_input(
        &mut self,
        event: InputEvent,
        card_count: usize,
        selected_kind: Option<CardKind>,
    ) -> UiAction {
        self.sync_card_count(card_count);
        let item_count = focus_item_count(card_count);
        let action = match (event, self.mode) {
            (InputEvent::ShortPress, UiMode::Cards) => {
                self.selected_focus = (self.selected_focus + 1) % item_count;
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::Cards) if self.selected_focus == 0 => {
                self.mode = UiMode::GlobalMenu { selected_item: 0 };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::Cards) => {
                if let Some(kind) = selected_kind {
                    if self.selected_card(card_count).is_some() {
                        self.mode = UiMode::InterfaceMenu {
                            selected_item: 0,
                            kind,
                        };
                    }
                }
                UiAction::None
            }
            (InputEvent::ShortPress, UiMode::GlobalMenu { selected_item }) => {
                self.mode = UiMode::GlobalMenu {
                    selected_item: (selected_item + 1) % MENU_ITEM_COUNT,
                };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::GlobalMenu { selected_item }) => {
                self.mode = UiMode::Cards;
                if selected_item == ANNOUNCE_MENU_ITEM {
                    UiAction::Announce
                } else {
                    UiAction::None
                }
            }
            (
                InputEvent::ShortPress,
                UiMode::InterfaceMenu {
                    selected_item,
                    kind,
                },
            ) => {
                self.mode = UiMode::InterfaceMenu {
                    selected_item: (selected_item + 1) % MENU_ITEM_COUNT,
                    kind,
                };
                UiAction::None
            }
            (
                InputEvent::LongPress,
                UiMode::InterfaceMenu {
                    selected_item,
                    kind,
                },
            ) => {
                self.mode = UiMode::Cards;
                match (kind, selected_item) {
                    (_, POWER_MENU_ITEM) => UiAction::ToggleSelectedInterface,
                    (CardKind::LoRa, LORA_TUNE_MENU_ITEM) => UiAction::OpenLoRaEditor,
                    (CardKind::LoRa, LORA_RESET_MENU_ITEM) => {
                        UiAction::SetLoRaProfile(DEFAULT_915_PROFILE)
                    }
                    _ => UiAction::None,
                }
            }
            (
                InputEvent::ShortPress,
                UiMode::LoRaEditor {
                    cursor,
                    edit,
                    profile,
                },
            ) => {
                self.mode = match edit {
                    EditMode::Browsing => UiMode::LoRaEditor {
                        cursor: cursor.next(),
                        edit,
                        profile,
                    },
                    EditMode::Field => UiMode::LoRaEditor {
                        cursor,
                        edit,
                        profile: step_lora_row(profile, cursor),
                    },
                    EditMode::Freq { place } => {
                        let mut next = profile;
                        next.frequency =
                            Frequency::new(bump_freq_place(profile.frequency.hz(), place));
                        UiMode::LoRaEditor {
                            cursor,
                            edit,
                            profile: next,
                        }
                    }
                };
                UiAction::None
            }
            (
                InputEvent::LongPress,
                UiMode::LoRaEditor {
                    cursor,
                    edit,
                    profile,
                },
            ) => match edit {
                EditMode::Browsing => {
                    if cursor.is_save() {
                        self.mode = UiMode::Cards;
                        UiAction::SetLoRaProfile(profile)
                    } else {
                        self.mode = UiMode::LoRaEditor {
                            cursor,
                            edit: if matches!(cursor, LoRaRow::Frequency) {
                                EditMode::Freq {
                                    place: FreqPlace::FIRST,
                                }
                            } else {
                                EditMode::Field
                            },
                            profile,
                        };
                        UiAction::None
                    }
                }
                EditMode::Field => {
                    self.mode = UiMode::LoRaEditor {
                        cursor,
                        edit: EditMode::Browsing,
                        profile,
                    };
                    UiAction::None
                }
                EditMode::Freq { place } => {
                    self.mode = match place.next() {
                        Some(next_place) => UiMode::LoRaEditor {
                            cursor,
                            edit: EditMode::Freq { place: next_place },
                            profile,
                        },
                        None => {
                            let mut next = profile;
                            next.frequency = Frequency::new(clamp_freq_hz(profile.frequency.hz()));
                            UiMode::LoRaEditor {
                                cursor,
                                edit: EditMode::Browsing,
                                profile: next,
                            }
                        }
                    };
                    UiAction::None
                }
            },
        };
        self.sync_card_count(card_count);
        action
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self::new()
    }
}

fn focus_item_count(card_count: usize) -> usize {
    card_count + 1
}

fn visible_start_for(item_count: usize, selected_focus: usize, visible_start: usize) -> usize {
    if item_count <= INITIAL_VISIBLE_FOCUS_ITEMS || selected_focus < INITIAL_VISIBLE_FOCUS_ITEMS {
        return 0;
    }

    let max_start = item_count
        .saturating_sub(SCROLLED_VISIBLE_FOCUS_ITEMS)
        .max(1);
    let visible_start = visible_start.clamp(1, max_start);
    if selected_focus < visible_start {
        selected_focus.max(1)
    } else if selected_focus >= visible_start + SCROLLED_VISIBLE_FOCUS_ITEMS {
        (selected_focus + 1 - SCROLLED_VISIBLE_FOCUS_ITEMS).min(max_start)
    } else {
        visible_start
    }
}

/// 3 significant figures, rolling unit B -> K -> M -> G (1000-based), max 3
/// numeric chars: `1.0K` up to `10K` up to `100K`, then `1.0M`, and so on.
/// Integer-only (no float), max 4 chars including the unit.
fn fmt_bytes(n: u64) -> HString<8> {
    let mut s = HString::new();
    if n < 1000 {
        let _ = write!(s, "{n}B");
        return s;
    }
    let (unit, unit_val) = if n < 1_000_000 {
        ('K', 1_000u64)
    } else if n < 1_000_000_000 {
        ('M', 1_000_000)
    } else {
        ('G', 1_000_000_000)
    };
    // value-in-the-unit scaled by 1000 (thousandths of the unit): [1000, 999_999]
    let thousandths = n * 1000 / unit_val;
    let int_part = thousandths / 1000; // [1, 999]
    if int_part < 10 {
        // one decimal: 1.0 .. 9.9
        let tenths = thousandths / 100;
        let _ = write!(s, "{}.{}{}", tenths / 10, tenths % 10, unit);
    } else {
        // whole: 10 .. 999
        let _ = write!(s, "{int_part}{unit}");
    }
    s
}

fn fmt_count(n: u32) -> HString<8> {
    let mut s = HString::new();
    if n < 1000 {
        let _ = write!(s, "{n}");
        return s;
    }

    let n = n as u64;
    let (unit, unit_val) = if n < 1_000_000 {
        ('K', 1_000u64)
    } else if n < 1_000_000_000 {
        ('M', 1_000_000)
    } else {
        ('B', 1_000_000_000)
    };
    let thousandths = n * 1000 / unit_val;
    let int_part = thousandths / 1000;
    if int_part < 10 {
        let tenths = thousandths / 100;
        let _ = write!(s, "{}.{}{}", tenths / 10, tenths % 10, unit);
    } else {
        let _ = write!(s, "{int_part}{unit}");
    }
    s
}

fn fmt_rate_bytes_per_sec(n: u32) -> HString<8> {
    let mut s = HString::new();
    if n < 1000 {
        let _ = write!(s, "{n}/s");
        return s;
    }

    let n = n as u64;
    let (unit, unit_val) = if n < 1_000_000 {
        ('K', 1_000u64)
    } else if n < 1_000_000_000 {
        ('M', 1_000_000)
    } else {
        ('G', 1_000_000_000)
    };
    let thousandths = n * 1000 / unit_val;
    let int_part = thousandths / 1000;
    if int_part < 10 {
        let tenths = thousandths / 100;
        let _ = write!(s, "{}.{}{}/s", tenths / 10, tenths % 10, unit);
    } else if int_part < 100 {
        let _ = write!(s, "{int_part}{unit}/s");
    } else {
        let _ = write!(s, "{int_part}{unit}s");
    }
    s
}

fn fmt_activity_age(age_secs: Option<u32>) -> HString<8> {
    let mut s = HString::new();
    match age_secs {
        None => {
            let _ = write!(s, "-");
        }
        Some(0) => {
            let _ = write!(s, "now");
        }
        Some(seconds) if seconds < 60 => {
            let _ = write!(s, "{seconds}s");
        }
        Some(seconds) if seconds < 3600 => {
            let _ = write!(s, "{}m", seconds / 60);
        }
        Some(seconds) => {
            let hours = (seconds / 3600).min(99);
            let _ = write!(s, "{hours}h");
        }
    }
    s
}

#[cfg(test)]
fn compact_numeric_width(text: &str) -> i32 {
    text.chars()
        .map(|ch| {
            if ch == '.' {
                COMPACT_DECIMAL_WIDTH
            } else if ch == '/' {
                COMPACT_SLASH_WIDTH
            } else {
                NUMBER_GLYPH_WIDTH
            }
        })
        .sum()
}

fn draw_compact_number<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    text: &str,
    point: Point,
    color: BinaryColor,
) {
    let style = MonoTextStyle::new(&FONT_5X8, color);
    let mut x = point.x;
    for ch in text.chars() {
        if ch == '.' {
            let _ = Rectangle::new(Point::new(x, point.y + COMPACT_DECIMAL_Y), Size::new(1, 1))
                .into_styled(fill(color))
                .draw(display);
            x += COMPACT_DECIMAL_WIDTH;
            continue;
        }

        if ch == '/' {
            for (dx, dy) in [(2, 0), (1, 1), (0, 2)] {
                let _ = Rectangle::new(
                    Point::new(x + dx, point.y + COMPACT_SLASH_Y + dy),
                    Size::new(1, 1),
                )
                .into_styled(fill(color))
                .draw(display);
            }
            x += COMPACT_SLASH_WIDTH;
            continue;
        }

        let mut glyph: HString<2> = HString::new();
        let _ = glyph.push(ch);
        let _ =
            Text::with_baseline(&glyph, Point::new(x, point.y), style, Baseline::Top).draw(display);
        x += NUMBER_GLYPH_WIDTH;
    }
}

fn selected_name_backing_width(label: &str, char_w: i32) -> u32 {
    let label_right = NAME_TEXT_X + label.chars().count() as i32 * char_w + 1;
    let icon_right = NAME_ICON_X + NAME_ICON_W + 1;
    let content_right = label_right.max(icon_right).min(WIDTH - 1);
    (content_right - NAME_BACKING_X).max(0) as u32
}

fn interface_menu_items(kind: CardKind) -> &'static [&'static str; MENU_ITEM_COUNT] {
    match kind {
        CardKind::Wifi | CardKind::Peer => &WIFI_MENU_ITEMS,
        CardKind::Usb => &USB_MENU_ITEMS,
        CardKind::Ble => &BLE_MENU_ITEMS,
        CardKind::LoRa => &LORA_MENU_ITEMS,
        CardKind::EspNow => &ESP_NOW_MENU_ITEMS,
        CardKind::Tcp => &TCP_MENU_ITEMS,
    }
}

fn menu_item_backing_width(label: &str) -> u32 {
    let text_right = MENU_TEXT_X + label.chars().count() as i32 * FONT_5X8_CHAR_W + 1;
    (text_right - MENU_BACKING_X).max(0) as u32
}

fn global_row_backing_width() -> u32 {
    let label_right = GLOBAL_TEXT_X + GLOBAL_LABEL.chars().count() as i32 * FONT_6X10_CHAR_W + 2;
    (label_right - GLOBAL_BACKING_X).max(0) as u32
}

fn fill(color: BinaryColor) -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_fill(color)
}

fn stroke(color: BinaryColor) -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_stroke(color, 1)
}

fn line<D: DrawTarget<Color = BinaryColor>>(display: &mut D, a: Point, b: Point) {
    line_colored(display, a, b, BinaryColor::On);
}

fn line_colored<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    a: Point,
    b: Point,
    color: BinaryColor,
) {
    let _ = Line::new(a, b).into_styled(stroke(color)).draw(display);
}

fn draw_pattern_colored<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    rows: &[&str],
    color: BinaryColor,
) {
    for (row_index, row) in rows.iter().enumerate() {
        for (col_index, pixel) in row.as_bytes().iter().enumerate() {
            if *pixel == b'#' {
                let _ = Rectangle::new(
                    Point::new(x + col_index as i32, y + row_index as i32),
                    Size::new(1, 1),
                )
                .into_styled(fill(color))
                .draw(display);
            }
        }
    }
}

/// A battery glyph drawn in the background color (it sits on the inverted title
/// bar): a 15x9 outline + left terminal nub, then either four filled segment
/// bars (to the nearest quarter) for a present battery, an incoming plug cue
/// for charging, or a dash for unknown. The bars are inset 1px from the outline
/// on each side for breathing room.
fn draw_battery<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    state: BatteryState,
) {
    let outline = stroke(BinaryColor::Off);
    let solid = fill(BinaryColor::Off);
    let _ = Rectangle::new(Point::new(x, y), Size::new(15, 9))
        .into_styled(outline)
        .draw(display);
    let _ = Rectangle::new(Point::new(x - 2, y + 3), Size::new(2, 3))
        .into_styled(solid)
        .draw(display);
    match state {
        BatteryState::Level(pct) | BatteryState::Charging(pct) => {
            // Four segments (2px bar + 1px gap) inset 1px inside the outline, so
            // they span x+2..x+12; filled to the nearest quarter — coarse by
            // design.
            let filled = (pct as u32 * 4 + 50) / 100;
            for i in 0..filled.min(4) {
                let bar_x = x + 2 + i as i32 * 3;
                let _ = Rectangle::new(Point::new(bar_x, y + 2), Size::new(2, 5))
                    .into_styled(solid)
                    .draw(display);
            }
            if matches!(state, BatteryState::Charging(_)) {
                let _ = Line::new(Point::new(x + 15, y + 4), Point::new(x + 18, y + 4))
                    .into_styled(outline)
                    .draw(display);
                let _ = Rectangle::new(Point::new(x + 19, y + 3), Size::new(2, 3))
                    .into_styled(solid)
                    .draw(display);
            }
        }
        BatteryState::Unknown => {
            let _ = Line::new(Point::new(x + 4, y + 4), Point::new(x + 10, y + 4))
                .into_styled(outline)
                .draw(display);
        }
    }
}

/// The two-line inverted title bar: a small left-aligned `Personal` with a
/// battery glyph on the right, over a big bold `Hopspot`, knocked out of a
/// filled bar.
fn draw_title_bar<D: DrawTarget<Color = BinaryColor>>(display: &mut D, battery: BatteryState) {
    let _ = Rectangle::new(Point::new(0, 0), Size::new(WIDTH as u32, TITLE_H as u32))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    // Line 1: small left "Personal" (8*5=40px) + battery on the right.
    let small = MonoTextStyle::new(&FONT_5X8, BinaryColor::Off);
    let _ = Text::with_baseline("Personal", Point::new(2, 1), small, Baseline::Top).draw(display);
    // x=45: the 2px nub starts at col 43 and the 15px outline ends at col 59,
    // leaving the right edge free for a future charging/plug indicator.
    draw_battery(display, 45, 1, battery);
    // Line 2: big bold "Hopspot" (7*9=63px, fills the width).
    let big = MonoTextStyle::new(&FONT_9X15_BOLD, BinaryColor::Off);
    let _ = Text::with_baseline("Hopspot", Point::new(1, 10), big, Baseline::Top).draw(display);
}

/// A thin up (`up`) or down arrow: a shortened 1px shaft with a small chevron
/// head, 5px wide and 7px tall, fitting a text row at `y`.
fn draw_arrow<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32, up: bool) {
    let cx = x + 2;
    // Shaft: down arrows omit the top pixel to open the stacked-row gap.
    let shaft_start = if up { y } else { y + 1 };
    line(display, Point::new(cx, shaft_start), Point::new(cx, y + 5));
    // head: chevron at the leading end
    let (tip, wing) = if up { (y, y + 2) } else { (y + 6, y + 4) };
    line(display, Point::new(cx, tip), Point::new(x, wing));
    line(display, Point::new(cx, tip), Point::new(x + 4, wing));
}

/// A tiny head-and-shoulders outline, ~9x7px.
fn draw_person<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    line(display, Point::new(x + 3, y), Point::new(x + 5, y));
    line(display, Point::new(x + 2, y + 1), Point::new(x + 2, y + 2));
    line(display, Point::new(x + 6, y + 1), Point::new(x + 6, y + 2));
    line(display, Point::new(x + 3, y + 3), Point::new(x + 5, y + 3));
    line(display, Point::new(x + 2, y + 4), Point::new(x + 1, y + 5));
    line(display, Point::new(x + 6, y + 4), Point::new(x + 7, y + 5));
}

/// A tiny two-loop chain outline, ~8x6px.
fn draw_link<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    line(display, Point::new(x + 1, y), Point::new(x + 2, y));
    line(display, Point::new(x, y + 1), Point::new(x, y + 4));
    line(display, Point::new(x + 1, y + 5), Point::new(x + 2, y + 5));
    line(display, Point::new(x + 5, y), Point::new(x + 6, y));
    line(display, Point::new(x + 7, y + 1), Point::new(x + 7, y + 4));
    line(display, Point::new(x + 5, y + 5), Point::new(x + 6, y + 5));
    let _ = Rectangle::new(Point::new(x + 4, y + 2), Size::new(1, 1))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    let _ = Rectangle::new(Point::new(x + 3, y + 3), Size::new(1, 1))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
}

fn draw_lightning<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    draw_pattern_colored(
        display,
        x,
        y,
        &[
            "   # ", "  #  ", " ####", "  #  ", " #   ", "#    ", "     ",
        ],
        BinaryColor::On,
    );
}

fn draw_clock<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    draw_pattern_colored(
        display,
        x,
        y,
        &[
            "  ###  ", " #   # ", "#  #  #", "#  ## #", "#     #", " #   # ", "  ###  ",
        ],
        BinaryColor::On,
    );
}

fn draw_offline_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    color: BinaryColor,
) {
    line_colored(
        display,
        Point::new(x + 2, y + 6),
        Point::new(x + 8, y),
        color,
    );
}

fn draw_menu_cursor<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    color: BinaryColor,
) {
    line_colored(
        display,
        Point::new(x, y + 2),
        Point::new(x + 3, y + 4),
        color,
    );
    line_colored(
        display,
        Point::new(x, y + 6),
        Point::new(x + 3, y + 4),
        color,
    );
}

fn draw_global_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    color: BinaryColor,
) {
    draw_pattern_colored(
        display,
        x,
        y,
        &[
            "#######  ",
            "         ",
            "  #####  ",
            "         ",
            "#######  ",
            "         ",
            "  #####  ",
            "         ",
            "#######  ",
        ],
        color,
    );
}

/// The per-interface icon — the one place that maps a [`CardKind`] to a glyph.
fn draw_interface_icon<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    kind: CardKind,
    color: BinaryColor,
) {
    match kind {
        // WiFi: the familiar status-bar arc stack, pixel-reduced to 9px. A peer reuses it.
        CardKind::Wifi | CardKind::Peer => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "  #####  ",
                    " #     # ",
                    "#       #",
                    "         ",
                    "   ###   ",
                    "  #   #  ",
                    "         ",
                    "    #    ",
                    "   ###   ",
                ],
                color,
            );
        }
        // USB: a connector "mouth" with a full-width plastic tongue + cable stub.
        CardKind::Usb => {
            line_colored(
                display,
                Point::new(x + 4, y),
                Point::new(x + 4, y + 2),
                color,
            );
            let _ = Rectangle::new(Point::new(x, y + 2), Size::new(9, 6))
                .into_styled(stroke(color))
                .draw(display);
            let _ = Line::new(Point::new(x + 1, y + 5), Point::new(x + 7, y + 5))
                .into_styled(stroke(color))
                .draw(display);
        }
        // BLE: pixel-reduced Bluetooth rune with its center spine, crossing
        // left stroke, and the paired right-side branches.
        CardKind::Ble => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "    #    ",
                    "    ##   ",
                    "#   # #  ",
                    " #  #  # ",
                    "  ####   ",
                    " #  #  # ",
                    "#   # #  ",
                    "    ##   ",
                    "    #    ",
                ],
                color,
            );
        }
        // LoRa: long-range radio, rendered as a mast with symmetric RF lobes.
        CardKind::LoRa => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "#   #   #",
                    " #  #  # ",
                    "  # # #  ",
                    "   ###   ",
                    "    #    ",
                    "    #    ",
                    "    #    ",
                    "   ###   ",
                    "  #####  ",
                ],
                color,
            );
        }
        // ESP-NOW: an omni broadcast node — a center dot with a wave opening to
        // each side (distinct from WiFi's upward arcs and LoRa's antenna).
        CardKind::EspNow => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "         ",
                    "#       #",
                    " #     # ",
                    "  # # #  ",
                    "   ###   ",
                    "  # # #  ",
                    " #     # ",
                    "#       #",
                    "         ",
                ],
                color,
            );
        }
        // TCP: a two-way exchange — a right-arrow over a left-arrow for the
        // reliable bidirectional stream.
        CardKind::Tcp => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "         ",
                    "         ",
                    "      #  ",
                    " ####### ",
                    "      #  ",
                    "  #      ",
                    " ####### ",
                    "  #      ",
                    "         ",
                ],
                color,
            );
        }
    }
}

/// Draw one card: an outlined box with a name line (icon + label) and, beneath
/// it, traffic and peers. `top` is the box's top edge.
fn draw_card<D: DrawTarget<Color = BinaryColor>>(display: &mut D, top: i32, card: &Card) {
    draw_card_with_selection(display, top, card, card.selected);
}

fn draw_card_with_selection<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    card: &Card,
    selected: bool,
) {
    let _ = Rectangle::new(Point::new(0, top), Size::new(WIDTH as u32, CARD_H as u32))
        .into_styled(stroke(BinaryColor::On))
        .draw(display);

    let name_color = if selected {
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    if selected {
        let _ = Rectangle::new(
            Point::new(NAME_BACKING_X, top + NAME_BACKING_Y),
            Size::new(
                selected_name_backing_width(&card.label, name_char_w(card.kind)),
                NAME_BACKING_H,
            ),
        )
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    }

    let label_style = MonoTextStyle::new(name_font(card.kind), name_color);
    let num_style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);

    // Name line: [icon] label. Dormant keeps the live icon — only the body changes.
    if card.liveness.is_offline() {
        draw_offline_icon(display, NAME_ICON_X, top + NAME_LINE_Y + 1, name_color);
    } else {
        draw_interface_icon(
            display,
            NAME_ICON_X,
            top + NAME_LINE_Y,
            card.kind,
            name_color,
        );
    }
    let _ = Text::with_baseline(
        &card.label,
        Point::new(NAME_TEXT_X, top + NAME_LINE_Y),
        label_style,
        Baseline::Top,
    )
    .draw(display);

    // Traffic rows for a live link, or a whole-card word for one that is down or
    // up-but-quiet.
    let tx_y = top + 13;
    let rx_y = top + 22;
    let live_y = top + 31;
    let whole_card_word = match card.liveness {
        Liveness::Offline => Some("Offline"),
        Liveness::Dormant => Some("Dormant"),
        Liveness::Disabled => Some("Off"),
        Liveness::Live => None,
    };
    if let Some(word) = whole_card_word {
        let _ = Text::with_baseline(word, Point::new(16, top + 20), num_style, Baseline::Top)
            .draw(display);
        return;
    }

    draw_arrow(display, 2, tx_y + 1, true);
    let tx_bytes = fmt_bytes(card.tx_bytes);
    draw_compact_number(
        display,
        tx_bytes.as_str(),
        Point::new(8, tx_y),
        BinaryColor::On,
    );
    draw_arrow(display, 2, rx_y, false);
    let rx_bytes = fmt_bytes(card.rx_bytes);
    draw_compact_number(
        display,
        rx_bytes.as_str(),
        Point::new(8, rx_y),
        BinaryColor::On,
    );

    // Destination and link counters sit in a compact right-side stats column.
    draw_person(display, STAT_ICON_X, tx_y + 1);
    let destinations = fmt_count(card.destinations);
    draw_compact_number(
        display,
        destinations.as_str(),
        Point::new(STAT_TEXT_X, tx_y),
        BinaryColor::On,
    );
    draw_link(display, STAT_ICON_X, rx_y + 1);
    let links = fmt_count(card.links);
    draw_compact_number(
        display,
        links.as_str(),
        Point::new(STAT_TEXT_X, rx_y),
        BinaryColor::On,
    );

    draw_lightning(display, 2, live_y + 1);
    let rate = fmt_rate_bytes_per_sec(card.rate_bytes_per_sec);
    draw_compact_number(
        display,
        rate.as_str(),
        Point::new(8, live_y),
        BinaryColor::On,
    );
    draw_clock(display, ACTIVITY_ICON_X, live_y + 1);
    let age = fmt_activity_age(card.last_activity_secs);
    draw_compact_number(
        display,
        age.as_str(),
        Point::new(ACTIVITY_TEXT_X, live_y),
        BinaryColor::On,
    );
}

fn draw_card_peek<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    card: &Card,
    selected: bool,
) {
    line(display, Point::new(0, top), Point::new(WIDTH - 1, top));
    line(display, Point::new(0, top), Point::new(0, HEIGHT - 1));
    line(
        display,
        Point::new(WIDTH - 1, top),
        Point::new(WIDTH - 1, HEIGHT - 1),
    );

    if top + NAME_LINE_Y + 9 >= HEIGHT {
        return;
    }

    let name_color = if selected {
        let _ = Rectangle::new(
            Point::new(NAME_BACKING_X, top + NAME_BACKING_Y),
            Size::new(
                selected_name_backing_width(&card.label, name_char_w(card.kind)),
                NAME_BACKING_H,
            ),
        )
        .into_styled(fill(BinaryColor::On))
        .draw(display);
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    let label_style = MonoTextStyle::new(name_font(card.kind), name_color);
    if card.liveness.is_offline() {
        draw_offline_icon(display, NAME_ICON_X, top + NAME_LINE_Y + 1, name_color);
    } else {
        draw_interface_icon(
            display,
            NAME_ICON_X,
            top + NAME_LINE_Y,
            card.kind,
            name_color,
        );
    }
    let _ = Text::with_baseline(
        &card.label,
        Point::new(NAME_TEXT_X, top + NAME_LINE_Y),
        label_style,
        Baseline::Top,
    )
    .draw(display);
}

fn draw_global_row<D: DrawTarget<Color = BinaryColor>>(display: &mut D, top: i32, selected: bool) {
    let row_color = if selected {
        let _ = Rectangle::new(
            Point::new(GLOBAL_BACKING_X, top + GLOBAL_BACKING_Y),
            Size::new(global_row_backing_width(), GLOBAL_BACKING_H),
        )
        .into_styled(fill(BinaryColor::On))
        .draw(display);
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    let label_style = MonoTextStyle::new(&FONT_6X10, row_color);
    draw_global_icon(display, GLOBAL_ICON_X, top + NAME_LINE_Y, row_color);
    let _ = Text::with_baseline(
        GLOBAL_LABEL,
        Point::new(GLOBAL_TEXT_X, top + NAME_LINE_Y),
        label_style,
        Baseline::Top,
    )
    .draw(display);
}

fn draw_menu_item<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    y: i32,
    label: &str,
    selected: bool,
) {
    let color = if selected {
        let _ = Rectangle::new(
            Point::new(MENU_BACKING_X, y - 1),
            Size::new(menu_item_backing_width(label), MENU_BACKING_H),
        )
        .into_styled(fill(BinaryColor::On))
        .draw(display);
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    let style = MonoTextStyle::new(&FONT_5X8, color);
    draw_menu_cursor(display, MENU_MARK_X, y, color);
    let _ =
        Text::with_baseline(label, Point::new(MENU_TEXT_X, y), style, Baseline::Top).draw(display);
}

fn draw_global_menu<D: DrawTarget<Color = BinaryColor>>(display: &mut D, selected_item: usize) {
    draw_global_icon(display, NAME_ICON_X, MENU_HEADER_Y, BinaryColor::On);
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        GLOBAL_LABEL,
        Point::new(NAME_TEXT_X, MENU_HEADER_Y),
        header_style,
        Baseline::Top,
    )
    .draw(display);

    let subtitle_style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(
        "Global",
        Point::new(NAME_TEXT_X, MENU_SUBTITLE_Y),
        subtitle_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );

    for (index, item) in GLOBAL_MENU_ITEMS.iter().enumerate() {
        draw_menu_item(
            display,
            MENU_ITEM_TOP + index as i32 * MENU_ITEM_STEP,
            item,
            index == selected_item.min(MENU_ITEM_COUNT - 1),
        );
    }
}

fn lora_region_name(region: Region) -> &'static str {
    match region {
        Region::Eu868 => "EU868",
        Region::Us915 => "US915",
        Region::Au915 => "AU915",
        Region::As923 => "AS923",
        Region::Unlimited => "Open",
    }
}

fn lora_row_label(row: LoRaRow) -> &'static str {
    match row {
        LoRaRow::SpreadingFactor => "SF",
        LoRaRow::Bandwidth => "BW",
        LoRaRow::CodingRate => "CR",
        LoRaRow::Frequency => "Freq",
        LoRaRow::TxPower => "Pwr",
        LoRaRow::Region => "Reg",
        LoRaRow::Save => "Save",
    }
}

fn lora_row_value(row: LoRaRow, profile: &RadioProfile) -> heapless::String<12> {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    let mut value = heapless::String::new();
    match row {
        LoRaRow::SpreadingFactor => {
            let _ = write!(value, "{}", spreading_factor as u8);
        }
        LoRaRow::Bandwidth => {
            let _ = write!(value, "{}k", bandwidth.hz() / 1000);
        }
        LoRaRow::CodingRate => {
            let _ = write!(value, "4/{}", coding_rate.denominator());
        }
        LoRaRow::Frequency => {
            let hz = profile.frequency.hz();
            let _ = write!(value, "{}.{}", hz / 1_000_000, (hz % 1_000_000) / 100_000);
        }
        LoRaRow::TxPower => {
            let _ = write!(value, "{}dB", profile.tx_power.dbm());
        }
        LoRaRow::Region => {
            let _ = value.push_str(lora_region_name(profile.region));
        }
        LoRaRow::Save => {}
    }
    value
}

fn lora_row_text(row: LoRaRow, profile: &RadioProfile, editing: bool) -> heapless::String<16> {
    let mut text = heapless::String::new();
    if row.is_save() {
        let _ = text.push_str("Save");
        return text;
    }
    let label = lora_row_label(row);
    let value = lora_row_value(row, profile);
    if editing {
        let _ = write!(text, "{label} [{value}]");
    } else {
        let _ = write!(text, "{label} {value}");
    }
    text
}

const LORA_EDITOR_TOP: i32 = CARD_TOP + 2;
const LORA_DOT_X: i32 = 1;
const LORA_DOT_SIZE: u32 = 2;
const LORA_ROW_TEXT_X: i32 = 6;
const LORA_ROW_BACKING_H: u32 = 10;

fn push_freq_digit(text: &mut heapless::String<16>, digit: u32, active: bool) {
    if active {
        let _ = write!(text, "[{digit}]");
    } else {
        let _ = write!(text, "{digit}");
    }
}

fn lora_freq_edit_text(hz: u32, place: FreqPlace) -> heapless::String<16> {
    let mut text = heapless::String::new();
    push_freq_digit(
        &mut text,
        freq_hundreds_digit(hz),
        matches!(place, FreqPlace::Hundreds),
    );
    push_freq_digit(
        &mut text,
        (hz / 10_000_000) % 10,
        matches!(place, FreqPlace::Tens),
    );
    push_freq_digit(
        &mut text,
        (hz / 1_000_000) % 10,
        matches!(place, FreqPlace::Ones),
    );
    let _ = text.push('.');
    push_freq_digit(
        &mut text,
        (hz / 100_000) % 10,
        matches!(place, FreqPlace::Tenths),
    );
    text
}

fn draw_lora_editor<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cursor: LoRaRow,
    edit: EditMode,
    profile: &RadioProfile,
) {
    for (index, &row) in LORA_ROWS.iter().enumerate() {
        let y = LORA_EDITOR_TOP + index as i32 * MENU_ITEM_STEP;
        let selected = row == cursor;
        let text = match edit {
            EditMode::Freq { place } if selected && matches!(row, LoRaRow::Frequency) => {
                lora_freq_edit_text(profile.frequency.hz(), place)
            }
            EditMode::Field if selected => lora_row_text(row, profile, true),
            _ => lora_row_text(row, profile, false),
        };
        let color = if selected {
            let width =
                (LORA_ROW_TEXT_X + text.chars().count() as i32 * FONT_5X8_CHAR_W + 1).max(0) as u32;
            let _ = Rectangle::new(Point::new(0, y - 1), Size::new(width, LORA_ROW_BACKING_H))
                .into_styled(fill(BinaryColor::On))
                .draw(display);
            BinaryColor::Off
        } else {
            BinaryColor::On
        };
        let _ = Rectangle::new(
            Point::new(LORA_DOT_X, y + 3),
            Size::new(LORA_DOT_SIZE, LORA_DOT_SIZE),
        )
        .into_styled(fill(color))
        .draw(display);
        let style = MonoTextStyle::new(&FONT_5X8, color);
        let _ = Text::with_baseline(&text, Point::new(LORA_ROW_TEXT_X, y), style, Baseline::Top)
            .draw(display);
    }
}

fn draw_interface_menu<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    card: &Card,
    selected_item: usize,
) {
    draw_interface_icon(
        display,
        NAME_ICON_X,
        MENU_HEADER_Y,
        card.kind,
        BinaryColor::On,
    );
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        &card.label,
        Point::new(NAME_TEXT_X, MENU_HEADER_Y),
        header_style,
        Baseline::Top,
    )
    .draw(display);

    let subtitle_style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(
        "Menu",
        Point::new(NAME_TEXT_X, MENU_SUBTITLE_Y),
        subtitle_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );

    for (index, item) in interface_menu_items(card.kind).iter().enumerate() {
        let label = if index == POWER_MENU_ITEM {
            if card.liveness == Liveness::Disabled {
                "Turn On"
            } else {
                "Turn Off"
            }
        } else {
            item
        };
        draw_menu_item(
            display,
            MENU_ITEM_TOP + index as i32 * MENU_ITEM_STEP,
            label,
            index == selected_item.min(MENU_ITEM_COUNT - 1),
        );
    }
}

/// Render the full screen: title bar + a card per interface (up to what fits).
/// Clears first; the caller flushes.
pub fn draw<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, battery);
    draw_global_row(display, GLOBAL_ROW_TOP, false);
    for (i, card) in cards.iter().enumerate() {
        let top = FIRST_CARD_WITH_GLOBAL_TOP + i as i32 * CARD_SLOT_STEP;
        if top >= HEIGHT {
            break;
        }
        if top + CARD_H <= HEIGHT {
            draw_card(display, top, card);
        } else {
            draw_card_peek(display, top, card, card.selected);
        }
    }
}

/// Render the full screen using [`UiState`] for selection and pagination.
///
/// This is the path for real interaction: [`UiState`] controls which card's
/// name row is selected and which window of cards is visible. Plain [`draw`]
/// remains available for static/manual selected-card rendering.
pub fn draw_with_state<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, battery);

    if let UiMode::LoRaEditor {
        cursor,
        edit,
        profile,
    } = state.mode
    {
        draw_lora_editor(display, cursor, edit, &profile);
        return;
    }

    if let Some(selected_item) = state.global_menu_selected_item() {
        draw_global_menu(display, selected_item);
        return;
    }

    if let Some(selected_item) = state.interface_menu_selected_item() {
        if let Some(selected_card) = state.selected_card(cards.len()) {
            draw_interface_menu(display, &cards[selected_card], selected_item);
            return;
        }
    }

    let selected = state.selected_card(cards.len());
    let start = state.visible_start(cards.len());
    let mut top = CARD_TOP;
    let mut focus_index = start;
    if start == 0 {
        draw_global_row(display, GLOBAL_ROW_TOP, state.global_selected());
        top = FIRST_CARD_WITH_GLOBAL_TOP;
        focus_index = 1;
    }
    while top < HEIGHT && focus_index < focus_item_count(cards.len()) {
        let card_index = focus_index - 1;
        let selected_card = selected == Some(card_index);
        if top + CARD_H <= HEIGHT {
            draw_card_with_selection(display, top, &cards[card_index], selected_card);
        } else {
            draw_card_peek(display, top, &cards[card_index], selected_card);
        }
        top += CARD_SLOT_STEP;
        focus_index += 1;
    }
}

/// A boot/connecting splash: title bar + a centered status line.
pub fn splash<D: DrawTarget<Color = BinaryColor>>(display: &mut D, status: &str) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, BatteryState::Unknown);
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(status, Point::new(2, CARD_TOP + 4), style, Baseline::Top)
        .draw(display);
}

#[cfg(test)]
mod tests {
    use embedded_graphics::mock_display::MockDisplay;

    use super::*;

    fn test_card(label: &'static str) -> Card {
        Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Usb,
            label: card_label(label),
            selected: false,
            liveness: Liveness::Live,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        }
    }

    #[test]
    fn short_press_cycles_global_then_cards_and_pages_visible_window() {
        let mut state = UiState::new();
        state.sync_card_count(5);

        assert!(state.global_selected());
        assert_eq!(state.selected_card(5), None);
        assert_eq!(state.visible_start(5), 0);

        state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
        assert_eq!(state.selected_card(5), Some(0));
        assert_eq!(state.visible_start(5), 0);

        state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
        assert_eq!(state.selected_card(5), Some(1));
        assert_eq!(state.visible_start(5), 0);

        state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
        assert_eq!(state.selected_card(5), Some(2));
        assert_eq!(state.visible_start(5), 2);

        state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
        assert_eq!(state.selected_card(5), Some(3));
        assert_eq!(state.visible_start(5), 3);

        state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
        assert_eq!(state.selected_card(5), Some(4));
        assert_eq!(state.visible_start(5), 4);

        state.handle_input(InputEvent::ShortPress, 5, Some(CardKind::Usb));
        assert!(state.global_selected());
        assert_eq!(state.selected_card(5), None);
        assert_eq!(state.visible_start(5), 0);
    }

    #[test]
    fn long_press_opens_global_menu_and_short_press_cycles_menu_items() {
        let mut state = UiState::new();

        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));

        assert_eq!(state.selected_card(4), None);
        assert_eq!(state.visible_start(4), 0);
        assert_eq!(state.global_menu_selected_item(), Some(0));
        assert_eq!(state.menu_selected_item(), Some(0));

        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

        assert_eq!(state.selected_card(4), None);
        assert_eq!(state.global_menu_selected_item(), Some(1));
        assert_eq!(state.menu_selected_item(), Some(1));

        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));

        assert!(state.global_selected());
        assert_eq!(state.menu_selected_item(), None);
    }

    #[test]
    fn long_press_on_the_announce_item_returns_the_announce_action() {
        let mut state = UiState::new();

        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::None
        );
        assert_eq!(state.global_menu_selected_item(), Some(ANNOUNCE_MENU_ITEM));

        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::Announce,
        );
        assert_eq!(state.menu_selected_item(), None);
        assert!(state.global_selected());
    }

    #[test]
    fn long_press_on_any_other_menu_item_just_closes_the_menu() {
        let mut state = UiState::new();
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::None
        );
        assert_eq!(state.menu_selected_item(), None);
    }

    #[test]
    fn long_press_opens_interface_menu_after_card_focus() {
        let mut state = UiState::new();
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));

        assert_eq!(state.selected_card(4), Some(0));
        assert_eq!(state.visible_start(4), 0);
        assert_eq!(state.interface_menu_selected_item(), Some(0));

        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

        assert_eq!(state.selected_card(4), Some(0));
        assert_eq!(state.interface_menu_selected_item(), Some(1));

        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));

        assert_eq!(state.selected_card(4), Some(0));
        assert_eq!(state.menu_selected_item(), None);
    }

    #[test]
    fn lora_editor_taps_to_a_field_grabs_it_steps_the_value_and_saves() {
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);

        state.handle_input(InputEvent::ShortPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::ShortPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);

        for _ in 0..5 {
            state.handle_input(InputEvent::ShortPress, 1, None);
        }
        let saved = state.handle_input(InputEvent::LongPress, 1, None);

        let expected = step_lora_row(DEFAULT_915_PROFILE, LoRaRow::Bandwidth);
        assert_eq!(saved, UiAction::SetLoRaProfile(expected));
        assert_ne!(expected, DEFAULT_915_PROFILE);
    }

    fn freq_editor_at_first_place(profile: RadioProfile) -> UiState {
        let mut state = UiState::new();
        state.open_lora_editor(profile);
        for _ in 0..3 {
            state.handle_input(InputEvent::ShortPress, 1, None);
        }
        state.handle_input(InputEvent::LongPress, 1, None);
        state
    }

    fn save_and_take_freq(state: &mut UiState) -> u32 {
        for _ in 0..3 {
            state.handle_input(InputEvent::ShortPress, 1, None);
        }
        match state.handle_input(InputEvent::LongPress, 1, None) {
            UiAction::SetLoRaProfile(profile) => profile.frequency.hz(),
            other => panic!("expected SetLoRaProfile, got {other:?}"),
        }
    }

    #[test]
    fn lora_editor_dials_frequency_by_place_then_saves() {
        let mut state = freq_editor_at_first_place(DEFAULT_915_PROFILE);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::ShortPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::ShortPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        for _ in 0..3 {
            state.handle_input(InputEvent::ShortPress, 1, None);
        }
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(save_and_take_freq(&mut state), 926_300_000);
    }

    #[test]
    fn lora_editor_clamps_an_out_of_band_frequency_to_the_band_edge() {
        let mut state = freq_editor_at_first_place(DEFAULT_915_PROFILE);
        state.handle_input(InputEvent::LongPress, 1, None);
        for _ in 0..2 {
            state.handle_input(InputEvent::ShortPress, 1, None);
        }
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(save_and_take_freq(&mut state), 928_000_000);
    }

    #[test]
    fn lora_editor_leading_digit_switches_bands_and_clamps_to_the_eu_edge() {
        let mut state = freq_editor_at_first_place(DEFAULT_915_PROFILE);
        state.handle_input(InputEvent::ShortPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(save_and_take_freq(&mut state), 863_000_000);
    }

    #[test]
    fn draw_with_state_marks_selected_card_below_global_row() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        let cards = [test_card("A"), test_card("B")];
        let mut state = UiState::new();
        state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));

        draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

        let selected_top = FIRST_CARD_WITH_GLOBAL_TOP;
        assert_eq!(state.selected_card(cards.len()), Some(0));
        assert_eq!(state.visible_start(cards.len()), 0);
        assert_eq!(
            display.get_pixel(Point::new(NAME_BACKING_X, selected_top + NAME_BACKING_Y)),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(0, selected_top)),
            Some(BinaryColor::On)
        );
        assert_ne!(
            display.get_pixel(Point::new(
                GLOBAL_BACKING_X,
                GLOBAL_ROW_TOP + GLOBAL_BACKING_Y
            )),
            Some(BinaryColor::On)
        );
    }

    #[test]
    fn draw_with_state_renders_selected_global_row() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        let cards = [test_card("USB")];
        let state = UiState::new();

        draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

        assert!(state.global_selected());
        assert_eq!(
            display.get_pixel(Point::new(
                GLOBAL_BACKING_X,
                GLOBAL_ROW_TOP + GLOBAL_BACKING_Y
            )),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(GLOBAL_ICON_X, GLOBAL_ROW_TOP + NAME_LINE_Y)),
            Some(BinaryColor::Off)
        );
        assert_eq!(
            display.get_pixel(Point::new(NAME_ICON_X, GLOBAL_ROW_TOP + NAME_LINE_Y)),
            Some(BinaryColor::Off)
        );
        assert_eq!(
            display.get_pixel(Point::new(GLOBAL_BACKING_X, GLOBAL_ROW_TOP)),
            Some(BinaryColor::Off)
        );
        assert_eq!(
            display.get_pixel(Point::new(
                GLOBAL_BACKING_X,
                GLOBAL_ROW_TOP + GLOBAL_BACKING_Y + GLOBAL_BACKING_H as i32
            )),
            Some(BinaryColor::Off)
        );
        assert_eq!(
            display.get_pixel(Point::new(0, GLOBAL_ROW_TOP + GLOBAL_ROW_H - 1)),
            Some(BinaryColor::Off)
        );
        assert_eq!(
            display.get_pixel(Point::new(0, FIRST_CARD_WITH_GLOBAL_TOP)),
            Some(BinaryColor::On)
        );
    }

    #[test]
    fn draw_with_state_scrolls_global_row_out_of_card_window() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        let cards = [test_card("A"), test_card("B"), test_card("C")];
        let mut state = UiState::new();
        state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));

        draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

        assert_eq!(state.selected_card(cards.len()), Some(2));
        assert_eq!(state.visible_start(cards.len()), 2);
        assert_eq!(
            display.get_pixel(Point::new(0, CARD_TOP)),
            Some(BinaryColor::On)
        );
        assert_ne!(
            display.get_pixel(Point::new(NAME_BACKING_X, CARD_TOP + NAME_BACKING_Y)),
            Some(BinaryColor::On)
        );
    }

    #[test]
    fn draw_with_state_renders_global_menu() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        let cards = [test_card("USB")];
        let mut state = UiState::new();
        state.handle_input(InputEvent::LongPress, cards.len(), Some(CardKind::Usb));

        draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

        assert_eq!(state.global_menu_selected_item(), Some(0));
        assert_eq!(
            display.get_pixel(Point::new(NAME_ICON_X, MENU_HEADER_Y)),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(MENU_BACKING_X, MENU_ITEM_TOP - 1)),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(MENU_MARK_X, MENU_ITEM_TOP + 2)),
            Some(BinaryColor::Off)
        );
        assert_eq!(
            display.get_pixel(Point::new(0, MENU_DIVIDER_Y)),
            Some(BinaryColor::On)
        );
    }

    #[test]
    fn draw_with_state_renders_selected_interface_menu() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        let cards = [
            test_card("USB"),
            Card {
                id: InterfaceId::new([0; 8]),
                kind: CardKind::Ble,
                label: card_label("BLE"),
                selected: false,
                liveness: Liveness::Live,
                tx_bytes: 0,
                rx_bytes: 0,
                links: 0,
                destinations: 0,
                rate_bytes_per_sec: 0,
                last_activity_secs: None,
            },
        ];
        let mut state = UiState::new();
        state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, cards.len(), Some(CardKind::Usb));
        state.handle_input(InputEvent::LongPress, cards.len(), Some(CardKind::Usb));

        draw_with_state(&mut display, &cards, BatteryState::Unknown, &state);

        assert_eq!(state.selected_card(cards.len()), Some(1));
        assert_eq!(state.interface_menu_selected_item(), Some(0));
        assert_eq!(
            display.get_pixel(Point::new(NAME_ICON_X + 4, MENU_HEADER_Y)),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(MENU_BACKING_X, MENU_ITEM_TOP - 1)),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(MENU_MARK_X, MENU_ITEM_TOP + 2)),
            Some(BinaryColor::Off)
        );
        assert_eq!(
            display.get_pixel(Point::new(0, MENU_DIVIDER_Y)),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(0, CARD_TOP)),
            Some(BinaryColor::Off)
        );
    }

    #[test]
    fn count_formatter_uses_blank_base_then_metric_suffixes() {
        assert_eq!(fmt_count(0).as_str(), "0");
        assert_eq!(fmt_count(999).as_str(), "999");
        assert_eq!(fmt_count(1_000).as_str(), "1.0K");
        assert_eq!(fmt_count(12_345).as_str(), "12K");
        assert_eq!(fmt_count(999_999).as_str(), "999K");
        assert_eq!(fmt_count(1_000_000).as_str(), "1.0M");
        assert_eq!(fmt_count(1_234_567_890).as_str(), "1.2B");
    }

    #[test]
    fn live_stat_formatters_stay_compact() {
        assert_eq!(fmt_rate_bytes_per_sec(0).as_str(), "0/s");
        assert_eq!(fmt_rate_bytes_per_sec(999).as_str(), "999/s");
        assert_eq!(fmt_rate_bytes_per_sec(1_200).as_str(), "1.2K/s");
        assert_eq!(fmt_rate_bytes_per_sec(12_000).as_str(), "12K/s");
        assert_eq!(fmt_rate_bytes_per_sec(999_999).as_str(), "999Ks");
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
            "#   # #  ",
            " #  #  # ",
            "  ####   ",
            " #  #  # ",
            "#   # #  ",
            "    ##   ",
            "    #    ",
        ]);
    }

    #[test]
    fn unknown_battery_dash_is_symmetric() {
        let mut display = MockDisplay::new();

        draw_battery(&mut display, 2, 0, BatteryState::Unknown);

        assert_eq!(display.get_pixel(Point::new(5, 4)), None);
        for x in 6..=12 {
            assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
        }
        assert_eq!(display.get_pixel(Point::new(13, 4)), None);
    }

    #[test]
    fn charging_battery_draws_right_side_plug() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);

        draw_battery(&mut display, 2, 0, BatteryState::Charging(100));

        for x in 17..=20 {
            assert_eq!(display.get_pixel(Point::new(x, 4)), Some(BinaryColor::Off));
        }
        assert_eq!(display.get_pixel(Point::new(21, 3)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(23, 4)), None);
    }

    #[test]
    fn person_icon_reads_as_peer_count_glyph() {
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
    fn lightning_icon_reads_as_rate_glyph() {
        let mut display = MockDisplay::new();

        draw_lightning(&mut display, 0, 0);

        display.assert_pattern(&["   # ", "  #  ", " ####", "  #  ", " #   ", "#    "]);
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

    #[test]
    fn card_stacks_traffic_and_moves_peers_right() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let card = Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Usb,
            label: card_label("USB"),
            selected: false,
            liveness: Liveness::Live,
            tx_bytes: 123,
            rx_bytes: 456,
            links: 5,
            destinations: 7,
            rate_bytes_per_sec: 12_345,
            last_activity_secs: Some(3),
        };

        draw_card(&mut display, 0, &card);

        assert_eq!(display.get_pixel(Point::new(4, 14)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(4, 20)), None);
        assert_eq!(display.get_pixel(Point::new(4, 22)), None);
        assert_eq!(display.get_pixel(Point::new(4, 23)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(4, 28)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(4, 29)), None);
        assert_eq!(display.get_pixel(Point::new(33, 14)), None);
        assert_eq!(display.get_pixel(Point::new(37, 14)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(35, 14)), None);
        assert_eq!(display.get_pixel(Point::new(42, 14)), None);
        assert_eq!(display.get_pixel(Point::new(35, 23)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(37, 23)), None);
        assert_eq!(display.get_pixel(Point::new(5, 32)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(38, 32)), Some(BinaryColor::On));
    }

    #[test]
    fn large_link_and_peer_counts_fit_right_column() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let card = Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Wifi,
            label: card_label("WiFi"),
            selected: false,
            liveness: Liveness::Live,
            tx_bytes: 999_999_999,
            rx_bytes: 999_999_999,
            links: 999_999,
            destinations: 1_234_567_890,
            rate_bytes_per_sec: 999_999_999,
            last_activity_secs: Some(3599),
        };

        draw_card(&mut display, 0, &card);

        assert_eq!(compact_numeric_width("999K"), 20);
        assert_eq!(compact_numeric_width("1.2B"), 17);
        assert!(STAT_TEXT_X + compact_numeric_width("999K") < WIDTH);
        assert!(8 + compact_numeric_width("99M/s") < STAT_ICON_X);
        assert!(8 + compact_numeric_width("999Ms") < STAT_ICON_X);
        assert!(ACTIVITY_TEXT_X + compact_numeric_width("-") < WIDTH);
    }

    #[test]
    fn offline_card_centers_status_and_hides_metrics() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let card = Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::EspNow,
            label: card_label("ESP-NOW"),
            selected: false,
            liveness: Liveness::Offline,
            tx_bytes: 123,
            rx_bytes: 456,
            links: 5,
            destinations: 7,
            rate_bytes_per_sec: 123,
            last_activity_secs: Some(12),
        };

        draw_card(&mut display, 0, &card);

        assert_eq!(display.get_pixel(Point::new(18, 21)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(3, 11)), None);
        assert_eq!(display.get_pixel(Point::new(4, 10)), None);
        assert_eq!(display.get_pixel(Point::new(5, 9)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(3, 4)), None);
        assert_eq!(display.get_pixel(Point::new(4, 14)), None);
        assert_eq!(display.get_pixel(Point::new(44, 14)), None);
        assert_eq!(display.get_pixel(Point::new(45, 23)), None);
        assert_eq!(display.get_pixel(Point::new(5, 32)), None);
        assert_eq!(display.get_pixel(Point::new(36, 32)), None);
    }

    #[test]
    fn selected_card_inverts_name_content() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let card = Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Wifi,
            label: card_label("WiFi"),
            selected: true,
            liveness: Liveness::Live,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        };

        draw_card(&mut display, 0, &card);

        assert_eq!(display.get_pixel(Point::new(0, 0)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(63, 0)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(0, 11)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(63, 11)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(1, 1)), None);
        assert_eq!(display.get_pixel(Point::new(2, 1)), None);
        assert_eq!(display.get_pixel(Point::new(45, 1)), None);
        assert_eq!(display.get_pixel(Point::new(0, 12)), Some(BinaryColor::On));
        assert_eq!(
            display.get_pixel(Point::new(0, CARD_H - 1)),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(63, CARD_H - 1)),
            Some(BinaryColor::On)
        );
        assert_eq!(
            display.get_pixel(Point::new(31, CARD_H - 1)),
            Some(BinaryColor::On)
        );
        assert_eq!(display.get_pixel(Point::new(2, 2)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(2, 10)), Some(BinaryColor::On));
        assert_eq!(display.get_pixel(Point::new(2, 11)), None);
        assert_eq!(display.get_pixel(Point::new(5, 2)), Some(BinaryColor::Off));
    }
}
