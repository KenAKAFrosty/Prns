//! The "Personal Hopspot" status screen: portrait 64x128, drawn against any `embedded_graphics`
//! `DrawTarget<Color = BinaryColor>`, so the same pixels land on the S3's SSD1306 OLED and on
//! the desktop simulator window.
//!
//! A two-line inverted title bar over a global menu row and a vertical stack of interface cards:
//! a name line (icon + label), stacked up/down traffic, link and tracked-destination counts,
//! live throughput, last-activity age. The glyphs are drawn primitives, not font characters; the
//! icon mapping is one `match`, the single place to enrich. [`UiState`] keeps the selected focus
//! item visible, paging the stack once more interfaces exist than fit, and a long press opens
//! the global or selected interface's menu.

use core::fmt::Write as _;

use embedded_graphics::mono_font::ascii::{FONT_4X6, FONT_5X8, FONT_6X10, FONT_9X15_BOLD};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use heapless::{String as HString, Vec as HVec};
use personal_rns::interfaces::lora::core::{
    Frequency, ModemPreset, Modulation, RadioProfile, Region, TxPower, DEFAULT_915_PROFILE,
};
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::routing::links::channel::{channel_mdu, ChannelWindow};
use personal_rns::routing::links::data::link_mdu;
use personal_rns::routing::links::resources::max_part_count;
use personal_rns::routing::links::resources::{
    MAX_RETRIES, RATE_FAST_BYTES_PER_SECOND, WINDOW, WINDOW_MAX,
};
use personal_rns::routing::links::MAX_LINK_MTU;
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity};

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
const FOOTER_FIRST_LINE_H: i32 = 10;
const FOOTER_SECOND_LINE_OFFSET: i32 = FOOTER_FIRST_LINE_H + 1;
const FOOTER_SECOND_LINE_H: i32 = 8;
const FOOTER_SECTION_GAP: i32 = 8;
const FOOTER_THIRD_LINE_OFFSET: i32 =
    FOOTER_SECOND_LINE_OFFSET + FOOTER_SECOND_LINE_H + FOOTER_SECTION_GAP;
const FOOTER_FOURTH_LINE_OFFSET: i32 = FOOTER_THIRD_LINE_OFFSET + FOOTER_FIRST_LINE_H + 1;
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
const MENU_HEADER_Y: i32 = CARD_TOP + 2;
const MENU_SUBTITLE_Y: i32 = CARD_TOP + 13;
const MENU_DIVIDER_Y: i32 = CARD_TOP + 23;
const MENU_ITEM_TOP: i32 = CARD_TOP + 29;
const MENU_ITEM_STEP: i32 = 13;
const MENU_BACKING_X: i32 = 2;
const MENU_BACKING_H: u32 = 10;
const MENU_MARK_X: i32 = 4;
const MENU_TEXT_X: i32 = 12;
const MENU_REASON_X: i32 = 2;
const MENU_DETAIL_STEP: i32 = 7;
const FONT_5X8_CHAR_W: i32 = 5;
const FONT_4X6_CHAR_W: i32 = 4;
const LIMITS_PER_PAGE: usize = 6;

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

const GLOBAL_MENU_ITEMS: &[&str] = &["Announce", "Limits", "Sleep", "Back"];
const GLOBAL_MENU_ITEMS_DISPLAY: &[&str] = &["Announce", "Limits", "OLED Off", "Sleep", "Back"];
const GLOBAL_MENU_ITEMS_AP: &[&str] = &["Announce", "Limits", "Sleep", "AP Mode", "Back"];
const GLOBAL_MENU_ITEMS_AP_DISPLAY: &[&str] =
    &["Announce", "Limits", "OLED Off", "Sleep", "AP Mode", "Back"];
const ANNOUNCE_MENU_ITEM: usize = 0;
const LIMITS_MENU_ITEM: usize = 1;
const OLED_OFF_MENU_ITEM: usize = 2;
const SLEEP_MENU_ITEM: usize = 3;
const RADIO_MENU_ITEM: usize = 4;
const SLEEP_MENU_ITEM_NO_DISPLAY: usize = 2;
const RADIO_MENU_ITEM_NO_DISPLAY: usize = 3;
const GLOBAL_MENU_ITEM_STEP: i32 = 11;
const BATTERY_CHARGE_BLINK_MS: u64 = 600;
/// Item 0 of every interface menu is the power toggle; its label is rendered live ("Turn Off" /
/// "Turn On") from the card's [`Liveness`], and long-pressing it emits [`UiAction::ToggleSelectedInterface`].
const POWER_MENU_ITEM: usize = 0;
const POWER_ONLY_MENU_ITEMS: &[&str] = &["Power", "Back"];
const LORA_MENU_ITEMS: &[&str] = &["Power", "Tune", "Reset", "Back"];
const LORA_TUNE_MENU_ITEM: usize = 1;
const LORA_RESET_MENU_ITEM: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LimitValue {
    Count(u32),
    Bytes(u64),
    Range(u32, u32),
    RateBytesPerSec(u64),
    Text(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LimitRow {
    label: &'static str,
    value: LimitValue,
}

impl LimitRow {
    const fn count(label: &'static str, value: u32) -> Self {
        Self {
            label,
            value: LimitValue::Count(value),
        }
    }

    const fn bytes(label: &'static str, value: u64) -> Self {
        Self {
            label,
            value: LimitValue::Bytes(value),
        }
    }

    const fn range(label: &'static str, low: u32, high: u32) -> Self {
        Self {
            label,
            value: LimitValue::Range(low, high),
        }
    }

    const fn rate(label: &'static str, value: u64) -> Self {
        Self {
            label,
            value: LimitValue::RateBytesPerSec(value),
        }
    }

    const fn text(label: &'static str, value: &'static str) -> Self {
        Self {
            label,
            value: LimitValue::Text(value),
        }
    }
}

const LIMIT_ROW_CAPACITY: usize = 24;

fn limit_count(value: usize) -> u32 {
    value.min(u32::MAX as usize) as u32
}

fn capacity_row(label: &'static str, capacity: StorageCapacity) -> LimitRow {
    match capacity {
        StorageCapacity::Fixed(value) => LimitRow::count(label, limit_count(value)),
        StorageCapacity::Dynamic => LimitRow::text(label, "dyn"),
    }
}

fn push_limit_row(rows: &mut HVec<LimitRow, LIMIT_ROW_CAPACITY>, row: LimitRow) {
    let _ = rows.push(row);
}

fn build_limit_rows(limits: DisplayedStorageLimits) -> HVec<LimitRow, LIMIT_ROW_CAPACITY> {
    let mut rows = HVec::new();
    push_limit_row(&mut rows, capacity_row("Dst", limits.tracked_destinations));
    push_limit_row(&mut rows, capacity_row("Ann", limits.announce_records));
    push_limit_row(
        &mut rows,
        capacity_row("AppDst", limits.upstream_app_destinations),
    );
    push_limit_row(&mut rows, capacity_row("Links", limits.links));
    push_limit_row(&mut rows, capacity_row("Chans", limits.channels));
    if let Some(pool) = limits.channel_window_pool {
        push_limit_row(&mut rows, LimitRow::count("ChPool", limit_count(pool)));
    } else {
        push_limit_row(
            &mut rows,
            capacity_row("Reorder", limits.channel_reorder_depth),
        );
    }
    match limits.link_mtu {
        StorageCapacity::Fixed(mtu) => {
            push_limit_row(&mut rows, LimitRow::bytes("MTU", mtu as u64));
            push_limit_row(&mut rows, LimitRow::bytes("LinkMDU", link_mdu(mtu) as u64));
            push_limit_row(
                &mut rows,
                LimitRow::bytes("ChanMDU", channel_mdu(mtu) as u64),
            );
        }
        StorageCapacity::Dynamic => {
            push_limit_row(&mut rows, LimitRow::bytes("MaxMTU", MAX_LINK_MTU as u64));
        }
    }
    match limits.resource_transfer_bytes {
        StorageCapacity::Fixed(bytes) => {
            push_limit_row(&mut rows, LimitRow::bytes("ResBuf", bytes as u64));
            push_limit_row(
                &mut rows,
                LimitRow::count("ResPart", limit_count(max_part_count(bytes))),
            );
        }
        StorageCapacity::Dynamic => push_limit_row(&mut rows, LimitRow::text("ResBuf", "dyn")),
    }
    push_limit_row(
        &mut rows,
        LimitRow::range("ResWin", WINDOW as u32, WINDOW_MAX as u32),
    );
    push_limit_row(&mut rows, LimitRow::count("Retry", MAX_RETRIES as u32));
    push_limit_row(
        &mut rows,
        LimitRow::rate("Fast", RATE_FAST_BYTES_PER_SECOND),
    );
    push_limit_row(&mut rows, capacity_row("Receipts", limits.receipts));
    push_limit_row(&mut rows, capacity_row("PktHash", limits.packet_hashes));
    push_limit_row(&mut rows, capacity_row("RevRte", limits.reverse_routes));
    push_limit_row(
        &mut rows,
        capacity_row("PathReq", limits.pending_path_requests),
    );
    push_limit_row(&mut rows, capacity_row("HeldAnn", limits.held_announces));
    push_limit_row(&mut rows, capacity_row("HeldID", limits.held_identities));
    push_limit_row(
        &mut rows,
        capacity_row("Ratchet", limits.ratchets_per_destination),
    );
    push_limit_row(
        &mut rows,
        LimitRow::range(
            "ChanWin",
            ChannelWindow::MIN as u32,
            ChannelWindow::MAX_FAST as u32,
        ),
    );
    rows
}

fn storage_limit_page_count(limits: DisplayedStorageLimits) -> usize {
    let rows = build_limit_rows(limits);
    limit_page_count(&rows)
}

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

/// How alive an interface's card reads. `Live` is a confirmed link: the full card with numbers.
/// `Dormant` is up and watching with no confirmed link yet (the USB discoverer with nothing
/// plugged): the *live* icon over a "Dormant" body, so a card never pretends to carry traffic it
/// has none of. `Failed` is a genuinely failed interface: the offline icon and a "Failed" body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Liveness {
    Failed,
    Dormant,
    Live,
    /// Deliberately turned off from the UI: keeps its own interface icon (not the failure slash)
    /// over an "Off" body, so an interface a user switched off never reads as one that broke.
    Disabled,
}

impl Liveness {
    fn is_failed(self) -> bool {
        matches!(self, Liveness::Failed)
    }
}

#[must_use]
pub const fn liveness_from_connection(connection: ConnectionState) -> Liveness {
    match connection {
        ConnectionState::Connected | ConnectionState::Degraded => Liveness::Live,
        ConnectionState::Failed | ConnectionState::Unknown => Liveness::Failed,
        ConnectionState::Disabled => Liveness::Disabled,
        ConnectionState::Initializing
        | ConnectionState::Reconnecting
        | ConnectionState::Disconnected => Liveness::Dormant,
    }
}

/// The card label's backing buffer: owned, not `&'static str`, so a face can format a runtime
/// tag into it (a discovered peer's id). Truncated to the cap; the panel clips past its width.
pub const CARD_LABEL_CAP: usize = 16;
pub type CardLabel = heapless::String<CARD_LABEL_CAP>;

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

pub const INTERFACE_MENU_DETAIL_TEXT_CAP: usize = 16;
pub const INTERFACE_MENU_DETAIL_ROWS_CAP: usize = 8;
pub type InterfaceMenuDetailText = HString<INTERFACE_MENU_DETAIL_TEXT_CAP>;
pub type InterfaceMenuDetailRows = HVec<InterfaceMenuDetailRow, INTERFACE_MENU_DETAIL_ROWS_CAP>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InterfaceMenuDetailKind {
    Info,
    Peer,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InterfaceMenuDetailRow {
    text: InterfaceMenuDetailText,
    kind: InterfaceMenuDetailKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SupervisorPeerMenuStatus {
    pub id: InterfaceId,
    pub liveness: Liveness,
}

impl InterfaceMenuDetailRow {
    #[must_use]
    pub fn text(&self) -> &str {
        self.text.as_str()
    }

    #[must_use]
    pub const fn kind(&self) -> InterfaceMenuDetailKind {
        self.kind
    }

    #[must_use]
    pub fn from_text(kind: InterfaceMenuDetailKind, text: &str) -> Self {
        let mut row = Self {
            text: InterfaceMenuDetailText::new(),
            kind,
        };
        push_truncated(&mut row.text, text);
        row
    }

    #[must_use]
    pub fn info(label: &str, value: &str) -> Self {
        let mut row = Self {
            text: InterfaceMenuDetailText::new(),
            kind: InterfaceMenuDetailKind::Info,
        };
        push_truncated(&mut row.text, label);
        let _ = row.text.push(' ');
        push_truncated(&mut row.text, if value.is_empty() { "None" } else { value });
        row
    }
}

pub fn push_interface_menu_info(rows: &mut InterfaceMenuDetailRows, label: &str, value: &str) {
    let _ = rows.push(InterfaceMenuDetailRow::info(label, value));
}

pub fn push_supervisor_peer_rows<I>(rows: &mut InterfaceMenuDetailRows, peers: I) -> usize
where
    I: IntoIterator<Item = SupervisorPeerMenuStatus>,
{
    let count_index = rows.len();
    let _ = rows.push(InterfaceMenuDetailRow::from_text(
        InterfaceMenuDetailKind::Info,
        "Peers 0",
    ));
    let mut count = 0usize;
    for peer in peers {
        count = count.saturating_add(1);
        let mut text = InterfaceMenuDetailText::new();
        let bytes = peer.id.as_bytes();
        let _ = write!(
            text,
            "P {:02x}{:02x} {}",
            bytes[1],
            bytes[2],
            liveness_short_label(peer.liveness)
        );
        let _ = rows.push(InterfaceMenuDetailRow {
            text,
            kind: InterfaceMenuDetailKind::Peer,
        });
    }
    if let Some(row) = rows.get_mut(count_index) {
        row.text.clear();
        let _ = write!(row.text, "Peers {count}");
    }
    count
}

pub fn push_named_peer_row(
    rows: &mut InterfaceMenuDetailRows,
    label: &str,
    liveness: Option<Liveness>,
) -> usize {
    let count = usize::from(liveness.is_some());
    let mut count_text = InterfaceMenuDetailText::new();
    let _ = write!(count_text, "Peers {count}");
    let _ = rows.push(InterfaceMenuDetailRow {
        text: count_text,
        kind: InterfaceMenuDetailKind::Info,
    });
    if let Some(liveness) = liveness {
        let mut text = InterfaceMenuDetailText::new();
        let _ = text.push_str("P ");
        push_truncated(&mut text, label);
        let _ = text.push(' ');
        let _ = text.push_str(liveness_short_label(liveness));
        let _ = rows.push(InterfaceMenuDetailRow {
            text,
            kind: InterfaceMenuDetailKind::Peer,
        });
    }
    count
}

fn push_truncated<const N: usize>(text: &mut HString<N>, value: &str) {
    for c in value.chars() {
        if text.push(c).is_err() {
            break;
        }
    }
}

const fn liveness_short_label(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::Live => "Live",
        Liveness::Dormant => "Dorm",
        Liveness::Disabled => "Off",
        Liveness::Failed => "Fail",
    }
}

/// `TCP ` plus as much of the dial target as fits, so several clients are told apart by where
/// they point (`TCP 162.255.87` vs `TCP schttopup.c`) rather than all reading a bare `TCP`.
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

/// One interface's card: identity from the host, live numbers from the interface's status handle.
pub struct Card {
    /// What a face acts on for the selected card (toggle off/on); no separate index-to-id table.
    pub id: InterfaceId,
    pub kind: CardKind,
    pub label: CardLabel,
    pub selected: bool,
    pub liveness: Liveness,
    pub failure_reason: Option<&'static str>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub links: u32,
    /// Routing-table destinations reachable via this interface.
    pub destinations: u32,
    pub rate_bytes_per_sec: u32,
    pub last_activity_secs: Option<u32>,
}

pub fn sort_cards_for_display<const N: usize>(cards: &mut HVec<Card, N>) {
    cards.sort_unstable_by_key(|card| card_display_rank(card.kind));
}

const fn card_display_rank(kind: CardKind) -> u8 {
    match kind {
        CardKind::LoRa => 0,
        CardKind::Wifi => 1,
        CardKind::Ble => 2,
        CardKind::EspNow => 3,
        CardKind::Tcp => 4,
        CardKind::Peer => 5,
        CardKind::Usb => 6,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct CardActivitySignature {
    liveness: Liveness,
    tx_bytes: u64,
    rx_bytes: u64,
    links: u32,
    destinations: u32,
    rate_bytes_per_sec: u32,
}

impl CardActivitySignature {
    fn of(card: &Card) -> Self {
        Self {
            liveness: card.liveness,
            tx_bytes: card.tx_bytes,
            rx_bytes: card.rx_bytes,
            links: card.links,
            destinations: card.destinations,
            rate_bytes_per_sec: card.rate_bytes_per_sec,
        }
    }

    fn observed_active(self) -> bool {
        self.liveness == Liveness::Live || self.links > 0 || self.rate_bytes_per_sec > 0
    }
}

#[derive(Clone, Copy)]
struct CardActivityEntry {
    id: InterfaceId,
    signature: CardActivitySignature,
    last_activity_at_secs: Option<u32>,
}

/// Tracks the most recent observed activity for a fixed-size card set. The renderer stays
/// stateless and `no_std`: each face owns one tracker, calls [`update`](Self::update) before
/// drawing, and passes a monotonic seconds counter from whatever clock its platform has.
pub struct CardActivityTracker<const N: usize> {
    entries: [Option<CardActivityEntry>; N],
}

impl<const N: usize> CardActivityTracker<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    /// Stamp each card's `last_activity_secs` from changes observed since the previous frame.
    pub fn update(&mut self, cards: &mut [Card], now_secs: u32) {
        for card in cards.iter_mut() {
            let signature = CardActivitySignature::of(card);
            let last_activity_at_secs = match self.entry_mut(card.id) {
                Some(entry) => {
                    if entry.signature != signature {
                        entry.signature = signature;
                        entry.last_activity_at_secs = Some(now_secs);
                    }
                    entry.last_activity_at_secs
                }
                None => {
                    let last_activity_at_secs = signature.observed_active().then_some(now_secs);
                    if let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) {
                        *slot = Some(CardActivityEntry {
                            id: card.id,
                            signature,
                            last_activity_at_secs,
                        });
                    }
                    last_activity_at_secs
                }
            };
            card.last_activity_secs =
                last_activity_at_secs.map(|then| now_secs.saturating_sub(then));
        }
        self.prune(cards);
    }

    fn entry_mut(&mut self, id: InterfaceId) -> Option<&mut CardActivityEntry> {
        self.entries
            .iter_mut()
            .filter_map(Option::as_mut)
            .find(|entry| entry.id == id)
    }

    fn prune(&mut self, cards: &[Card]) {
        for slot in &mut self.entries {
            if slot
                .as_ref()
                .is_some_and(|entry| !cards.iter().any(|card| card.id == entry.id))
            {
                *slot = None;
            }
        }
    }
}

impl<const N: usize> Default for CardActivityTracker<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// What the title-bar battery glyph shows; `Unknown` (a dash) means no plausible battery is
/// detected. Boards without a charge-status signal keep reporting `Level`/`Unknown`.
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
    OledOff,
    Sleep,
    Wake,
    /// Flip the selected card's interface off or back on, keyed by the card's [`id`](Card::id).
    ToggleSelectedInterface,
    OpenLoRaEditor,
    SetLoRaProfile(RadioProfile),
    SwapRadioMode,
    OpenDocs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiNotice {
    Announcing,
    OledOff,
    TurningOff,
    TurningOn,
    Sleeping,
    Awake,
    Saved,
}

impl UiNotice {
    fn label(self) -> &'static str {
        match self {
            Self::Announcing => "Announcing",
            Self::OledOff => "OLED Off",
            Self::TurningOff => "Turning Off",
            Self::TurningOn => "Turning On",
            Self::Sleeping => "Sleeping",
            Self::Awake => "Awake",
            Self::Saved => "Saved",
        }
    }
}

/// A small free-form note drawn below the interface card stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiFooter<'a> {
    line1: &'a str,
    line2: Option<&'a str>,
    line3: Option<&'a str>,
    line4: Option<&'a str>,
}

impl<'a> UiFooter<'a> {
    pub const fn new(line1: &'a str, line2: Option<&'a str>) -> Self {
        Self {
            line1,
            line2,
            line3: None,
            line4: None,
        }
    }

    pub const fn with_lines(
        line1: &'a str,
        line2: Option<&'a str>,
        line3: Option<&'a str>,
        line4: Option<&'a str>,
    ) -> Self {
        Self {
            line1,
            line2,
            line3,
            line4,
        }
    }
}

/// Interaction state for the Hopspot card stack. The renderer stays data-driven: this only
/// records which focus row/card is selected, which slice of the stack is visible on the panel,
/// and whether a menu is open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiState {
    selected_focus: usize,
    visible_start: usize,
    mode: UiMode,
    display_power_capable: bool,
    ap_capable: bool,
    ap_active: bool,
    notice: Option<UiNotice>,
    storage_limits: DisplayedStorageLimits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UiMode {
    Cards,
    GlobalMenu {
        selected_item: usize,
    },
    LimitsPage {
        page: usize,
    },
    Sleeping,
    InterfaceMenu {
        selected_item: usize,
        kind: CardKind,
    },
    LoRaEditor {
        screen: LoRaScreen,
        profile: RadioProfile,
    },
    ConfirmRadioSwap {
        confirm: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoRaScreen {
    Region { cursor: usize },
    Preset { cursor: usize },
    Frequency { cursor: FreqRow, edit: EditMode },
    Custom { cursor: CustomRow, edit: EditMode },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditMode {
    Browsing,
    Field,
    Freq { place: FreqPlace },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PresetChoice {
    Preset(ModemPreset),
    Custom,
    Back,
}

const PRESET_CHOICES: [PresetChoice; 6] = [
    PresetChoice::Preset(ModemPreset::ShortFast),
    PresetChoice::Preset(ModemPreset::MediumFast),
    PresetChoice::Preset(ModemPreset::LongFast),
    PresetChoice::Preset(ModemPreset::LongSlow),
    PresetChoice::Custom,
    PresetChoice::Back,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FreqPlace {
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
enum CustomRow {
    SpreadingFactor,
    Bandwidth,
    CodingRate,
    FreqMhz,
    FreqKhz,
    TxPower,
    Save,
    Back,
}

const CUSTOM_ROWS: [CustomRow; 8] = [
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
enum FreqRow {
    Channel,
    Mhz,
    Khz,
    Save,
    Back,
}

const FREQ_ROWS: [FreqRow; 5] = [
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

const LORA_REGION_CANCEL: usize = Region::ALL.len();
const LORA_REGION_COUNT: usize = Region::ALL.len() + 1;

fn region_index(region: Region) -> usize {
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

fn scroll_start(cursor: usize, count: usize, visible: usize) -> usize {
    if count <= visible || cursor < visible {
        0
    } else {
        (cursor + 1 - visible).min(count - visible)
    }
}

fn step_custom_row(profile: RadioProfile, row: CustomRow) -> RadioProfile {
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

enum LoRaHold {
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

fn channel_count(profile: &RadioProfile) -> u32 {
    let (low, high) = profile.region.band();
    ((high - low) / channel_bandwidth_hz(profile)).max(1)
}

fn channel_center_hz(profile: &RadioProfile, channel: u32) -> u32 {
    let (low, _) = profile.region.band();
    let bandwidth = channel_bandwidth_hz(profile);
    low + bandwidth / 2 + channel * bandwidth
}

fn current_channel(profile: &RadioProfile) -> u32 {
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

fn lora_editor_tap(screen: LoRaScreen, profile: RadioProfile) -> (LoRaScreen, RadioProfile) {
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

fn lora_editor_hold(screen: LoRaScreen, profile: RadioProfile) -> LoRaHold {
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

impl UiState {
    pub const fn new() -> Self {
        Self {
            selected_focus: 0,
            visible_start: 0,
            mode: UiMode::Cards,
            display_power_capable: false,
            ap_capable: false,
            ap_active: false,
            notice: None,
            storage_limits: DisplayedStorageLimits::DYNAMIC,
        }
    }

    pub fn show_notice(&mut self, notice: UiNotice) {
        self.notice = Some(notice);
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    pub fn notice(&self) -> Option<UiNotice> {
        self.notice
    }

    pub fn global_selected(&self) -> bool {
        matches!(self.mode, UiMode::Cards) && self.selected_focus == 0
    }

    pub fn selected_card(&self, card_count: usize) -> Option<usize> {
        let card_index = self.selected_focus.checked_sub(1)?;
        if card_index < card_count {
            Some(card_index)
        } else {
            None
        }
    }

    /// The first visible focus item: `0` the global action row, `1..` cards shifted by one.
    pub fn visible_start(&self, card_count: usize) -> usize {
        self.visible_start_with_footer(card_count, false)
    }

    pub fn visible_start_with_footer(&self, card_count: usize, has_footer: bool) -> usize {
        visible_start_for(
            focus_item_count_with_footer(card_count, has_footer),
            self.selected_focus,
            self.visible_start,
        )
    }

    pub fn menu_selected_item(&self) -> Option<usize> {
        match self.mode {
            UiMode::GlobalMenu { selected_item } | UiMode::InterfaceMenu { selected_item, .. } => {
                Some(selected_item)
            }
            UiMode::Cards
            | UiMode::LimitsPage { .. }
            | UiMode::Sleeping
            | UiMode::LoRaEditor { .. }
            | UiMode::ConfirmRadioSwap { .. } => None,
        }
    }

    pub fn global_menu_selected_item(&self) -> Option<usize> {
        match self.mode {
            UiMode::GlobalMenu { selected_item } => Some(selected_item),
            UiMode::Cards
            | UiMode::LimitsPage { .. }
            | UiMode::Sleeping
            | UiMode::InterfaceMenu { .. }
            | UiMode::LoRaEditor { .. }
            | UiMode::ConfirmRadioSwap { .. } => None,
        }
    }

    pub fn interface_menu_selected_item(&self) -> Option<usize> {
        match self.mode {
            UiMode::InterfaceMenu { selected_item, .. } => Some(selected_item),
            UiMode::Cards
            | UiMode::GlobalMenu { .. }
            | UiMode::LimitsPage { .. }
            | UiMode::Sleeping
            | UiMode::LoRaEditor { .. }
            | UiMode::ConfirmRadioSwap { .. } => None,
        }
    }

    pub fn open_lora_editor(&mut self, profile: RadioProfile) {
        self.mode = UiMode::LoRaEditor {
            screen: LoRaScreen::Region {
                cursor: region_index(profile.region),
            },
            profile,
        };
    }

    pub fn set_radio_state(&mut self, capable: bool, active: bool) {
        self.ap_capable = capable;
        self.ap_active = active;
    }

    pub fn set_display_power_capable(&mut self, capable: bool) {
        self.display_power_capable = capable;
    }

    pub fn set_storage_limits(&mut self, limits: DisplayedStorageLimits) {
        self.storage_limits = limits;
    }

    fn global_menu_items(&self) -> &'static [&'static str] {
        match (self.display_power_capable, self.ap_capable) {
            (true, true) => GLOBAL_MENU_ITEMS_AP_DISPLAY,
            (true, false) => GLOBAL_MENU_ITEMS_DISPLAY,
            (false, true) => GLOBAL_MENU_ITEMS_AP,
            (false, false) => GLOBAL_MENU_ITEMS,
        }
    }

    fn global_radio_menu_item(&self) -> usize {
        if self.display_power_capable {
            RADIO_MENU_ITEM
        } else {
            RADIO_MENU_ITEM_NO_DISPLAY
        }
    }

    fn global_sleep_menu_item(&self) -> usize {
        if self.display_power_capable {
            SLEEP_MENU_ITEM
        } else {
            SLEEP_MENU_ITEM_NO_DISPLAY
        }
    }

    /// Reconcile selection/window state after the runtime's interface list changes.
    pub fn sync_card_count(&mut self, card_count: usize) {
        self.sync_card_count_with_footer(card_count, false);
    }

    /// Reconcile selection/window state when there is a non-card footer after the card list.
    pub fn sync_card_count_with_footer(&mut self, card_count: usize, has_footer: bool) {
        let item_count = focus_item_count_with_footer(card_count, has_footer);
        self.selected_focus = self.selected_focus.min(item_count - 1);
        self.visible_start = visible_start_for(item_count, self.selected_focus, self.visible_start);

        match self.mode {
            UiMode::Cards
            | UiMode::GlobalMenu { .. }
            | UiMode::LimitsPage { .. }
            | UiMode::Sleeping
            | UiMode::LoRaEditor { .. }
            | UiMode::ConfirmRadioSwap { .. } => {}
            UiMode::InterfaceMenu { .. } if self.selected_card(card_count).is_none() => {
                self.mode = UiMode::Cards;
            }
            UiMode::InterfaceMenu {
                selected_item,
                kind,
            } => {
                self.mode = UiMode::InterfaceMenu {
                    selected_item: selected_item.min(interface_menu_items(kind).len() - 1),
                    kind,
                };
            }
        }
        if let UiMode::GlobalMenu { selected_item } = self.mode {
            let count = self.global_menu_items().len();
            self.mode = UiMode::GlobalMenu {
                selected_item: selected_item.min(count - 1),
            };
        }
    }

    /// Apply one single-button event, returning what the app should do about it. `selected_kind`
    /// (read from the card list) lets the interface menu resolve its kind-specific items.
    pub fn handle_input(
        &mut self,
        event: InputEvent,
        card_count: usize,
        selected_kind: Option<CardKind>,
    ) -> UiAction {
        self.handle_input_with_footer(event, card_count, false, selected_kind)
    }

    /// Apply input when a non-card footer sits after the cards in the scroll stack.
    pub fn handle_input_with_footer(
        &mut self,
        event: InputEvent,
        card_count: usize,
        has_footer: bool,
        selected_kind: Option<CardKind>,
    ) -> UiAction {
        self.notice = None;
        self.sync_card_count_with_footer(card_count, has_footer);
        let item_count = focus_item_count_with_footer(card_count, has_footer);
        let action = match (event, self.mode) {
            (InputEvent::ShortPress | InputEvent::LongPress, UiMode::Sleeping) => {
                self.mode = UiMode::Cards;
                UiAction::Wake
            }
            (InputEvent::ShortPress, UiMode::LimitsPage { page }) => {
                self.mode = UiMode::LimitsPage {
                    page: (page + 1) % storage_limit_page_count(self.storage_limits),
                };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::LimitsPage { .. }) => {
                self.mode = UiMode::Cards;
                UiAction::None
            }
            (InputEvent::ShortPress, UiMode::Cards) => {
                self.selected_focus = (self.selected_focus + 1) % item_count;
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::Cards) if self.selected_focus == 0 => {
                self.mode = UiMode::GlobalMenu { selected_item: 0 };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::Cards)
                if has_footer && self.selected_focus == card_count + 1 =>
            {
                UiAction::OpenDocs
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
                let count = self.global_menu_items().len();
                self.mode = UiMode::GlobalMenu {
                    selected_item: (selected_item + 1) % count,
                };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::GlobalMenu { selected_item }) => match selected_item {
                ANNOUNCE_MENU_ITEM => {
                    self.mode = UiMode::Cards;
                    UiAction::Announce
                }
                LIMITS_MENU_ITEM => {
                    self.mode = UiMode::LimitsPage { page: 0 };
                    UiAction::None
                }
                OLED_OFF_MENU_ITEM if self.display_power_capable => {
                    self.mode = UiMode::Cards;
                    UiAction::OledOff
                }
                item if item == self.global_sleep_menu_item() => {
                    self.mode = UiMode::Sleeping;
                    UiAction::Sleep
                }
                item if self.ap_capable && item == self.global_radio_menu_item() => {
                    self.mode = UiMode::ConfirmRadioSwap { confirm: false };
                    UiAction::None
                }
                _ => {
                    self.mode = UiMode::Cards;
                    UiAction::None
                }
            },
            (InputEvent::ShortPress, UiMode::ConfirmRadioSwap { confirm }) => {
                self.mode = UiMode::ConfirmRadioSwap { confirm: !confirm };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::ConfirmRadioSwap { confirm }) => {
                self.mode = UiMode::Cards;
                if confirm {
                    UiAction::SwapRadioMode
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
                    selected_item: (selected_item + 1) % interface_menu_items(kind).len(),
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
            (InputEvent::ShortPress, UiMode::LoRaEditor { screen, profile }) => {
                let (screen, profile) = lora_editor_tap(screen, profile);
                self.mode = UiMode::LoRaEditor { screen, profile };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::LoRaEditor { screen, profile }) => {
                match lora_editor_hold(screen, profile) {
                    LoRaHold::Stay { screen, profile } => {
                        self.mode = UiMode::LoRaEditor { screen, profile };
                        UiAction::None
                    }
                    LoRaHold::Commit(profile) => {
                        self.mode = UiMode::Cards;
                        UiAction::SetLoRaProfile(profile)
                    }
                    LoRaHold::Cancel => {
                        self.mode = UiMode::Cards;
                        UiAction::None
                    }
                }
            }
        };
        self.sync_card_count_with_footer(card_count, has_footer);
        action
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self::new()
    }
}

fn focus_item_count_with_footer(card_count: usize, has_footer: bool) -> usize {
    card_count + 1 + usize::from(has_footer)
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
        let tenths = thousandths / 100;
        let _ = write!(s, "{}.{}{}", tenths / 10, tenths % 10, unit);
    } else {
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
        let _ = write!(s, "{n}B");
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
        let _ = write!(s, "{}.{}{}", tenths / 10, tenths % 10, unit);
    } else {
        let _ = write!(s, "{int_part}{unit}");
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

fn interface_menu_items(kind: CardKind) -> &'static [&'static str] {
    match kind {
        CardKind::LoRa => LORA_MENU_ITEMS,
        CardKind::Wifi
        | CardKind::Peer
        | CardKind::Usb
        | CardKind::Ble
        | CardKind::EspNow
        | CardKind::Tcp => POWER_ONLY_MENU_ITEMS,
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

/// The battery glyph, drawn in the background color (it sits on the inverted title bar): a 15x9
/// outline + left terminal nub, then four filled segment bars to the nearest quarter, an
/// incoming plug cue for charging, or a dash for unknown.
fn draw_battery<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    state: BatteryState,
    charging_tier_visible: bool,
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
        BatteryState::Level(pct) => {
            // Segments fill to the nearest quarter, anchored at the RIGHT so the leftmost bar
            // empties first as the cell drains (matching the panel's orientation).
            let filled = ((pct as u32 * 4 + 50) / 100).min(4);
            for i in (4 - filled)..4 {
                draw_battery_segment(display, x, y, i);
            }
        }
        BatteryState::Charging(pct) if pct >= 100 => {
            draw_full_battery(display, x, y);
        }
        BatteryState::Charging(pct) => {
            let filled = (pct as u32 * 4 / 100).min(3);
            for i in (4 - filled)..4 {
                draw_battery_segment(display, x, y, i);
            }
            if charging_tier_visible {
                draw_battery_segment(display, x, y, 3 - filled);
            }
            draw_charging_plug(display, x, y);
        }
        BatteryState::Unknown => {
            let _ = Line::new(Point::new(x + 4, y + 4), Point::new(x + 10, y + 4))
                .into_styled(outline)
                .draw(display);
        }
    }
}

fn draw_battery_segment<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    x: i32,
    y: i32,
    segment: u32,
) {
    let bar_x = x + 2 + segment as i32 * 3;
    let _ = Rectangle::new(Point::new(bar_x, y + 2), Size::new(2, 5))
        .into_styled(fill(BinaryColor::Off))
        .draw(display);
}

fn draw_full_battery<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    let _ = Rectangle::new(Point::new(x, y), Size::new(15, 9))
        .into_styled(fill(BinaryColor::Off))
        .draw(display);
}

fn battery_charge_tier_visible(animation_ms: u64) -> bool {
    (animation_ms / BATTERY_CHARGE_BLINK_MS).is_multiple_of(2)
}

/// The charging cue: a plug entering the battery's right side from off-screen right.
fn draw_charging_plug<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32) {
    let outline = stroke(BinaryColor::Off);
    let solid = fill(BinaryColor::Off);
    let _ = Rectangle::new(Point::new(x + 15, y + 2), Size::new(4, 5))
        .into_styled(solid)
        .draw(display);
    let _ = Line::new(Point::new(x + 19, y + 3), Point::new(x + 14, y + 3))
        .into_styled(outline)
        .draw(display);
    let _ = Line::new(Point::new(x + 19, y + 5), Point::new(x + 14, y + 5))
        .into_styled(outline)
        .draw(display);
}

/// The two-line inverted title bar: small `Personal` + battery glyph over a big bold `Hopspot`.
fn draw_title_bar<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    battery: BatteryState,
    animation_ms: u64,
) {
    let _ = Rectangle::new(Point::new(0, 0), Size::new(WIDTH as u32, TITLE_H as u32))
        .into_styled(fill(BinaryColor::On))
        .draw(display);
    let small = MonoTextStyle::new(&FONT_5X8, BinaryColor::Off);
    let _ = Text::with_baseline("Personal", Point::new(2, 1), small, Baseline::Top).draw(display);
    // x=45: the 2px nub starts at col 43 and the 15px outline ends at col 59,
    // leaving the right edge (cols 60..63) for the charging plug to enter from.
    draw_battery(
        display,
        44,
        1,
        battery,
        battery_charge_tier_visible(animation_ms),
    );
    let big = MonoTextStyle::new(&FONT_9X15_BOLD, BinaryColor::Off);
    let _ = Text::with_baseline("Hopspot", Point::new(1, 10), big, Baseline::Top).draw(display);
}

/// A thin up or down arrow: a shortened 1px shaft with a chevron head, 5x7, at text row `y`.
fn draw_arrow<D: DrawTarget<Color = BinaryColor>>(display: &mut D, x: i32, y: i32, up: bool) {
    let cx = x + 2;
    // Shaft: down arrows omit the top pixel to open the stacked-row gap.
    let shaft_start = if up { y } else { y + 1 };
    line(display, Point::new(cx, shaft_start), Point::new(cx, y + 5));
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
        CardKind::Ble => {
            draw_pattern_colored(
                display,
                x,
                y,
                &[
                    "    #    ",
                    "    ##   ",
                    "  # # #  ",
                    "   ###   ",
                    "    #    ",
                    "   ###   ",
                    "  # # #  ",
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
        // ESP-NOW: an omni broadcast node, distinct from WiFi's upward arcs and LoRa's mast.
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
        // TCP: a two-way exchange, a right-arrow over a left-arrow.
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

/// Draw one card at `top`: an outlined box with a name line (icon + label), traffic and stats.
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
    if card.liveness.is_failed() {
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

    let tx_y = top + 13;
    let rx_y = top + 22;
    let live_y = top + 31;
    let whole_card_word = match card.liveness {
        Liveness::Failed => Some("Failed"),
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
    draw_card_peek_to(display, top, card, selected, HEIGHT);
}

fn draw_card_peek_to<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    card: &Card,
    selected: bool,
    bottom: i32,
) {
    let bottom = bottom.clamp(top + 1, HEIGHT);
    line(display, Point::new(0, top), Point::new(WIDTH - 1, top));
    line(display, Point::new(0, top), Point::new(0, bottom - 1));
    line(
        display,
        Point::new(WIDTH - 1, top),
        Point::new(WIDTH - 1, bottom - 1),
    );

    if top + NAME_LINE_Y + 9 >= bottom {
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
    if card.liveness.is_failed() {
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

fn draw_footer<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    top: i32,
    footer: UiFooter<'_>,
    selected: bool,
) {
    draw_footer_line(
        display,
        footer.line1,
        top,
        &FONT_6X10,
        FONT_6X10_CHAR_W,
        selected,
    );
    if let Some(line2) = footer.line2 {
        draw_footer_line(
            display,
            line2,
            top + FOOTER_SECOND_LINE_OFFSET,
            &FONT_5X8,
            FONT_5X8_CHAR_W,
            selected,
        );
    }
    if let Some(line3) = footer.line3 {
        draw_footer_line(
            display,
            line3,
            top + FOOTER_THIRD_LINE_OFFSET,
            &FONT_6X10,
            FONT_6X10_CHAR_W,
            selected,
        );
    }
    if let Some(line4) = footer.line4 {
        draw_footer_line(
            display,
            line4,
            top + FOOTER_FOURTH_LINE_OFFSET,
            &FONT_5X8,
            FONT_5X8_CHAR_W,
            selected,
        );
    }
}

fn draw_footer_line<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    text: &str,
    y: i32,
    font: &'static MonoFont<'static>,
    char_w: i32,
    selected: bool,
) {
    if !(CARD_TOP..HEIGHT).contains(&y) {
        return;
    }
    let style = MonoTextStyle::new(
        font,
        if selected {
            BinaryColor::Off
        } else {
            BinaryColor::On
        },
    );
    let width = text.chars().count() as i32 * char_w;
    let x = ((WIDTH - width) / 2).max(0);
    if selected {
        let _ = Rectangle::new(
            Point::new(x.saturating_sub(2), y.saturating_sub(1)),
            Size::new(
                (width + 4).min(WIDTH) as u32,
                font.character_size.height + 2,
            ),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display);
    }
    let _ = Text::with_baseline(text, Point::new(x, y), style, Baseline::Top).draw(display);
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

fn draw_failure_reason<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    mut y: i32,
    reason: &str,
) {
    const LINE_H: i32 = 7;
    const MAX_CHARS: usize = ((WIDTH - MENU_REASON_X) / FONT_4X6_CHAR_W) as usize;
    let style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    let draw_line = |display: &mut D, y: &mut i32, line: &str| {
        if *y > HEIGHT - LINE_H {
            return false;
        }
        let _ = Text::with_baseline(line, Point::new(MENU_REASON_X, *y), style, Baseline::Top)
            .draw(display);
        *y += LINE_H;
        true
    };

    if !draw_line(display, &mut y, "Fail:") {
        return;
    }

    let mut line: heapless::String<24> = heapless::String::new();
    for word in reason.split_whitespace() {
        let sep = usize::from(!line.is_empty());
        let would_len = line.chars().count() + sep + word.chars().count();
        if would_len > MAX_CHARS && !line.is_empty() {
            if !draw_line(display, &mut y, &line) {
                return;
            }
            line.clear();
        }
        if !line.is_empty() {
            let _ = line.push(' ');
        }
        for ch in word.chars().take(MAX_CHARS) {
            let _ = line.push(ch);
        }
    }
    if !line.is_empty() {
        let _ = draw_line(display, &mut y, &line);
    }
}

fn draw_interface_menu_details<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    mut y: i32,
    rows: &[InterfaceMenuDetailRow],
) -> i32 {
    let style = MonoTextStyle::new(&FONT_4X6, BinaryColor::On);
    for row in rows {
        if y > HEIGHT - MENU_DETAIL_STEP {
            break;
        }
        let x = match row.kind {
            InterfaceMenuDetailKind::Info => MENU_REASON_X,
            InterfaceMenuDetailKind::Peer => MENU_REASON_X + 4,
        };
        let _ =
            Text::with_baseline(row.text(), Point::new(x, y), style, Baseline::Top).draw(display);
        y += MENU_DETAIL_STEP;
    }
    y
}

fn draw_global_menu<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    selected_item: usize,
    display_power_capable: bool,
    ap_capable: bool,
    ap_active: bool,
) {
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

    let items = match (display_power_capable, ap_capable) {
        (true, true) => GLOBAL_MENU_ITEMS_AP_DISPLAY,
        (true, false) => GLOBAL_MENU_ITEMS_DISPLAY,
        (false, true) => GLOBAL_MENU_ITEMS_AP,
        (false, false) => GLOBAL_MENU_ITEMS,
    };
    let radio_menu_item = if display_power_capable {
        RADIO_MENU_ITEM
    } else {
        RADIO_MENU_ITEM_NO_DISPLAY
    };
    for (index, item) in items.iter().enumerate() {
        let label = if index == radio_menu_item && ap_capable {
            if ap_active {
                "BLE Mode"
            } else {
                "AP Mode"
            }
        } else {
            *item
        };
        draw_menu_item(
            display,
            MENU_ITEM_TOP + index as i32 * GLOBAL_MENU_ITEM_STEP,
            label,
            index == selected_item.min(items.len() - 1),
        );
    }
}

fn draw_radio_confirm<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    confirm: bool,
    ap_active: bool,
) {
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        "Radio",
        Point::new(NAME_TEXT_X, MENU_HEADER_Y),
        header_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );
    let body = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let prompt = if ap_active { "To BLE?" } else { "To AP?" };
    let _ = Text::with_baseline(prompt, Point::new(2, MENU_ITEM_TOP), body, Baseline::Top)
        .draw(display);
    let _ = Text::with_baseline(
        "BLE off,",
        Point::new(2, MENU_ITEM_TOP + 9),
        body,
        Baseline::Top,
    )
    .draw(display);
    let _ = Text::with_baseline(
        "restarts",
        Point::new(2, MENU_ITEM_TOP + 18),
        body,
        Baseline::Top,
    )
    .draw(display);
    draw_menu_item(display, MENU_ITEM_TOP + 31, "No", !confirm);
    draw_menu_item(display, MENU_ITEM_TOP + 44, "Yes", confirm);
}

fn limit_page_count(rows: &[LimitRow]) -> usize {
    rows.len().max(1).div_ceil(LIMITS_PER_PAGE)
}

fn fmt_limit_value(value: LimitValue) -> HString<12> {
    let mut s = HString::new();
    match value {
        LimitValue::Count(value) => {
            let _ = write!(s, "{value}");
        }
        LimitValue::Bytes(value) => {
            let _ = write!(s, "{}", fmt_bytes(value));
        }
        LimitValue::Range(low, high) => {
            let _ = write!(s, "{low}-{high}");
        }
        LimitValue::RateBytesPerSec(value) => {
            let rate = fmt_rate_bytes_per_sec(value.min(u64::from(u32::MAX)) as u32);
            let _ = write!(s, "{rate}/s");
        }
        LimitValue::Text(value) => {
            let _ = write!(s, "{value}");
        }
    }
    s
}

fn draw_limits_text<D: DrawTarget<Color = BinaryColor>>(display: &mut D, y: i32, text: &str) {
    let style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(text, Point::new(2, y), style, Baseline::Top).draw(display);
}

fn draw_limits_page<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    page: usize,
    rows: &[LimitRow],
) {
    let page_count = limit_page_count(rows);
    let page = page.min(page_count - 1);
    let mut header: HString<16> = HString::new();
    let _ = write!(header, "Limits {}/{}", page + 1, page_count);
    let header_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        &header,
        Point::new(2, CARD_TOP + 2),
        header_style,
        Baseline::Top,
    )
    .draw(display);
    line(
        display,
        Point::new(0, MENU_DIVIDER_Y),
        Point::new(WIDTH - 1, MENU_DIVIDER_Y),
    );

    let start = page * LIMITS_PER_PAGE;
    for (offset, row) in rows.iter().skip(start).take(LIMITS_PER_PAGE).enumerate() {
        let value = fmt_limit_value(row.value);
        let mut line_buf: HString<16> = HString::new();
        let _ = write!(line_buf, "{} {value}", row.label);
        draw_limits_text(display, CARD_TOP + 29 + offset as i32 * 11, &line_buf);
    }
}

fn draw_sleeping<D: DrawTarget<Color = BinaryColor>>(display: &mut D) {
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(
        "Sleeping",
        Point::new(7, CARD_TOP + 20),
        style,
        Baseline::Top,
    )
    .draw(display);
    let hint = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(
        "ifaces off",
        Point::new(7, CARD_TOP + 36),
        hint,
        Baseline::Top,
    )
    .draw(display);
    let _ = Text::with_baseline(
        "press wake",
        Point::new(7, CARD_TOP + 48),
        hint,
        Baseline::Top,
    )
    .draw(display);
}

fn draw_notice<D: DrawTarget<Color = BinaryColor>>(display: &mut D, notice: UiNotice) {
    let label = notice.label();
    let char_count = label.chars().count() as i32;
    let x = ((WIDTH - char_count * FONT_5X8_CHAR_W) / 2).max(0);
    let style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);
    let _ = Text::with_baseline(label, Point::new(x, CARD_TOP + 27), style, Baseline::Top)
        .draw(display);
}

const LORA_EDITOR_TOP: i32 = CARD_TOP + 2;
const LORA_DOT_X: i32 = 1;
const LORA_DOT_SIZE: u32 = 2;
const LORA_ROW_TEXT_X: i32 = 6;
const LORA_ROW_BACKING_H: u32 = 10;
const LORA_VISIBLE_ROWS: usize = 7;

fn custom_row_label(row: CustomRow) -> &'static str {
    match row {
        CustomRow::SpreadingFactor => "SF",
        CustomRow::Bandwidth => "BW",
        CustomRow::CodingRate => "CR",
        CustomRow::FreqMhz => "MHz",
        CustomRow::FreqKhz => "kHz",
        CustomRow::TxPower => "Pwr",
        CustomRow::Save => "Save",
        CustomRow::Back => "Back",
    }
}

fn custom_row_value(row: CustomRow, profile: &RadioProfile) -> heapless::String<12> {
    let Modulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    let mut value = heapless::String::new();
    match row {
        CustomRow::SpreadingFactor => {
            let _ = write!(value, "{}", spreading_factor as u8);
        }
        CustomRow::Bandwidth => {
            let _ = write!(value, "{}k", bandwidth.hz() / 1000);
        }
        CustomRow::CodingRate => {
            let _ = write!(value, "4/{}", coding_rate.denominator());
        }
        CustomRow::TxPower => {
            let _ = write!(value, "{}dB", profile.tx_power.dbm());
        }
        CustomRow::FreqMhz | CustomRow::FreqKhz | CustomRow::Save | CustomRow::Back => {}
    }
    value
}

fn push_freq_digit(text: &mut heapless::String<16>, digit: u32, active: bool) {
    if active {
        let _ = write!(text, "[{digit}]");
    } else {
        let _ = write!(text, "{digit}");
    }
}

fn lora_freq_mhz_text(hz: u32, place: Option<FreqPlace>) -> heapless::String<16> {
    let mut text = heapless::String::new();
    push_freq_digit(
        &mut text,
        (hz / 100_000_000) % 10,
        place == Some(FreqPlace::Hundreds),
    );
    push_freq_digit(
        &mut text,
        (hz / 10_000_000) % 10,
        place == Some(FreqPlace::Tens),
    );
    push_freq_digit(
        &mut text,
        (hz / 1_000_000) % 10,
        place == Some(FreqPlace::Ones),
    );
    text
}

fn lora_freq_khz_text(hz: u32, place: Option<FreqPlace>) -> heapless::String<16> {
    let mut text = heapless::String::new();
    let _ = text.push('.');
    push_freq_digit(
        &mut text,
        (hz / 100_000) % 10,
        place == Some(FreqPlace::Tenths),
    );
    push_freq_digit(
        &mut text,
        (hz / 10_000) % 10,
        place == Some(FreqPlace::Hundredths),
    );
    push_freq_digit(
        &mut text,
        (hz / 1_000) % 10,
        place == Some(FreqPlace::Thousandths),
    );
    text
}

fn lora_custom_row_text(
    row: CustomRow,
    edit: EditMode,
    selected: bool,
    profile: &RadioProfile,
) -> heapless::String<16> {
    let mut text = heapless::String::new();
    if matches!(row, CustomRow::Save | CustomRow::Back) {
        let _ = text.push_str(custom_row_label(row));
        return text;
    }
    let label = custom_row_label(row);
    let hz = profile.frequency.hz();
    let active_place = match edit {
        EditMode::Freq { place } if selected => Some(place),
        _ => None,
    };
    match row {
        CustomRow::FreqMhz => {
            let value = lora_freq_mhz_text(hz, active_place);
            let _ = write!(text, "{label} {value}");
        }
        CustomRow::FreqKhz => {
            let value = lora_freq_khz_text(hz, active_place);
            let _ = write!(text, "{label} {value}");
        }
        _ => {
            let value = custom_row_value(row, profile);
            if selected && matches!(edit, EditMode::Field) {
                let _ = write!(text, "{label} [{value}]");
            } else {
                let _ = write!(text, "{label} {value}");
            }
        }
    }
    text
}

fn draw_lora_list_row<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    y: i32,
    text: &str,
    selected: bool,
) {
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
    let _ = Text::with_baseline(text, Point::new(LORA_ROW_TEXT_X, y), style, Baseline::Top)
        .draw(display);
}

fn region_choice_label(index: usize) -> &'static str {
    if index == LORA_REGION_CANCEL {
        "Cancel"
    } else {
        Region::ALL[index.min(Region::ALL.len() - 1)].label()
    }
}

fn draw_lora_region_picker<D: DrawTarget<Color = BinaryColor>>(display: &mut D, cursor: usize) {
    let start = scroll_start(cursor, LORA_REGION_COUNT, LORA_VISIBLE_ROWS);
    for slot in start..(start + LORA_VISIBLE_ROWS).min(LORA_REGION_COUNT) {
        let y = LORA_EDITOR_TOP + (slot - start) as i32 * MENU_ITEM_STEP;
        draw_lora_list_row(display, y, region_choice_label(slot), slot == cursor);
    }
}

fn preset_choice_label(choice: PresetChoice) -> &'static str {
    match choice {
        PresetChoice::Preset(preset) => preset.label(),
        PresetChoice::Custom => "Custom",
        PresetChoice::Back => "Back",
    }
}

fn draw_lora_preset_picker<D: DrawTarget<Color = BinaryColor>>(display: &mut D, cursor: usize) {
    for (slot, &choice) in PRESET_CHOICES.iter().enumerate() {
        let y = LORA_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
        draw_lora_list_row(display, y, preset_choice_label(choice), slot == cursor);
    }
}

fn draw_lora_custom<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cursor: CustomRow,
    edit: EditMode,
    profile: &RadioProfile,
) {
    for (slot, &row) in CUSTOM_ROWS.iter().enumerate() {
        let y = LORA_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
        let selected = row == cursor;
        let text = lora_custom_row_text(row, edit, selected, profile);
        draw_lora_list_row(display, y, &text, selected);
    }
}

fn lora_freq_row_text(
    row: FreqRow,
    edit: EditMode,
    selected: bool,
    profile: &RadioProfile,
) -> heapless::String<16> {
    let mut text = heapless::String::new();
    let hz = profile.frequency.hz();
    let active_place = match edit {
        EditMode::Freq { place } if selected => Some(place),
        _ => None,
    };
    match row {
        FreqRow::Channel => {
            let channel = current_channel(profile);
            if selected && matches!(edit, EditMode::Field) {
                let _ = write!(text, "Ch [{channel}]");
            } else {
                let count = channel_count(profile);
                let _ = write!(text, "Ch {channel}/{count}");
            }
        }
        FreqRow::Mhz => {
            let value = lora_freq_mhz_text(hz, active_place);
            let _ = write!(text, "MHz {value}");
        }
        FreqRow::Khz => {
            let value = lora_freq_khz_text(hz, active_place);
            let _ = write!(text, "kHz {value}");
        }
        FreqRow::Save => {
            let _ = text.push_str("Save");
        }
        FreqRow::Back => {
            let _ = text.push_str("Back");
        }
    }
    text
}

fn draw_lora_frequency<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cursor: FreqRow,
    edit: EditMode,
    profile: &RadioProfile,
) {
    for (slot, &row) in FREQ_ROWS.iter().enumerate() {
        let y = LORA_EDITOR_TOP + slot as i32 * MENU_ITEM_STEP;
        let selected = row == cursor;
        let text = lora_freq_row_text(row, edit, selected, profile);
        draw_lora_list_row(display, y, &text, selected);
    }
}

fn draw_lora_editor<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    screen: LoRaScreen,
    profile: &RadioProfile,
) {
    match screen {
        LoRaScreen::Region { cursor } => draw_lora_region_picker(display, cursor),
        LoRaScreen::Preset { cursor } => draw_lora_preset_picker(display, cursor),
        LoRaScreen::Frequency { cursor, edit } => {
            draw_lora_frequency(display, cursor, edit, profile)
        }
        LoRaScreen::Custom { cursor, edit } => draw_lora_custom(display, cursor, edit, profile),
    }
}

fn draw_interface_menu<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    card: &Card,
    selected_item: usize,
    details: &[InterfaceMenuDetailRow],
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

    let items = interface_menu_items(card.kind);
    for (index, item) in items.iter().enumerate() {
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
            index == selected_item.min(items.len() - 1),
        );
    }
    let mut detail_y = MENU_ITEM_TOP + items.len() as i32 * MENU_ITEM_STEP + 1;
    if !details.is_empty() {
        detail_y = draw_interface_menu_details(display, detail_y, details);
    }
    if card.liveness.is_failed() {
        if let Some(reason) = card.failure_reason {
            draw_failure_reason(display, detail_y - 1, reason);
        }
    }
}

/// Render the full screen: title bar + a card per interface (up to what fits).
/// Clears first; the caller flushes.
pub fn draw<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
) {
    draw_at(display, cards, battery, 0);
}

pub fn draw_at<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    animation_ms: u64,
) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, battery, animation_ms);
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

/// Render using [`UiState`] for selection and pagination: the real-interaction path. Plain
/// [`draw`] remains for static/manual selected-card rendering.
pub fn draw_with_state<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
) {
    draw_with_state_at(display, cards, battery, state, 0);
}

pub fn draw_with_state_at<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
    animation_ms: u64,
) {
    draw_with_state_footer_at(display, cards, battery, state, None, animation_ms);
}

pub fn draw_with_state_footer_at<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
    footer: Option<UiFooter<'_>>,
    animation_ms: u64,
) {
    draw_with_state_footer_details_at(display, cards, battery, state, footer, &[], animation_ms);
}

pub fn draw_with_state_footer_details_at<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    cards: &[Card],
    battery: BatteryState,
    state: &UiState,
    footer: Option<UiFooter<'_>>,
    interface_menu_details: &[InterfaceMenuDetailRow],
    animation_ms: u64,
) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, battery, animation_ms);

    if let Some(notice) = state.notice() {
        draw_notice(display, notice);
        return;
    }

    if let UiMode::LoRaEditor { screen, profile } = state.mode {
        draw_lora_editor(display, screen, &profile);
        return;
    }

    if let UiMode::LimitsPage { page } = state.mode {
        let rows = build_limit_rows(state.storage_limits);
        draw_limits_page(display, page, &rows);
        return;
    }

    if state.mode == UiMode::Sleeping {
        draw_sleeping(display);
        return;
    }

    if let UiMode::ConfirmRadioSwap { confirm } = state.mode {
        draw_radio_confirm(display, confirm, state.ap_active);
        return;
    }

    if let Some(selected_item) = state.global_menu_selected_item() {
        draw_global_menu(
            display,
            selected_item,
            state.display_power_capable,
            state.ap_capable,
            state.ap_active,
        );
        return;
    }

    if let Some(selected_item) = state.interface_menu_selected_item() {
        if let Some(selected_card) = state.selected_card(cards.len()) {
            draw_interface_menu(
                display,
                &cards[selected_card],
                selected_item,
                interface_menu_details,
            );
            return;
        }
    }

    let selected = state.selected_card(cards.len());
    let item_count = focus_item_count_with_footer(cards.len(), footer.is_some());
    let footer_focus = cards.len() + 1;
    let start = visible_start_for(item_count, state.selected_focus, state.visible_start);
    let mut top = CARD_TOP;
    let mut focus_index = start;
    if start == 0 {
        draw_global_row(display, GLOBAL_ROW_TOP, state.global_selected());
        top = FIRST_CARD_WITH_GLOBAL_TOP;
        focus_index = 1;
    }
    while top < HEIGHT && focus_index < item_count {
        if focus_index == footer_focus {
            if let Some(footer) = footer {
                draw_footer(
                    display,
                    top + 2,
                    footer,
                    state.selected_focus == footer_focus,
                );
            }
        } else {
            let card_index = focus_index - 1;
            let selected_card = selected == Some(card_index);
            if top + CARD_H <= HEIGHT {
                draw_card_with_selection(display, top, &cards[card_index], selected_card);
            } else {
                draw_card_peek(display, top, &cards[card_index], selected_card);
            }
        }
        top += CARD_SLOT_STEP;
        focus_index += 1;
    }
}

/// A boot/connecting splash: title bar + a centered status line.
pub fn splash<D: DrawTarget<Color = BinaryColor>>(display: &mut D, status: &str) {
    let _ = display.clear(BinaryColor::Off);
    draw_title_bar(display, BatteryState::Unknown, 0);
    let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let _ = Text::with_baseline(status, Point::new(2, CARD_TOP + 4), style, Baseline::Top)
        .draw(display);
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use embedded_graphics::mock_display::MockDisplay;

    use super::*;

    const TEST_WIDTH: usize = WIDTH as usize;
    const TEST_HEIGHT: usize = HEIGHT as usize;

    struct PanelDisplay {
        pixels: [[Option<BinaryColor>; TEST_WIDTH]; TEST_HEIGHT],
    }

    impl PanelDisplay {
        fn new() -> Self {
            Self {
                pixels: [[None; TEST_WIDTH]; TEST_HEIGHT],
            }
        }

        fn get_pixel(&self, point: Point) -> Option<BinaryColor> {
            if point.x < 0 || point.y < 0 || point.x >= WIDTH || point.y >= HEIGHT {
                return None;
            }
            self.pixels[point.y as usize][point.x as usize]
        }
    }

    impl DrawTarget for PanelDisplay {
        type Color = BinaryColor;
        type Error = Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x >= 0 && point.y >= 0 && point.x < WIDTH && point.y < HEIGHT {
                    self.pixels[point.y as usize][point.x as usize] = Some(color);
                }
            }
            Ok(())
        }
    }

    impl OriginDimensions for PanelDisplay {
        fn size(&self) -> Size {
            Size::new(WIDTH as u32, HEIGHT as u32)
        }
    }

    fn test_card(label: &'static str) -> Card {
        Card {
            id: InterfaceId::new([0; 8]),
            kind: CardKind::Usb,
            label: card_label(label),
            selected: false,
            liveness: Liveness::Live,
            failure_reason: None,
            tx_bytes: 0,
            rx_bytes: 0,
            links: 0,
            destinations: 0,
            rate_bytes_per_sec: 0,
            last_activity_secs: None,
        }
    }

    fn has_on_pixel(
        display: &PanelDisplay,
        xs: core::ops::Range<i32>,
        ys: core::ops::Range<i32>,
    ) -> bool {
        for y in ys {
            for x in xs.clone() {
                if display.get_pixel(Point::new(x, y)) == Some(BinaryColor::On) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn display_sort_pins_usb_last_and_prioritizes_radios() {
        let mut cards: HVec<Card, 8> = HVec::new();
        for kind in [
            CardKind::Usb,
            CardKind::Wifi,
            CardKind::Tcp,
            CardKind::Ble,
            CardKind::EspNow,
            CardKind::LoRa,
        ] {
            let mut card = test_card("iface");
            card.kind = kind;
            let _ = cards.push(card);
        }

        sort_cards_for_display(&mut cards);

        let kinds: HVec<CardKind, 8> = cards.iter().map(|card| card.kind).collect();
        assert_eq!(
            kinds.as_slice(),
            &[
                CardKind::LoRa,
                CardKind::Wifi,
                CardKind::Ble,
                CardKind::EspNow,
                CardKind::Tcp,
                CardKind::Usb,
            ]
        );
    }

    #[test]
    fn activity_tracker_stamps_age_when_a_card_changes() {
        let mut tracker = CardActivityTracker::<2>::new();
        let mut cards = [test_card("USB")];
        cards[0].liveness = Liveness::Dormant;

        tracker.update(&mut cards, 10);
        assert_eq!(cards[0].last_activity_secs, None);

        cards[0].rx_bytes = 16;
        tracker.update(&mut cards, 12);
        assert_eq!(cards[0].last_activity_secs, Some(0));

        tracker.update(&mut cards, 17);
        assert_eq!(cards[0].last_activity_secs, Some(5));
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

        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
        assert_eq!(state.global_menu_selected_item(), Some(2));
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
        assert_eq!(state.global_menu_selected_item(), Some(3));

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
    fn long_press_on_limits_opens_the_paged_limits_page() {
        let mut state = UiState::new();
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::None
        );
        assert_eq!(state.mode, UiMode::LimitsPage { page: 0 });
        assert_eq!(state.menu_selected_item(), None);
        assert_eq!(
            state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb)),
            UiAction::None
        );
        assert_eq!(state.mode, UiMode::LimitsPage { page: 1 });
        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::None
        );
        assert!(state.global_selected());
    }

    #[test]
    fn long_press_on_sleep_enters_sleep_and_next_press_wakes() {
        let mut state = UiState::new();
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::Sleep
        );
        assert_eq!(state.mode, UiMode::Sleeping);
        assert_eq!(
            state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb)),
            UiAction::Wake
        );
        assert!(state.global_selected());
    }

    #[test]
    fn oled_capable_menu_offers_display_off_before_sleep() {
        let mut state = UiState::new();
        state.set_display_power_capable(true);
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
        state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));

        assert_eq!(state.global_menu_selected_item(), Some(OLED_OFF_MENU_ITEM));
        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::OledOff
        );
        assert!(state.global_selected());

        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
        for _ in 0..SLEEP_MENU_ITEM {
            state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
        }
        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::Sleep
        );
    }

    #[test]
    fn limit_rows_use_the_supplied_storage_limits() {
        let rows = build_limit_rows(DisplayedStorageLimits {
            upstream_app_destinations: StorageCapacity::Fixed(4),
            held_identities: StorageCapacity::Fixed(2),
            ..DisplayedStorageLimits::DYNAMIC
        });

        let app_dst = rows
            .iter()
            .find(|row| row.label == "AppDst")
            .map(|row| row.value);
        let held_id = rows
            .iter()
            .find(|row| row.label == "HeldID")
            .map(|row| row.value);

        assert_eq!(app_dst, Some(LimitValue::Count(4)));
        assert_eq!(held_id, Some(LimitValue::Count(2)));
    }

    #[test]
    fn long_press_on_back_closes_the_global_menu() {
        let mut state = UiState::new();
        state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb));
        for _ in 0..3 {
            state.handle_input(InputEvent::ShortPress, 4, Some(CardKind::Usb));
        }

        assert_eq!(
            state.handle_input(InputEvent::LongPress, 4, Some(CardKind::Usb)),
            UiAction::None
        );
        assert_eq!(state.menu_selected_item(), None);
        assert!(state.global_selected());
    }

    #[test]
    fn global_menu_cycles_only_actionable_items() {
        let mut state = UiState::new();
        state.handle_input(InputEvent::LongPress, 1, Some(CardKind::Usb));

        assert_eq!(state.global_menu_selected_item(), Some(0));
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        assert_eq!(state.global_menu_selected_item(), Some(1));
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        assert_eq!(state.global_menu_selected_item(), Some(2));
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        assert_eq!(state.global_menu_selected_item(), Some(3));
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        assert_eq!(state.global_menu_selected_item(), Some(0));
    }

    #[test]
    fn non_lora_interface_menus_cycle_power_and_back_only() {
        let mut state = UiState::new();
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        state.handle_input(InputEvent::LongPress, 1, Some(CardKind::Usb));

        assert_eq!(state.interface_menu_selected_item(), Some(0));
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        assert_eq!(state.interface_menu_selected_item(), Some(1));
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::Usb));
        assert_eq!(state.interface_menu_selected_item(), Some(0));
    }

    #[test]
    fn lora_interface_menu_keeps_tune_and_reset() {
        let mut state = UiState::new();
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
        state.handle_input(InputEvent::LongPress, 1, Some(CardKind::LoRa));

        assert_eq!(state.interface_menu_selected_item(), Some(0));
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
        assert_eq!(
            state.interface_menu_selected_item(),
            Some(LORA_TUNE_MENU_ITEM)
        );
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
        assert_eq!(
            state.interface_menu_selected_item(),
            Some(LORA_RESET_MENU_ITEM)
        );
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
        assert_eq!(state.interface_menu_selected_item(), Some(3));
        state.handle_input(InputEvent::ShortPress, 1, Some(CardKind::LoRa));
        assert_eq!(state.interface_menu_selected_item(), Some(0));
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

    fn lora_screen(state: &UiState) -> LoRaScreen {
        match state.mode {
            UiMode::LoRaEditor { screen, .. } => screen,
            other => panic!("not in the lora editor: {other:?}"),
        }
    }

    fn lora_working_profile(state: &UiState) -> RadioProfile {
        match state.mode {
            UiMode::LoRaEditor { profile, .. } => profile,
            other => panic!("not in the lora editor: {other:?}"),
        }
    }

    fn tap(state: &mut UiState, times: usize) {
        for _ in 0..times {
            state.handle_input(InputEvent::ShortPress, 1, None);
        }
    }

    fn preset_choice_index(choice: PresetChoice) -> usize {
        PRESET_CHOICES
            .iter()
            .position(|&candidate| candidate == choice)
            .expect("preset choice is present")
    }

    fn tap_to_preset_choice(state: &mut UiState, choice: PresetChoice) {
        let current = match lora_screen(state) {
            LoRaScreen::Preset { cursor } => cursor,
            other => panic!("not on the preset list: {other:?}"),
        };
        let target = preset_choice_index(choice);
        tap(
            state,
            (target + PRESET_CHOICES.len() - current) % PRESET_CHOICES.len(),
        );
        assert_eq!(lora_screen(state), LoRaScreen::Preset { cursor: target });
    }

    #[test]
    fn the_tuner_opens_on_the_region_list_at_the_current_region() {
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Region {
                cursor: region_index(DEFAULT_915_PROFILE.region),
            }
        );
    }

    #[test]
    fn accepting_a_region_snaps_the_default_frequency_and_power_ceiling() {
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        let target = region_index(Region::Eu868);
        tap(&mut state, target);
        state.handle_input(InputEvent::LongPress, 1, None);

        assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
        let profile = lora_working_profile(&state);
        assert_eq!(profile.region, Region::Eu868);
        assert_eq!(profile.frequency, Region::Eu868.default_frequency());
        assert_eq!(profile.tx_power, Region::Eu868.max_tx_power());
    }

    #[test]
    fn cancel_from_the_region_list_returns_to_cards_without_committing() {
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        tap(
            &mut state,
            LORA_REGION_CANCEL - region_index(DEFAULT_915_PROFILE.region),
        );
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Region {
                cursor: LORA_REGION_CANCEL,
            }
        );
        let action = state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(action, UiAction::None);
        assert_eq!(state.mode, UiMode::Cards);
    }

    #[test]
    fn a_nonpreset_modulation_lands_the_cursor_on_custom() {
        let mut state = UiState::new();
        let mut profile = DEFAULT_915_PROFILE;
        profile.modulation = step_custom_row(DEFAULT_915_PROFILE, CustomRow::Bandwidth).modulation;
        state.open_lora_editor(profile);
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Preset {
                cursor: preset_choice_index(PresetChoice::Custom),
            }
        );
    }

    #[test]
    fn choosing_a_named_preset_applies_it_then_opens_the_frequency_step() {
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
        let action = state.handle_input(InputEvent::LongPress, 1, None);

        assert_eq!(action, UiAction::None);
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Frequency {
                cursor: FreqRow::Channel,
                edit: EditMode::Browsing,
            }
        );
        assert_eq!(
            lora_working_profile(&state).modulation,
            ModemPreset::ShortFast.modulation()
        );
    }

    #[test]
    fn the_channel_row_cycles_to_the_next_band_channel_center() {
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Frequency {
                cursor: FreqRow::Channel,
                edit: EditMode::Browsing,
            }
        );
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::ShortPress, 1, None);

        let hz = lora_working_profile(&state).frequency.hz();
        let (low, _) = Region::Us915.band();
        assert_eq!((hz - low - 125_000) % 250_000, 0);
        assert_eq!(hz, 915_375_000);
    }

    #[test]
    fn the_frequency_step_dials_a_channel_then_saves_with_the_preset() {
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Frequency {
                cursor: FreqRow::Channel,
                edit: EditMode::Browsing,
            }
        );
        tap(&mut state, 2);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 6);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 2);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 5);
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(lora_working_profile(&state).frequency.hz(), 915_625_000);

        tap(&mut state, 1);
        let committed = state.handle_input(InputEvent::LongPress, 1, None);
        let mut expected = DEFAULT_915_PROFILE;
        expected.modulation = ModemPreset::ShortFast.modulation();
        expected.frequency = Frequency::new(915_625_000);
        assert_eq!(committed, UiAction::SetLoRaProfile(expected));
        assert_eq!(state.mode, UiMode::Cards);
    }

    #[test]
    fn back_from_the_frequency_step_returns_to_the_preset_list() {
        let mut state = UiState::new();
        state.open_lora_editor(DEFAULT_915_PROFILE);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap_to_preset_choice(&mut state, PresetChoice::Preset(ModemPreset::ShortFast));
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 4);
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Frequency {
                cursor: FreqRow::Back,
                edit: EditMode::Browsing,
            }
        );
        state.handle_input(InputEvent::LongPress, 1, None);
        assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
    }

    fn open_custom(state: &mut UiState) {
        state.open_lora_editor(DEFAULT_915_PROFILE);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap_to_preset_choice(state, PresetChoice::Custom);
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(
            lora_screen(state),
            LoRaScreen::Custom {
                cursor: CustomRow::SpreadingFactor,
                edit: EditMode::Browsing,
            }
        );
    }

    #[test]
    fn custom_grabs_a_field_steps_it_and_saves() {
        let mut state = UiState::new();
        open_custom(&mut state);
        tap(&mut state, 1);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 1);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 5);
        let committed = state.handle_input(InputEvent::LongPress, 1, None);

        let mut expected = DEFAULT_915_PROFILE;
        expected.modulation = step_custom_row(DEFAULT_915_PROFILE, CustomRow::Bandwidth).modulation;
        assert_eq!(committed, UiAction::SetLoRaProfile(expected));
    }

    #[test]
    fn custom_dials_a_fractional_frequency_across_the_two_rows() {
        let mut state = UiState::new();
        open_custom(&mut state);
        tap(&mut state, 4);
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Custom {
                cursor: CustomRow::FreqKhz,
                edit: EditMode::Browsing,
            }
        );
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 6);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 2);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 5);
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(lora_working_profile(&state).frequency.hz(), 915_625_000);

        tap(&mut state, 2);
        match state.handle_input(InputEvent::LongPress, 1, None) {
            UiAction::SetLoRaProfile(profile) => assert_eq!(profile.frequency.hz(), 915_625_000),
            other => panic!("expected SetLoRaProfile, got {other:?}"),
        }
    }

    #[test]
    fn custom_clamps_an_out_of_band_frequency_to_the_region_edge() {
        let mut state = UiState::new();
        open_custom(&mut state);
        tap(&mut state, 3);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        tap(&mut state, 2);
        state.handle_input(InputEvent::LongPress, 1, None);
        state.handle_input(InputEvent::LongPress, 1, None);
        assert_eq!(lora_working_profile(&state).frequency.hz(), 928_000_000);
    }

    #[test]
    fn back_from_custom_returns_to_the_preset_list() {
        let mut state = UiState::new();
        open_custom(&mut state);
        tap(&mut state, 7);
        assert_eq!(
            lora_screen(&state),
            LoRaScreen::Custom {
                cursor: CustomRow::Back,
                edit: EditMode::Browsing,
            }
        );
        state.handle_input(InputEvent::LongPress, 1, None);
        assert!(matches!(lora_screen(&state), LoRaScreen::Preset { .. }));
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
    fn draw_with_state_footer_scrolls_after_the_last_card() {
        let cards = [test_card("USB"), test_card("BLE"), test_card("WiFi")];
        let mut state = UiState::new();
        let footer = UiFooter::new("Docs", Some("127.0.0.1"));
        for _ in 0..4 {
            state.handle_input_with_footer(
                InputEvent::ShortPress,
                cards.len(),
                true,
                Some(CardKind::Usb),
            );
        }

        assert_eq!(state.selected_card(cards.len()), None);
        assert_eq!(state.visible_start_with_footer(cards.len(), true), 3);

        let mut display = PanelDisplay::new();
        draw_with_state_footer_at(
            &mut display,
            &cards,
            BatteryState::Unknown,
            &state,
            Some(footer),
            0,
        );
        assert!(has_on_pixel(
            &display,
            0..WIDTH,
            (CARD_TOP + CARD_SLOT_STEP)
                ..(CARD_TOP + CARD_SLOT_STEP + FOOTER_SECOND_LINE_OFFSET + 8)
        ));
    }

    #[test]
    fn draw_with_state_footer_can_show_softap_docs_details() {
        let cards = [test_card("USB"), test_card("BLE"), test_card("WiFi")];
        let mut state = UiState::new();
        let footer = UiFooter::with_lines(
            "WifiAP",
            Some("Hopspot-EW53"),
            Some("docs @"),
            Some("192.168.4.1"),
        );
        for _ in 0..4 {
            state.handle_input_with_footer(
                InputEvent::ShortPress,
                cards.len(),
                true,
                Some(CardKind::Usb),
            );
        }

        let mut display = PanelDisplay::new();
        draw_with_state_footer_at(
            &mut display,
            &cards,
            BatteryState::Unknown,
            &state,
            Some(footer),
            0,
        );
        assert!(has_on_pixel(
            &display,
            0..WIDTH,
            (CARD_TOP + CARD_SLOT_STEP + FOOTER_FOURTH_LINE_OFFSET)
                ..(CARD_TOP + CARD_SLOT_STEP + FOOTER_FOURTH_LINE_OFFSET + 10)
        ));
    }

    #[test]
    fn footer_focus_long_press_opens_docs() {
        let mut state = UiState::new();

        assert_eq!(
            state.handle_input_with_footer(InputEvent::ShortPress, 1, true, Some(CardKind::Usb)),
            UiAction::None
        );
        assert_eq!(state.selected_card(1), Some(0));

        assert_eq!(
            state.handle_input_with_footer(InputEvent::ShortPress, 1, true, None),
            UiAction::None
        );
        assert_eq!(state.selected_card(1), None);

        assert_eq!(
            state.handle_input_with_footer(InputEvent::LongPress, 1, true, None),
            UiAction::OpenDocs
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
                failure_reason: None,
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
    fn supervisor_peer_rows_format_count_and_compact_peer_statuses() {
        let mut rows = InterfaceMenuDetailRows::new();
        push_interface_menu_info(&mut rows, "AP", "Hopspot-EW53");
        let count = push_supervisor_peer_rows(
            &mut rows,
            [
                SupervisorPeerMenuStatus {
                    id: InterfaceId::new([0, 0xab, 0xcd, 0, 0, 0, 0, 0]),
                    liveness: Liveness::Live,
                },
                SupervisorPeerMenuStatus {
                    id: InterfaceId::new([0, 0x12, 0x34, 0, 0, 0, 0, 0]),
                    liveness: Liveness::Dormant,
                },
            ],
        );

        assert_eq!(count, 2);
        assert_eq!(rows[0].text(), "AP Hopspot-EW53");
        assert_eq!(rows[1].text(), "Peers 2");
        assert_eq!(rows[2].text(), "P abcd Live");
        assert_eq!(rows[3].text(), "P 1234 Dorm");
        assert_eq!(rows[2].kind(), InterfaceMenuDetailKind::Peer);
    }

    #[test]
    fn named_peer_rows_format_single_link_interfaces() {
        let mut rows = InterfaceMenuDetailRows::new();
        let count = push_named_peer_row(&mut rows, "USB", Some(Liveness::Live));

        assert_eq!(count, 1);
        assert_eq!(rows[0].text(), "Peers 1");
        assert_eq!(rows[1].text(), "P USB Live");
        assert_eq!(rows[1].kind(), InterfaceMenuDetailKind::Peer);

        rows.clear();
        let count = push_named_peer_row(&mut rows, "USB", None);
        assert_eq!(count, 0);
        assert_eq!(rows[0].text(), "Peers 0");
    }

    #[test]
    fn interface_menu_draws_detail_rows_below_actions() {
        let mut display = PanelDisplay::new();
        let mut card = test_card("WiFi/LAN");
        card.kind = CardKind::Wifi;
        let mut rows = InterfaceMenuDetailRows::new();
        push_interface_menu_info(&mut rows, "STA", "None");
        push_interface_menu_info(&mut rows, "AP", "Hopspot-EW53");
        let _ = push_supervisor_peer_rows(
            &mut rows,
            [SupervisorPeerMenuStatus {
                id: InterfaceId::new([0, 0xab, 0xcd, 0, 0, 0, 0, 0]),
                liveness: Liveness::Live,
            }],
        );

        draw_interface_menu(&mut display, &card, POWER_MENU_ITEM, &rows);

        let detail_top = MENU_ITEM_TOP + POWER_ONLY_MENU_ITEMS.len() as i32 * MENU_ITEM_STEP + 1;
        assert!(
            has_on_pixel(&display, MENU_REASON_X..WIDTH, detail_top..HEIGHT),
            "interface menus should render supplied detail rows below the actions"
        );
    }

    #[test]
    fn failed_interface_menu_draws_failure_reason() {
        let mut display = PanelDisplay::new();
        let mut card = test_card("BLE");
        card.kind = CardKind::Ble;
        card.liveness = Liveness::Failed;
        card.failure_reason = Some("BlueZ GATT Channels >1; set Channels=1");

        draw_interface_menu(&mut display, &card, POWER_MENU_ITEM, &[]);

        let reason_top = MENU_ITEM_TOP + POWER_ONLY_MENU_ITEMS.len() as i32 * MENU_ITEM_STEP - 1;
        assert!(
            has_on_pixel(&display, MENU_REASON_X..WIDTH, reason_top..HEIGHT),
            "failed-card menus should show the failure reason below the actions"
        );
    }

    #[test]
    fn each_lora_screen_renders_its_selected_row_within_bounds() {
        let screens = [
            LoRaScreen::Region {
                cursor: LORA_REGION_CANCEL,
            },
            LoRaScreen::Preset {
                cursor: PRESET_CHOICES.len() - 1,
            },
            LoRaScreen::Frequency {
                cursor: FreqRow::Back,
                edit: EditMode::Browsing,
            },
            LoRaScreen::Custom {
                cursor: CustomRow::Back,
                edit: EditMode::Browsing,
            },
        ];
        for screen in screens {
            let mut display = MockDisplay::new();
            display.set_allow_overdraw(true);
            display.set_allow_out_of_bounds_drawing(true);
            let mut state = UiState::new();
            state.open_lora_editor(DEFAULT_915_PROFILE);
            if let UiMode::LoRaEditor { profile, .. } = state.mode {
                state.mode = UiMode::LoRaEditor { screen, profile };
            }

            draw_with_state(&mut display, &[], BatteryState::Unknown, &state);

            assert_eq!(
                display.get_pixel(Point::new(LORA_DOT_X, LORA_EDITOR_TOP + 3)),
                Some(BinaryColor::On)
            );
        }
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
        assert_eq!(fmt_rate_bytes_per_sec(0).as_str(), "0B");
        assert_eq!(fmt_rate_bytes_per_sec(999).as_str(), "999B");
        assert_eq!(fmt_rate_bytes_per_sec(1_200).as_str(), "1.2K");
        assert_eq!(fmt_rate_bytes_per_sec(12_000).as_str(), "12K");
        assert_eq!(fmt_rate_bytes_per_sec(999_999).as_str(), "999K");
        assert_eq!(fmt_rate_bytes_per_sec(1_234_567).as_str(), "1.2M");
        assert_eq!(fmt_rate_bytes_per_sec(1_234_567_890).as_str(), "1.2G");

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

        draw_battery(&mut display, 2, 0, BatteryState::Unknown, true);

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

        draw_battery(&mut display, 2, 0, BatteryState::Charging(62), true);

        assert_eq!(display.get_pixel(Point::new(7, 4)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(10, 4)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(13, 4)), Some(BinaryColor::Off));
    }

    #[test]
    fn charging_battery_hides_only_the_current_tier_on_the_off_phase() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);

        draw_battery(&mut display, 2, 0, BatteryState::Charging(62), false);

        assert_eq!(display.get_pixel(Point::new(7, 4)), None);
        assert_eq!(display.get_pixel(Point::new(10, 4)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(13, 4)), Some(BinaryColor::Off));
    }

    #[test]
    fn charging_battery_draws_right_side_plug_until_full() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);

        draw_battery(&mut display, 2, 0, BatteryState::Charging(62), true);

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

        draw_battery(&mut display, 2, 0, BatteryState::Charging(100), false);

        assert_eq!(display.get_pixel(Point::new(2, 0)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(16, 8)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(0, 4)), Some(BinaryColor::Off));
        assert_eq!(display.get_pixel(Point::new(21, 3)), None);
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
            failure_reason: None,
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
            failure_reason: None,
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
        assert!(8 + compact_numeric_width("999M") < STAT_ICON_X);
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
            liveness: Liveness::Failed,
            failure_reason: Some("BlueZ GATT Channels >1; set Channels=1"),
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
            failure_reason: None,
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
