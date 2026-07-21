pub(in crate::screen) mod lora;

use personal_rns::interfaces::lora::{RadioProfile, DEFAULT_915_PROFILE};
use personal_rns::storage::DisplayedStorageLimits;

use super::limits::storage_limit_page_count;
use super::model::CardKind;
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

/// Single-button input as interpreted by the board support layer.
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

/// Interaction state for the Hopspot card stack. The renderer stays data-driven: this only records which focus row/card is selected, which slice of the stack is visible on the panel, and whether a menu is open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiState {
    pub(in crate::screen) selected_focus: usize,
    pub(in crate::screen) visible_start: usize,
    pub(in crate::screen) mode: UiMode,
    pub(in crate::screen) display_power_capable: bool,
    pub(in crate::screen) ap_capable: bool,
    pub(in crate::screen) ap_active: bool,
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

pub(in crate::screen) fn focus_item_count_with_footer(
    card_count: usize,
    has_footer: bool,
) -> usize {
    card_count + 1 + usize::from(has_footer)
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
