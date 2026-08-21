pub(in crate::screen) mod lora;

use core::future::Future;

use personal_rns::interfaces::lora::RadioProfile;
use personal_rns::storage::DisplayedStorageLimits;

use crate::PersistenceState;

use super::limits::storage_limit_page_count;
use super::model::{Card, CardKind, ScreenContent};
use lora::{lora_editor_hold, lora_editor_tap, region_index, LoRaHold, LoRaScreen};

const INITIAL_VISIBLE_FOCUS_ITEMS: usize = 3;
const SCROLLED_VISIBLE_FOCUS_ITEMS: usize = 2;
const PERSISTENCE_NOTICE_MILLIS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum GlobalMenuItem {
    Announce,
    Limits,
    Gnss,
    OledOff,
    OledAutoOff,
    Sleep,
    RadioMode,
    Back,
}

const GLOBAL_MENU_ORDER: [GlobalMenuItem; 8] = [
    GlobalMenuItem::Announce,
    GlobalMenuItem::Limits,
    GlobalMenuItem::Gnss,
    GlobalMenuItem::OledOff,
    GlobalMenuItem::OledAutoOff,
    GlobalMenuItem::Sleep,
    GlobalMenuItem::RadioMode,
    GlobalMenuItem::Back,
];

#[cfg(test)]
pub(in crate::screen) const ANNOUNCE_MENU_ITEM: usize = 0;
#[cfg(test)]
pub(in crate::screen) const OLED_OFF_MENU_ITEM: usize = 2;
#[cfg(test)]
pub(in crate::screen) const OLED_AUTO_OFF_MENU_ITEM: usize = 3;
#[cfg(test)]
pub(in crate::screen) const SLEEP_MENU_ITEM: usize = 4;
#[cfg(test)]
pub(in crate::screen) const RADIO_MENU_ITEM_NO_DISPLAY: usize = 3;
pub(in crate::screen) const POWER_MENU_ITEM: usize = 0;
pub(in crate::screen) const POWER_ONLY_MENU_ITEMS: &[&str] = &["Power", "Back"];
pub(in crate::screen) const SHARED_INSTANCE_MENU_ITEMS: &[&str] = &["Power", "RNS Config", "Back"];
pub(in crate::screen) const SHARED_INSTANCE_CONFIG_MENU_ITEM: usize = 1;
pub(in crate::screen) const WIFI_MENU_ITEMS: &[&str] = &["Power", "Station", "Back"];
pub(in crate::screen) const STATION_UPLINK_MENU_ITEM: usize = 1;
const LORA_MENU_ITEMS: &[&str] = &["Power", "Tune", "Reset", "Back"];
pub(in crate::screen) const LORA_TUNE_MENU_ITEM: usize = 1;
pub(in crate::screen) const LORA_RESET_MENU_ITEM: usize = 2;

pub(in crate::screen) fn interface_menu_items(
    kind: CardKind,
    shared_instance_config_export: SharedInstanceConfigExport,
) -> &'static [&'static str] {
    match kind {
        CardKind::LoRa => LORA_MENU_ITEMS,
        CardKind::WifiStation | CardKind::WifiStationDisabled => WIFI_MENU_ITEMS,
        CardKind::SharedInstance
            if shared_instance_config_export == SharedInstanceConfigExport::Available =>
        {
            SHARED_INSTANCE_MENU_ITEMS
        }
        CardKind::Wifi
        | CardKind::Peer
        | CardKind::Usb
        | CardKind::Ble
        | CardKind::EspNow
        | CardKind::SharedInstance
        | CardKind::Tcp => POWER_ONLY_MENU_ITEMS,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEvent {
    ShortPress,
    LongPress,
}

/// What an input asked the app to do. The UI owns focus and menus; anything that reaches beyond the screen surfaces here for the app to act on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiAction {
    None,
    Announce,
    OledOff,
    ToggleOledAutoOff,
    Sleep,
    Wake,
    ControlGnss(crate::GnssReceiverCommand),
    /// Flip the selected card's interface off or back on, keyed by the card's [`id`](crate::screen::Card::id).
    ToggleSelectedInterface,
    ToggleStationUplink,
    OpenLoRaEditor,
    SetLoRaProfile(RadioProfile),
    ResetLoRaProfile,
    SwapRadioMode,
    OpenDocs,
    CopySharedInstanceConfig,
}

prns_macros::iterable_enum! {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum UiNotice {
        Announcing,
        OledOff,
        OledAutoOffOn,
        OledAutoOffOff,
        TurningOff,
        TurningOn,
        DisconnectingAp,
        ReconnectingAp,
        Sleeping,
        Awake,
        Saved,
        ApplyFailed,
        ProfileNotSaved,
        ProfileRecovered,
        ProfileReset,
        IdentityReset,
        IdentityUnstable,
        SaveDeferred,
        SaveFailed,
    }
    #[cfg(test)]
    pub(in crate::screen) const ALL;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioProfileChangeResult {
    Saved,
    ApplyFailed,
    ProfileNotSaved,
}

impl RadioProfileChangeResult {
    #[must_use]
    pub const fn applied(self) -> bool {
        matches!(self, Self::Saved | Self::ProfileNotSaved)
    }

    #[must_use]
    pub const fn notice(self) -> UiNotice {
        match self {
            Self::Saved => UiNotice::Saved,
            Self::ApplyFailed => UiNotice::ApplyFailed,
            Self::ProfileNotSaved => UiNotice::ProfileNotSaved,
        }
    }
}

pub async fn apply_and_persist_radio_profile<Apply, Persist, PersistFuture>(
    apply: Apply,
    persist: Persist,
) -> RadioProfileChangeResult
where
    Apply: Future<Output = bool>,
    Persist: FnOnce() -> PersistFuture,
    PersistFuture: Future<Output = bool>,
{
    if !apply.await {
        RadioProfileChangeResult::ApplyFailed
    } else if persist().await {
        RadioProfileChangeResult::Saved
    } else {
        RadioProfileChangeResult::ProfileNotSaved
    }
}

impl UiNotice {
    pub(in crate::screen) const fn lines(self) -> NoticeLines {
        match self {
            Self::Announcing => NoticeLines::one("Announcing"),
            Self::OledOff => NoticeLines::one("OLED Off"),
            Self::OledAutoOffOn => NoticeLines::two("Auto-off", "On"),
            Self::OledAutoOffOff => NoticeLines::two("Auto-off", "Off"),
            Self::TurningOff => NoticeLines::one("Turning Off"),
            Self::TurningOn => NoticeLines::one("Turning On"),
            Self::DisconnectingAp => NoticeLines::two("Disconnecting", "AP"),
            Self::ReconnectingAp => NoticeLines::two("Reconnecting", "AP"),
            Self::Sleeping => NoticeLines::one("Sleeping"),
            Self::Awake => NoticeLines::one("Awake"),
            Self::Saved => NoticeLines::one("Saved"),
            Self::ApplyFailed => NoticeLines::one("Apply Failed"),
            Self::ProfileNotSaved => NoticeLines::two("Profile", "Not saved"),
            Self::ProfileRecovered => NoticeLines::two("Profile", "Recovered"),
            Self::ProfileReset => NoticeLines::two("Profile", "Reset"),
            Self::IdentityReset => NoticeLines::two("Identity", "Reset"),
            Self::IdentityUnstable => NoticeLines::two("Identity", "Unstable"),
            Self::SaveDeferred => {
                NoticeLines::three("Save deferred", "Flash cooldown", "Auto retry")
            }
            Self::SaveFailed => NoticeLines::three("Save failed", "Flash error", "Auto retry"),
        }
    }
}

pub(in crate::screen) struct NoticeLines {
    lines: [&'static str; 3],
    len: usize,
}

impl NoticeLines {
    const fn one(first: &'static str) -> Self {
        Self {
            lines: [first, "", ""],
            len: 1,
        }
    }

    const fn two(first: &'static str, second: &'static str) -> Self {
        Self {
            lines: [first, second, ""],
            len: 2,
        }
    }

    const fn three(first: &'static str, second: &'static str, third: &'static str) -> Self {
        Self {
            lines: [first, second, third],
            len: 3,
        }
    }

    pub(in crate::screen) fn as_slice(&self) -> &[&'static str] {
        &self.lines[..self.len]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistenceNotice {
    observed: PersistenceState,
    visible_until: Option<(u64, UiNotice)>,
}

impl PersistenceNotice {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            observed: PersistenceState::Durable,
            visible_until: None,
        }
    }

    pub fn update(
        &mut self,
        ui_state: &mut UiState,
        persistence: PersistenceState,
        now_millis: u64,
    ) -> bool {
        let mut changed = false;
        if let Some((until, notice)) = self.visible_until {
            if now_millis >= until {
                changed = ui_state.clear_notice_if(notice);
                self.visible_until = None;
            }
        }
        if persistence == self.observed {
            return changed;
        }
        let notice = match persistence {
            PersistenceState::Durable => UiNotice::Saved,
            PersistenceState::Deferred => UiNotice::SaveDeferred,
            PersistenceState::Failed => UiNotice::SaveFailed,
        };
        self.observed = persistence;
        self.visible_until = Some((now_millis.saturating_add(PERSISTENCE_NOTICE_MILLIS), notice));
        ui_state.show_notice(notice);
        true
    }
}

impl Default for PersistenceNotice {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayPowerControl {
    Unavailable,
    Available,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointState {
    Unsupported,
    Inactive,
    Active,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedInstanceConfigExport {
    Unavailable,
    Available,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GnssAvailability {
    Unavailable,
    Available,
}

pub struct UiConfiguration {
    pub storage_limits: DisplayedStorageLimits,
    pub display_power_control: DisplayPowerControl,
    pub access_point: AccessPointState,
    pub shared_instance_config_export: SharedInstanceConfigExport,
    pub gnss: GnssAvailability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiState {
    pub(in crate::screen) selected_focus: usize,
    pub(in crate::screen) visible_start: usize,
    pub(in crate::screen) mode: UiMode,
    pub(in crate::screen) display_power_control: DisplayPowerControl,
    pub(in crate::screen) access_point: AccessPointState,
    pub(in crate::screen) shared_instance_config_export: SharedInstanceConfigExport,
    pub(in crate::screen) gnss: GnssAvailability,
    pub(in crate::screen) gnss_visible: bool,
    pub(in crate::screen) notice: Option<UiNotice>,
    pub(in crate::screen) storage_limits: DisplayedStorageLimits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum UiMode {
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

impl UiState {
    pub const fn new(configuration: UiConfiguration) -> Self {
        Self {
            selected_focus: 0,
            visible_start: 0,
            mode: UiMode::Cards,
            display_power_control: configuration.display_power_control,
            access_point: configuration.access_point,
            shared_instance_config_export: configuration.shared_instance_config_export,
            gnss: configuration.gnss,
            gnss_visible: false,
            notice: None,
            storage_limits: configuration.storage_limits,
        }
    }

    pub fn show_notice(&mut self, notice: UiNotice) {
        self.notice = Some(notice);
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    pub fn clear_notice_if(&mut self, notice: UiNotice) -> bool {
        if self.notice != Some(notice) {
            return false;
        }
        self.notice = None;
        true
    }

    pub(in crate::screen) fn notice(&self) -> Option<UiNotice> {
        self.notice
    }

    pub(in crate::screen) fn global_selected(&self) -> bool {
        matches!(self.mode, UiMode::Cards) && self.selected_focus == 0
    }

    pub fn selected_card<'card>(&self, cards: &'card [Card]) -> Option<&'card Card> {
        cards.get(self.selected_card_index(cards.len())?)
    }

    pub(in crate::screen) fn selected_card_index(&self, card_count: usize) -> Option<usize> {
        let card_index = self.selected_focus.checked_sub(1)?;
        if card_index < card_count {
            Some(card_index)
        } else {
            None
        }
    }

    pub(in crate::screen) fn global_menu_selected_item(&self) -> Option<usize> {
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

    pub(in crate::screen) fn interface_menu_selected_item(&self) -> Option<usize> {
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

    #[must_use]
    pub const fn gnss_visible(&self) -> bool {
        self.gnss_visible
    }

    pub(in crate::screen) fn global_menu_items(&self) -> impl Iterator<Item = GlobalMenuItem> + '_ {
        GLOBAL_MENU_ORDER
            .into_iter()
            .filter(|item| self.global_menu_item_available(*item))
    }

    fn global_menu_item_available(&self, item: GlobalMenuItem) -> bool {
        match item {
            GlobalMenuItem::Gnss => self.gnss == GnssAvailability::Available,
            GlobalMenuItem::OledOff | GlobalMenuItem::OledAutoOff => {
                self.display_power_control == DisplayPowerControl::Available
            }
            GlobalMenuItem::RadioMode => self.access_point != AccessPointState::Unsupported,
            GlobalMenuItem::Announce
            | GlobalMenuItem::Limits
            | GlobalMenuItem::Sleep
            | GlobalMenuItem::Back => true,
        }
    }

    fn global_menu_item_count(&self) -> usize {
        self.global_menu_items().count()
    }

    fn global_menu_item(&self, index: usize) -> Option<GlobalMenuItem> {
        self.global_menu_items().nth(index)
    }

    pub(in crate::screen) const fn global_menu_item_label(
        &self,
        item: GlobalMenuItem,
    ) -> &'static str {
        match item {
            GlobalMenuItem::Announce => "Announce",
            GlobalMenuItem::Limits => "Limits",
            GlobalMenuItem::Gnss if self.gnss_visible => "GPS Off",
            GlobalMenuItem::Gnss => "GPS On",
            GlobalMenuItem::OledOff => "OLED Off",
            GlobalMenuItem::OledAutoOff => "Auto Off",
            GlobalMenuItem::Sleep => "Sleep",
            GlobalMenuItem::RadioMode => match self.access_point {
                AccessPointState::Active => "BLE Mode",
                AccessPointState::Inactive => "AP Mode",
                AccessPointState::Unsupported => "Radio Mode",
            },
            GlobalMenuItem::Back => "Back",
        }
    }

    pub fn sync(&mut self, content: ScreenContent<'_, '_>) {
        let item_count = focus_item_count(content);
        self.selected_focus = self.selected_focus.min(item_count - 1);
        self.visible_start = visible_start_for(item_count, self.selected_focus, self.visible_start);

        match self.mode {
            UiMode::Cards
            | UiMode::GlobalMenu { .. }
            | UiMode::LimitsPage { .. }
            | UiMode::Sleeping
            | UiMode::LoRaEditor { .. }
            | UiMode::ConfirmRadioSwap { .. } => {}
            UiMode::InterfaceMenu { .. } if self.selected_card(content.cards).is_none() => {
                self.mode = UiMode::Cards;
            }
            UiMode::InterfaceMenu {
                selected_item,
                kind,
            } => {
                self.mode = UiMode::InterfaceMenu {
                    selected_item: selected_item.min(
                        interface_menu_items(kind, self.shared_instance_config_export).len() - 1,
                    ),
                    kind,
                };
            }
        }
        if let UiMode::GlobalMenu { selected_item } = self.mode {
            let count = self.global_menu_item_count();
            self.mode = UiMode::GlobalMenu {
                selected_item: selected_item.min(count - 1),
            };
        }
    }

    pub fn handle_input(&mut self, event: InputEvent, content: ScreenContent<'_, '_>) -> UiAction {
        if event == InputEvent::ShortPress && self.notice.take().is_some() {
            self.sync(content);
            return UiAction::None;
        }
        let card_count = content.cards.len();
        self.notice = None;
        self.sync(content);
        let item_count = focus_item_count(content);
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
                if content.local_docs.is_some() && self.selected_focus == card_count + 1 =>
            {
                UiAction::OpenDocs
            }
            (InputEvent::LongPress, UiMode::Cards) => {
                if let Some(card) = self.selected_card(content.cards) {
                    self.mode = UiMode::InterfaceMenu {
                        selected_item: 0,
                        kind: card.kind(),
                    };
                }
                UiAction::None
            }
            (InputEvent::ShortPress, UiMode::GlobalMenu { selected_item }) => {
                let count = self.global_menu_item_count();
                self.mode = UiMode::GlobalMenu {
                    selected_item: (selected_item + 1) % count,
                };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::GlobalMenu { selected_item }) => {
                match self.global_menu_item(selected_item) {
                    Some(GlobalMenuItem::Announce) => {
                        self.mode = UiMode::Cards;
                        UiAction::Announce
                    }
                    Some(GlobalMenuItem::Limits) => {
                        self.mode = UiMode::LimitsPage { page: 0 };
                        UiAction::None
                    }
                    Some(GlobalMenuItem::Gnss) => {
                        self.gnss_visible = !self.gnss_visible;
                        self.mode = UiMode::Cards;
                        UiAction::ControlGnss(if self.gnss_visible {
                            crate::GnssReceiverCommand::Enable
                        } else {
                            crate::GnssReceiverCommand::Disable
                        })
                    }
                    Some(GlobalMenuItem::OledOff) => {
                        self.mode = UiMode::Cards;
                        UiAction::OledOff
                    }
                    Some(GlobalMenuItem::OledAutoOff) => {
                        self.mode = UiMode::Cards;
                        UiAction::ToggleOledAutoOff
                    }
                    Some(GlobalMenuItem::Sleep) => {
                        self.mode = UiMode::Sleeping;
                        UiAction::Sleep
                    }
                    Some(GlobalMenuItem::RadioMode) => {
                        self.mode = UiMode::ConfirmRadioSwap { confirm: false };
                        UiAction::None
                    }
                    Some(GlobalMenuItem::Back) | None => {
                        self.mode = UiMode::Cards;
                        UiAction::None
                    }
                }
            }
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
                    selected_item: (selected_item + 1)
                        % interface_menu_items(kind, self.shared_instance_config_export).len(),
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
                    (CardKind::SharedInstance, SHARED_INSTANCE_CONFIG_MENU_ITEM)
                        if self.shared_instance_config_export
                            == SharedInstanceConfigExport::Available =>
                    {
                        UiAction::CopySharedInstanceConfig
                    }
                    (_, POWER_MENU_ITEM) => UiAction::ToggleSelectedInterface,
                    (
                        CardKind::WifiStation | CardKind::WifiStationDisabled,
                        STATION_UPLINK_MENU_ITEM,
                    ) => UiAction::ToggleStationUplink,
                    (CardKind::LoRa, LORA_TUNE_MENU_ITEM) => UiAction::OpenLoRaEditor,
                    (CardKind::LoRa, LORA_RESET_MENU_ITEM) => UiAction::ResetLoRaProfile,
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
        self.sync(content);
        action
    }
}

pub(in crate::screen) fn focus_item_count(content: ScreenContent<'_, '_>) -> usize {
    content.cards.len() + 1 + usize::from(content.local_docs.is_some())
}

pub(in crate::screen) fn visible_start_for(
    item_count: usize,
    selected_focus: usize,
    visible_start: usize,
) -> usize {
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
