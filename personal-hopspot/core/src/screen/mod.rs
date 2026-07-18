//! The "Personal Hopspot" status screen: portrait 64x128, drawn against any `embedded_graphics` `DrawTarget<Color = BinaryColor>`, so the same pixels land on the S3's SSD1306 OLED and on the desktop simulator window.
//!
//! A two-line inverted title bar over a global menu row and a vertical stack of interface cards: a name line (icon + label), stacked up/down traffic, link and tracked-destination counts, live throughput, last-activity age. The glyphs are drawn primitives, not font characters; the icon mapping is one `match`, the single place to enrich. [`UiState`] keeps the selected focus item visible, paging the stack once more interfaces exist than fit, and a long press opens the global or selected interface's menu.

mod render;

pub use render::{
    draw, draw_at, draw_with_state, draw_with_state_at, draw_with_state_footer_at,
    draw_with_state_footer_details_at, splash,
};

use core::fmt::Write as _;

use heapless::{String as HString, Vec as HVec};
use personal_rns::interfaces::lora::core::{
    Frequency, ModemPreset, Modulation, RadioProfile, Region, TxPower, DEFAULT_915_PROFILE,
};
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::routing::links::channel::{channel_mdu, ChannelWindow};
use personal_rns::routing::links::data::link_mdu;
use personal_rns::routing::links::resources::max_part_count;
use personal_rns::routing::links::resources::{
    PART_REQUEST_MAX_RETRIES, RATE_FAST_BYTES_PER_SECOND, WINDOW_MAX, WINDOW_START,
};
use personal_rns::routing::links::MAX_LINK_MTU;
use personal_rns::storage::{DisplayedStorageLimits, StorageCapacity};

const INITIAL_VISIBLE_FOCUS_ITEMS: usize = 3;
const SCROLLED_VISIBLE_FOCUS_ITEMS: usize = 2;
const LIMITS_PER_PAGE: usize = 6;

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
/// Item 0 of every interface menu is the power toggle; its label is rendered live ("Turn Off" / "Turn On") from the card's [`Liveness`], and long-pressing it emits [`UiAction::ToggleSelectedInterface`].
const POWER_MENU_ITEM: usize = 0;
const POWER_ONLY_MENU_ITEMS: &[&str] = &["Power", "Back"];
const LORA_MENU_ITEMS: &[&str] = &["Power", "Tune", "Reset", "Back"];
const LORA_TUNE_MENU_ITEM: usize = 1;
const LORA_RESET_MENU_ITEM: usize = 2;

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
        LimitRow::range("ResWin", WINDOW_START as u32, WINDOW_MAX as u32),
    );
    push_limit_row(
        &mut rows,
        LimitRow::count("Retry", PART_REQUEST_MAX_RETRIES as u32),
    );
    push_limit_row(
        &mut rows,
        LimitRow::rate("Fast", RATE_FAST_BYTES_PER_SECOND),
    );
    push_limit_row(&mut rows, capacity_row("Receipts", limits.receipts));
    push_limit_row(&mut rows, capacity_row("PktHash", limits.packet_hashes));
    push_limit_row(
        &mut rows,
        capacity_row("BlkHole", limits.blackholed_identities),
    );
    match limits.blackhole_reason_bytes {
        StorageCapacity::Fixed(bytes) => {
            push_limit_row(&mut rows, LimitRow::bytes("BlkWhy", bytes as u64));
        }
        StorageCapacity::Dynamic => push_limit_row(&mut rows, LimitRow::text("BlkWhy", "dyn")),
    }
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

fn limit_page_count(rows: &[LimitRow]) -> usize {
    rows.len().max(1).div_ceil(LIMITS_PER_PAGE)
}

fn storage_limit_page_count(limits: DisplayedStorageLimits) -> usize {
    let rows = build_limit_rows(limits);
    limit_page_count(&rows)
}

/// What interface a card represents — the single source for its icon. Add a variant (and its `match` arm in `draw_interface_icon`) as new interface kinds land; never a wildcard, so the compiler flags the missing glyph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardKind {
    Wifi,
    Usb,
    Ble,
    LoRa,
    EspNow,
    Tcp,
    /// A fleet member a supervisor stood up (a WiFi/USB peer), not an interface a node configured itself. Renders one font-size down — fits its id tag and reads as subordinate to its parent.
    Peer,
}

/// How alive an interface's card reads. `Live` is a confirmed link: the full card with numbers. `Dormant` is up and watching with no confirmed link yet (the USB discoverer with nothing plugged): the *live* icon over a "Dormant" body, so a card never pretends to carry traffic it has none of. `Failed` is a genuinely failed interface: the offline icon and a "Failed" body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Liveness {
    Failed,
    Dormant,
    Live,
    /// Deliberately turned off from the UI: keeps its own interface icon (not the failure slash) over an "Off" body, so an interface a user switched off never reads as one that broke.
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

/// The card label's backing buffer: owned, not `&'static str`, so a face can format a runtime tag into it (a discovered peer's id). Truncated to the cap; the panel clips past its width.
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

/// `TCP ` plus as much of the dial target as fits, so several clients are told apart by where they point (`TCP 162.255.87` vs `TCP schttopup.c`) rather than all reading a bare `TCP`.
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

/// Tracks the most recent observed activity for a fixed-size card set. The renderer stays stateless and `no_std`: each face owns one tracker, calls [`update`](Self::update) before drawing, and passes a monotonic seconds counter from whatever clock its platform has.
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

/// What the title-bar battery glyph shows; `Unknown` (a dash) means no plausible battery is detected. Boards without a charge-status signal keep reporting `Level`/`Unknown`.
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

/// What an input asked the app to do. The UI owns focus and menus; anything that reaches beyond the screen surfaces here for the app to act on.
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

/// Interaction state for the Hopspot card stack. The renderer stays data-driven: this only records which focus row/card is selected, which slice of the stack is visible on the panel, and whether a menu is open.
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

    /// Apply one single-button event, returning what the app should do about it. `selected_kind` (read from the card list) lets the interface menu resolve its kind-specific items.
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

#[cfg(test)]
mod tests;
