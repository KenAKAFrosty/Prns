pub(in crate::screen) mod lora;

use personal_rns::interfaces::lora::{RadioProfile, DEFAULT_915_PROFILE};
use personal_rns::storage::DisplayedStorageLimits;

use super::limits::storage_limit_page_count;
use super::model::{Card, CardKind, ScreenContent};
use lora::{lora_editor_hold, lora_editor_tap, region_index, LoRaHold, LoRaScreen};

const INITIAL_VISIBLE_FOCUS_ITEMS: usize = 3;
const SCROLLED_VISIBLE_FOCUS_ITEMS: usize = 2;
pub(in crate::screen) const GLOBAL_MENU_ITEMS: &[&str] = &["Announce", "Limits", "Sleep", "Back"];
pub(in crate::screen) const GLOBAL_MENU_ITEMS_DISPLAY: &[&str] =
    &["Announce", "Limits", "OLED Off", "Sleep", "Back"];
pub(in crate::screen) const GLOBAL_MENU_ITEMS_AP: &[&str] =
    &["Announce", "Limits", "Sleep", "AP Mode", "Back"];
pub(in crate::screen) const GLOBAL_MENU_ITEMS_AP_DISPLAY: &[&str] =
    &["Announce", "Limits", "OLED Off", "Sleep", "AP Mode", "Back"];
pub(in crate::screen) const ANNOUNCE_MENU_ITEM: usize = 0;
const LIMITS_MENU_ITEM: usize = 1;
pub(in crate::screen) const OLED_OFF_MENU_ITEM: usize = 2;
pub(in crate::screen) const SLEEP_MENU_ITEM: usize = 3;
pub(in crate::screen) const RADIO_MENU_ITEM: usize = 4;
const SLEEP_MENU_ITEM_NO_DISPLAY: usize = 2;
pub(in crate::screen) const RADIO_MENU_ITEM_NO_DISPLAY: usize = 3;
pub(in crate::screen) const POWER_MENU_ITEM: usize = 0;
pub(in crate::screen) const POWER_ONLY_MENU_ITEMS: &[&str] = &["Power", "Back"];
const LORA_MENU_ITEMS: &[&str] = &["Power", "Tune", "Reset", "Back"];
pub(in crate::screen) const LORA_TUNE_MENU_ITEM: usize = 1;
pub(in crate::screen) const LORA_RESET_MENU_ITEM: usize = 2;

pub(in crate::screen) fn interface_menu_items(kind: CardKind) -> &'static [&'static str] {
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
    Sleep,
    Wake,
    /// Flip the selected card's interface off or back on, keyed by the card's [`id`](crate::screen::Card::id).
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
    pub(in crate::screen) fn label(self) -> &'static str {
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

pub struct UiConfiguration {
    pub storage_limits: DisplayedStorageLimits,
    pub display_power_control: DisplayPowerControl,
    pub access_point: AccessPointState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiState {
    pub(in crate::screen) selected_focus: usize,
    pub(in crate::screen) visible_start: usize,
    pub(in crate::screen) mode: UiMode,
    pub(in crate::screen) display_power_control: DisplayPowerControl,
    pub(in crate::screen) access_point: AccessPointState,
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

    pub(in crate::screen) fn global_menu_items(&self) -> &'static [&'static str] {
        match (self.display_power_control, self.access_point) {
            (
                DisplayPowerControl::Available,
                AccessPointState::Inactive | AccessPointState::Active,
            ) => GLOBAL_MENU_ITEMS_AP_DISPLAY,
            (DisplayPowerControl::Available, AccessPointState::Unsupported) => {
                GLOBAL_MENU_ITEMS_DISPLAY
            }
            (
                DisplayPowerControl::Unavailable,
                AccessPointState::Inactive | AccessPointState::Active,
            ) => GLOBAL_MENU_ITEMS_AP,
            (DisplayPowerControl::Unavailable, AccessPointState::Unsupported) => GLOBAL_MENU_ITEMS,
        }
    }

    pub(in crate::screen) fn global_radio_menu_item(&self) -> usize {
        match self.display_power_control {
            DisplayPowerControl::Available => RADIO_MENU_ITEM,
            DisplayPowerControl::Unavailable => RADIO_MENU_ITEM_NO_DISPLAY,
        }
    }

    fn global_sleep_menu_item(&self) -> usize {
        match self.display_power_control {
            DisplayPowerControl::Available => SLEEP_MENU_ITEM,
            DisplayPowerControl::Unavailable => SLEEP_MENU_ITEM_NO_DISPLAY,
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

    pub fn handle_input(&mut self, event: InputEvent, content: ScreenContent<'_, '_>) -> UiAction {
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
                OLED_OFF_MENU_ITEM
                    if self.display_power_control == DisplayPowerControl::Available =>
                {
                    self.mode = UiMode::Cards;
                    UiAction::OledOff
                }
                item if item == self.global_sleep_menu_item() => {
                    self.mode = UiMode::Sleeping;
                    UiAction::Sleep
                }
                item if self.access_point != AccessPointState::Unsupported
                    && item == self.global_radio_menu_item() =>
                {
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
