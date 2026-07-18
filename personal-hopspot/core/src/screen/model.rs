use core::fmt::Write as _;

use heapless::{String as HString, Vec as HVec};
use personal_rns::interfaces::{ConnectionState, InterfaceId};

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
    pub(in crate::screen) fn is_failed(self) -> bool {
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

/// A small free-form note drawn below the interface card stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiFooter<'a> {
    pub(in crate::screen) line1: &'a str,
    pub(in crate::screen) line2: Option<&'a str>,
    pub(in crate::screen) line3: Option<&'a str>,
    pub(in crate::screen) line4: Option<&'a str>,
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
