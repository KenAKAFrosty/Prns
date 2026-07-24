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
    /// A fleet member a supervisor stood up (a Wi-Fi/USB peer), not an interface a node configured itself. Renders one font-size down — fits its id tag and reads as subordinate to its parent.
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
pub(crate) const fn liveness_from_connection(connection: ConnectionState) -> Liveness {
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
const CARD_LABEL_CAP: usize = 16;
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

const INTERFACE_MENU_DETAIL_TEXT_CAP: usize = 16;
const INTERFACE_MENU_DETAIL_ROWS_CAP: usize = 8;
type InterfaceMenuDetailText = HString<INTERFACE_MENU_DETAIL_TEXT_CAP>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InterfaceMenuDetailKind {
    Info,
    Peer,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct InterfaceMenuDetailRow {
    text: InterfaceMenuDetailText,
    kind: InterfaceMenuDetailKind,
}

pub struct InterfaceMenuDetails {
    rows: HVec<InterfaceMenuDetailRow, INTERFACE_MENU_DETAIL_ROWS_CAP>,
}

pub struct WifiNetworkStatus<'a> {
    pub station_ssid: Option<&'a str>,
    pub access_point_ssid: Option<&'a str>,
}

impl InterfaceMenuDetailRow {
    #[must_use]
    pub(crate) fn text(&self) -> &str {
        self.text.as_str()
    }

    #[must_use]
    pub(crate) const fn kind(&self) -> InterfaceMenuDetailKind {
        self.kind
    }

    fn from_text(kind: InterfaceMenuDetailKind, text: &str) -> Self {
        let mut row = Self {
            text: InterfaceMenuDetailText::new(),
            kind,
        };
        push_truncated(&mut row.text, text);
        row
    }

    fn info(label: &str, value: &str) -> Self {
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

impl InterfaceMenuDetails {
    #[must_use]
    pub const fn empty() -> Self {
        Self { rows: HVec::new() }
    }

    pub(crate) fn as_slice(&self) -> &[InterfaceMenuDetailRow] {
        self.rows.as_slice()
    }

    pub(crate) fn push_info(&mut self, label: &str, value: &str) {
        let _ = self.rows.push(InterfaceMenuDetailRow::info(label, value));
    }

    pub fn push_egress_pressure(&mut self, events: u32) {
        if events == 0 {
            return;
        }
        let mut value = InterfaceMenuDetailText::new();
        let _ = write!(value, "{events}");
        self.push_info("Egress drops", value.as_str());
    }

    pub fn push_ingress_pressure(&mut self, events: u32) {
        if events == 0 {
            return;
        }
        let mut value = InterfaceMenuDetailText::new();
        let _ = write!(value, "{events}");
        self.push_info("Ingress drops", value.as_str());
    }

    pub(crate) fn push_supervisor_peers<I>(&mut self, peers: I) -> usize
    where
        I: IntoIterator<Item = (InterfaceId, Liveness)>,
    {
        let count_index = self.rows.len();
        let _ = self.rows.push(InterfaceMenuDetailRow::from_text(
            InterfaceMenuDetailKind::Info,
            "Peers 0",
        ));
        let mut count = 0usize;
        for (id, liveness) in peers {
            count = count.saturating_add(1);
            let mut text = InterfaceMenuDetailText::new();
            let bytes = id.as_bytes();
            let _ = write!(
                text,
                "P {:02x}{:02x} {}",
                bytes[1],
                bytes[2],
                liveness_short_label(liveness)
            );
            let _ = self.rows.push(InterfaceMenuDetailRow {
                text,
                kind: InterfaceMenuDetailKind::Peer,
            });
        }
        if let Some(row) = self.rows.get_mut(count_index) {
            row.text.clear();
            let _ = write!(row.text, "Peers {count}");
        }
        count
    }

    pub(crate) fn push_named_peer(&mut self, label: &str, liveness: Option<Liveness>) -> usize {
        let count = usize::from(liveness.is_some());
        let mut count_text = InterfaceMenuDetailText::new();
        let _ = write!(count_text, "Peers {count}");
        let _ = self.rows.push(InterfaceMenuDetailRow {
            text: count_text,
            kind: InterfaceMenuDetailKind::Info,
        });
        if let Some(liveness) = liveness {
            let mut text = InterfaceMenuDetailText::new();
            let _ = text.push_str("P ");
            push_truncated(&mut text, label);
            let _ = text.push(' ');
            let _ = text.push_str(liveness_short_label(liveness));
            let _ = self.rows.push(InterfaceMenuDetailRow {
                text,
                kind: InterfaceMenuDetailKind::Peer,
            });
        }
        count
    }
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
    pub(crate) id: InterfaceId,
    pub(crate) kind: CardKind,
    pub(crate) label: CardLabel,
    pub(crate) liveness: Liveness,
    pub(crate) failure_reason: Option<&'static str>,
    pub(crate) tx_bytes: u64,
    pub(crate) rx_bytes: u64,
    pub(crate) links: u32,
    /// Routing-table destinations reachable via this interface.
    pub(crate) destinations: u32,
    pub(crate) rate_bytes_per_sec: u32,
    pub(crate) last_activity_secs: Option<u32>,
}

impl Card {
    #[must_use]
    pub const fn id(&self) -> InterfaceId {
        self.id
    }

    #[must_use]
    pub const fn kind(&self) -> CardKind {
        self.kind
    }

    #[must_use]
    pub const fn liveness(&self) -> Liveness {
        self.liveness
    }
}

pub(crate) fn sort_cards_for_display<const N: usize>(cards: &mut HVec<Card, N>) {
    cards.sort_unstable_by(|a, b| {
        card_display_rank(a.kind)
            .cmp(&card_display_rank(b.kind))
            .then_with(|| a.label.as_str().cmp(b.label.as_str()))
            .then_with(|| a.id.cmp(&b.id))
    });
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

pub struct LocalDocsAccess<'a> {
    pub wifi_ssid: &'a str,
    pub docs_host: &'a str,
}

#[derive(Clone, Copy)]
pub struct ScreenContent<'content, 'docs> {
    pub cards: &'content [Card],
    pub local_docs: Option<&'content LocalDocsAccess<'docs>>,
}
