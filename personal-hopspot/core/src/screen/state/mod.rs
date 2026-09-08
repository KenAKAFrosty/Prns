pub(in crate::screen) mod subg;

use core::future::Future;

use personal_rns::interfaces::lora::RadioProfile;
use personal_rns::interfaces::subghz::regions::us915::US915_AUTO_LORA_PROFILE;
use personal_rns::interfaces::subghz::{
    ResolvedSubGMode, SubGConfiguration, SubGConfigurationState,
};
#[cfg(feature = "remote-control-pairing")]
use personal_rns::remote_control::RemoteControlPairingAttemptId;
use personal_rns::storage::DisplayedStorageLimits;
#[cfg(feature = "remote-control-pairing")]
use personal_rns::units::InstantMillis;

use crate::PersistenceState;
#[cfg(feature = "remote-control-pairing")]
use crate::{
    RemoteControlPairingAvailability, RemoteControlTargetPairingPhase,
    RemoteControlTargetPairingState,
};

use super::limits::storage_limit_page_count;
use super::model::{Card, CardKind, ScreenContent, SubGCardState};
use subg::{region_index, subg_editor_hold, subg_editor_tap, SubGEditorOutcome, SubGScreen};

const INITIAL_VISIBLE_FOCUS_ITEMS: usize = 3;
const SCROLLED_VISIBLE_FOCUS_ITEMS: usize = 2;
const PERSISTENCE_NOTICE_MILLIS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum GlobalMenuItem {
    Announce,
    #[cfg(feature = "remote-control-pairing")]
    PairRemoteControl,
    Limits,
    Gnss,
    BlankDisplay,
    DisplayAutoOff,
    Sleep,
    RadioMode,
    Back,
}

#[cfg(feature = "remote-control-pairing")]
const GLOBAL_MENU_ORDER: [GlobalMenuItem; 9] = [
    GlobalMenuItem::Announce,
    GlobalMenuItem::PairRemoteControl,
    GlobalMenuItem::Limits,
    GlobalMenuItem::Gnss,
    GlobalMenuItem::BlankDisplay,
    GlobalMenuItem::DisplayAutoOff,
    GlobalMenuItem::Sleep,
    GlobalMenuItem::RadioMode,
    GlobalMenuItem::Back,
];

#[cfg(not(feature = "remote-control-pairing"))]
const GLOBAL_MENU_ORDER: [GlobalMenuItem; 8] = [
    GlobalMenuItem::Announce,
    GlobalMenuItem::Limits,
    GlobalMenuItem::Gnss,
    GlobalMenuItem::BlankDisplay,
    GlobalMenuItem::DisplayAutoOff,
    GlobalMenuItem::Sleep,
    GlobalMenuItem::RadioMode,
    GlobalMenuItem::Back,
];

#[cfg(test)]
pub(in crate::screen) const ANNOUNCE_MENU_ITEM: usize = 0;
#[cfg(test)]
pub(in crate::screen) const BLANK_DISPLAY_MENU_ITEM: usize = 2;
#[cfg(test)]
pub(in crate::screen) const DISPLAY_AUTO_OFF_MENU_ITEM: usize = 3;
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
const SUBG_MENU_ITEMS: &[&str] = &["Power", "Configure", "Clear", "Back"];
const SUBG_SETUP_MENU_ITEMS: &[&str] = &["Configure", "Back"];
pub(in crate::screen) const SUBG_CONFIGURE_MENU_ITEM: usize = 1;
pub(in crate::screen) const SUBG_CLEAR_MENU_ITEM: usize = 2;
pub(in crate::screen) const SUBG_SETUP_CONFIGURE_MENU_ITEM: usize = 0;

pub(in crate::screen) fn interface_menu_items(
    kind: CardKind,
    shared_instance_config_export: SharedInstanceConfigExport,
) -> &'static [&'static str] {
    match kind {
        CardKind::SubG(SubGCardState::Setup) => SUBG_SETUP_MENU_ITEMS,
        CardKind::SubG(SubGCardState::AutoLoRa | SubGCardState::ManualLoRa) => SUBG_MENU_ITEMS,
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
    #[cfg(feature = "remote-control-pairing")]
    OpenRemoteControlPairing,
    #[cfg(feature = "remote-control-pairing")]
    CloseRemoteControlPairing,
    #[cfg(feature = "remote-control-pairing")]
    ApproveRemoteControlTargetPairing(RemoteControlPairingAttemptId),
    #[cfg(feature = "remote-control-pairing")]
    RejectRemoteControlTargetPairing(RemoteControlPairingAttemptId),
    BlankDisplay,
    ToggleDisplayAutoOff,
    Sleep,
    Wake,
    ControlGnss(crate::GnssReceiverCommand),
    /// Flip the selected card's interface off or back on, keyed by the card's [`id`](crate::screen::Card::id).
    ToggleSelectedInterface,
    ToggleStationUplink,
    OpenSubGEditor,
    SetSubGConfiguration(SubGConfiguration),
    ClearSubGConfiguration,
    SwapRadioMode,
    OpenDocs,
    CopySharedInstanceConfig,
}

impl UiAction {
    #[doc(hidden)]
    #[allow(non_upper_case_globals)]
    pub const DisplayOff: Self = Self::BlankDisplay;
}

prns_macros::iterable_enum! {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum UiNotice {
        Announcing,
        DisplayOff,
        DisplayAutoOffOn,
        DisplayAutoOffOff,
        TurningOff,
        TurningOn,
        DisconnectingAp,
        ReconnectingAp,
        Sleeping,
        Awake,
        Saved,
        ApplyFailed,
        SubGNotSaved,
        SubGRecovered,
        SubGReset,
        SubGMigrated,
        SubGUncertain,
        IdentityReset,
        IdentityUnstable,
        StateRecovered,
        SaveDeferred,
        SaveFailed,
    }
    #[cfg(test)]
    pub(in crate::screen) const ALL;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubGConfigurationChangeResult {
    Saved,
    ApplyFailed,
    PersistenceFailed,
    PersistenceUncertain,
    RollbackFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActiveSubGConfiguration {
    Previous,
    Requested,
}

impl SubGConfigurationChangeResult {
    #[must_use]
    pub const fn committed(self) -> bool {
        matches!(self, Self::Saved)
    }

    #[must_use]
    pub const fn active_configuration(self) -> ActiveSubGConfiguration {
        match self {
            Self::ApplyFailed | Self::PersistenceFailed => ActiveSubGConfiguration::Previous,
            Self::PersistenceUncertain | Self::RollbackFailed => ActiveSubGConfiguration::Requested,
            Self::Saved => ActiveSubGConfiguration::Requested,
        }
    }

    #[must_use]
    pub const fn notice(self) -> UiNotice {
        match self {
            Self::Saved => UiNotice::Saved,
            Self::ApplyFailed => UiNotice::ApplyFailed,
            Self::PersistenceFailed => UiNotice::SubGNotSaved,
            Self::PersistenceUncertain | Self::RollbackFailed => UiNotice::SubGUncertain,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubGConfigurationStepOutcome {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubGConfigurationPersistenceOutcome {
    Committed,
    NotCommitted,
    Indeterminate,
}

#[inline(never)]
pub async fn apply_and_persist_subg_configuration<
    Apply,
    ApplyFuture,
    Persist,
    PersistFuture,
    Rollback,
    RollbackFuture,
>(
    apply: Apply,
    persist: Persist,
    rollback: Rollback,
) -> SubGConfigurationChangeResult
where
    Apply: FnOnce() -> ApplyFuture,
    ApplyFuture: Future<Output = SubGConfigurationStepOutcome>,
    Persist: FnOnce() -> PersistFuture,
    PersistFuture: Future<Output = SubGConfigurationPersistenceOutcome>,
    Rollback: FnOnce() -> RollbackFuture,
    RollbackFuture: Future<Output = SubGConfigurationStepOutcome>,
{
    if matches!(apply().await, SubGConfigurationStepOutcome::Failed) {
        return SubGConfigurationChangeResult::ApplyFailed;
    }
    match persist().await {
        SubGConfigurationPersistenceOutcome::Committed => {
            return SubGConfigurationChangeResult::Saved
        }
        SubGConfigurationPersistenceOutcome::Indeterminate => {
            return SubGConfigurationChangeResult::PersistenceUncertain
        }
        SubGConfigurationPersistenceOutcome::NotCommitted => {}
    }
    match rollback().await {
        SubGConfigurationStepOutcome::Succeeded => SubGConfigurationChangeResult::PersistenceFailed,
        SubGConfigurationStepOutcome::Failed => SubGConfigurationChangeResult::RollbackFailed,
    }
}

impl UiNotice {
    pub(in crate::screen) const fn lines(self) -> NoticeLines {
        match self {
            Self::Announcing => NoticeLines::one("Announcing"),
            Self::DisplayOff => NoticeLines::one("Screen Off"),
            Self::DisplayAutoOffOn => NoticeLines::two("Auto-off", "On"),
            Self::DisplayAutoOffOff => NoticeLines::two("Auto-off", "Off"),
            Self::TurningOff => NoticeLines::one("Turning Off"),
            Self::TurningOn => NoticeLines::one("Turning On"),
            Self::DisconnectingAp => NoticeLines::two("Disconnecting", "AP"),
            Self::ReconnectingAp => NoticeLines::two("Reconnecting", "AP"),
            Self::Sleeping => NoticeLines::one("Sleeping"),
            Self::Awake => NoticeLines::one("Awake"),
            Self::Saved => NoticeLines::one("Saved"),
            Self::ApplyFailed => NoticeLines::one("Apply Failed"),
            Self::SubGNotSaved => NoticeLines::two("SubG", "Not saved"),
            Self::SubGRecovered => NoticeLines::two("SubG", "Recovered"),
            Self::SubGReset => NoticeLines::two("SubG", "Reset"),
            Self::SubGMigrated => NoticeLines::two("SubG", "Migrated"),
            Self::SubGUncertain => NoticeLines::two("SubG", "Uncertain"),
            Self::IdentityReset => NoticeLines::two("Identity", "Reset"),
            Self::IdentityUnstable => NoticeLines::two("Identity", "Unstable"),
            Self::StateRecovered => NoticeLines::two("State", "Recovered"),
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
        let Some(notice) = self.observe(persistence) else {
            return changed;
        };
        self.visible_until = Some((now_millis.saturating_add(PERSISTENCE_NOTICE_MILLIS), notice));
        ui_state.show_notice(notice);
        true
    }

    #[must_use]
    pub fn observe(&mut self, persistence: PersistenceState) -> Option<UiNotice> {
        if persistence == self.observed {
            return None;
        }
        self.observed = persistence;
        Some(match persistence {
            PersistenceState::Durable => UiNotice::Saved,
            PersistenceState::Recovered => UiNotice::StateRecovered,
            PersistenceState::Deferred => UiNotice::SaveDeferred,
            PersistenceState::Failed => UiNotice::SaveFailed,
        })
    }
}

impl Default for PersistenceNotice {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UserBlankingCapability {
    Unavailable,
    Available,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserBlanking(UserBlankingCapability);

impl UserBlanking {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self(UserBlankingCapability::Unavailable)
    }

    pub(in crate::screen) const fn available() -> Self {
        Self(UserBlankingCapability::Available)
    }

    #[must_use]
    pub const fn is_available(self) -> bool {
        matches!(self.0, UserBlankingCapability::Available)
    }
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
    pub user_blanking: UserBlanking,
    pub access_point: AccessPointState,
    pub shared_instance_config_export: SharedInstanceConfigExport,
    pub gnss: GnssAvailability,
    #[cfg(feature = "remote-control-pairing")]
    pub remote_control_pairing: RemoteControlPairingAvailability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiState {
    pub(in crate::screen) selected_focus: usize,
    pub(in crate::screen) visible_start: usize,
    pub(in crate::screen) mode: UiMode,
    pub(in crate::screen) user_blanking: UserBlanking,
    pub(in crate::screen) access_point: AccessPointState,
    pub(in crate::screen) shared_instance_config_export: SharedInstanceConfigExport,
    pub(in crate::screen) gnss: GnssAvailability,
    pub(in crate::screen) gnss_visible: bool,
    pub(in crate::screen) notice: Option<UiNotice>,
    pub(in crate::screen) storage_limits: DisplayedStorageLimits,
    #[cfg(feature = "remote-control-pairing")]
    pub(in crate::screen) remote_control_pairing: RemoteControlPairingAvailability,
    #[cfg(feature = "remote-control-pairing")]
    pub(in crate::screen) remote_control_state: RemoteControlTargetPairingState,
    #[cfg(feature = "remote-control-pairing")]
    pub(in crate::screen) remote_control_now: InstantMillis,
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
    SubGEditor {
        screen: SubGScreen,
        profile: RadioProfile,
    },
    ConfirmSubGClear {
        confirm: bool,
    },
    ConfirmRadioSwap {
        confirm: bool,
    },
    #[cfg(feature = "remote-control-pairing")]
    RemoteControlPairing {
        approve: bool,
    },
}

impl UiState {
    pub const fn new(configuration: UiConfiguration) -> Self {
        Self {
            selected_focus: 0,
            visible_start: 0,
            mode: UiMode::Cards,
            user_blanking: configuration.user_blanking,
            access_point: configuration.access_point,
            shared_instance_config_export: configuration.shared_instance_config_export,
            gnss: configuration.gnss,
            gnss_visible: false,
            notice: None,
            storage_limits: configuration.storage_limits,
            #[cfg(feature = "remote-control-pairing")]
            remote_control_pairing: configuration.remote_control_pairing,
            #[cfg(feature = "remote-control-pairing")]
            remote_control_state: RemoteControlTargetPairingState::new(),
            #[cfg(feature = "remote-control-pairing")]
            remote_control_now: InstantMillis(0),
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

    #[must_use]
    pub const fn visible_notice(&self) -> Option<UiNotice> {
        self.notice
    }

    pub(in crate::screen) const fn notice(&self) -> Option<UiNotice> {
        self.visible_notice()
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
            | UiMode::SubGEditor { .. }
            | UiMode::ConfirmSubGClear { .. }
            | UiMode::ConfirmRadioSwap { .. } => None,
            #[cfg(feature = "remote-control-pairing")]
            UiMode::RemoteControlPairing { .. } => None,
        }
    }

    pub(in crate::screen) fn interface_menu_selected_item(&self) -> Option<usize> {
        match self.mode {
            UiMode::InterfaceMenu { selected_item, .. } => Some(selected_item),
            UiMode::Cards
            | UiMode::GlobalMenu { .. }
            | UiMode::LimitsPage { .. }
            | UiMode::Sleeping
            | UiMode::SubGEditor { .. }
            | UiMode::ConfirmSubGClear { .. }
            | UiMode::ConfirmRadioSwap { .. } => None,
            #[cfg(feature = "remote-control-pairing")]
            UiMode::RemoteControlPairing { .. } => None,
        }
    }

    pub fn open_subg_editor(&mut self, state: SubGConfigurationState) {
        let profile = match state {
            SubGConfigurationState::Unconfigured => US915_AUTO_LORA_PROFILE,
            SubGConfigurationState::Configured(configuration) => {
                let ResolvedSubGMode::LoRa(profile) = configuration.resolve();
                profile
            }
        };
        self.mode = UiMode::SubGEditor {
            screen: SubGScreen::Region {
                cursor: region_index(profile.region()),
            },
            profile,
        };
    }

    #[must_use]
    pub const fn gnss_visible(&self) -> bool {
        self.gnss_visible
    }

    #[cfg(feature = "remote-control-pairing")]
    pub fn sync_remote_control(
        &mut self,
        state: RemoteControlTargetPairingState,
        now: InstantMillis,
    ) {
        self.remote_control_state = state;
        self.remote_control_now = now;
    }

    #[cfg(feature = "remote-control-pairing")]
    pub(in crate::screen) const fn remote_control_state(&self) -> RemoteControlTargetPairingState {
        self.remote_control_state
    }

    #[cfg(feature = "remote-control-pairing")]
    pub(in crate::screen) const fn remote_control_now(&self) -> InstantMillis {
        self.remote_control_now
    }

    #[cfg(feature = "remote-control-pairing")]
    pub(in crate::screen) const fn remote_control_pairing_approval_selected(&self) -> bool {
        matches!(self.mode, UiMode::RemoteControlPairing { approve: true })
    }

    pub(in crate::screen) fn global_menu_items(&self) -> impl Iterator<Item = GlobalMenuItem> + '_ {
        GLOBAL_MENU_ORDER
            .into_iter()
            .filter(|item| self.global_menu_item_available(*item))
    }

    fn global_menu_item_available(&self, item: GlobalMenuItem) -> bool {
        match item {
            GlobalMenuItem::Gnss => self.gnss == GnssAvailability::Available,
            GlobalMenuItem::BlankDisplay | GlobalMenuItem::DisplayAutoOff => {
                self.user_blanking.is_available()
            }
            GlobalMenuItem::RadioMode => self.access_point != AccessPointState::Unsupported,
            #[cfg(feature = "remote-control-pairing")]
            GlobalMenuItem::PairRemoteControl => {
                self.remote_control_pairing == RemoteControlPairingAvailability::Available
            }
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
            #[cfg(feature = "remote-control-pairing")]
            GlobalMenuItem::PairRemoteControl => "Pair remote",
            GlobalMenuItem::Limits => "Limits",
            GlobalMenuItem::Gnss if self.gnss_visible => "GPS Off",
            GlobalMenuItem::Gnss => "GPS On",
            GlobalMenuItem::BlankDisplay => "Screen Off",
            GlobalMenuItem::DisplayAutoOff => "Auto Off",
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
            | UiMode::SubGEditor { .. }
            | UiMode::ConfirmSubGClear { .. }
            | UiMode::ConfirmRadioSwap { .. } => {}
            #[cfg(feature = "remote-control-pairing")]
            UiMode::RemoteControlPairing { .. } => {}
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
        if self.mode != UiMode::Sleeping
            && event == InputEvent::ShortPress
            && self.notice.take().is_some()
        {
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
                    #[cfg(feature = "remote-control-pairing")]
                    Some(GlobalMenuItem::PairRemoteControl) => {
                        self.mode = UiMode::RemoteControlPairing { approve: false };
                        UiAction::OpenRemoteControlPairing
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
                    Some(GlobalMenuItem::BlankDisplay) => {
                        self.mode = UiMode::Cards;
                        UiAction::BlankDisplay
                    }
                    Some(GlobalMenuItem::DisplayAutoOff) => {
                        self.mode = UiMode::Cards;
                        UiAction::ToggleDisplayAutoOff
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
            (InputEvent::ShortPress, UiMode::ConfirmSubGClear { confirm }) => {
                self.mode = UiMode::ConfirmSubGClear { confirm: !confirm };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::ConfirmSubGClear { confirm }) => {
                self.mode = UiMode::Cards;
                if confirm {
                    UiAction::ClearSubGConfiguration
                } else {
                    UiAction::None
                }
            }
            #[cfg(feature = "remote-control-pairing")]
            (InputEvent::ShortPress, UiMode::RemoteControlPairing { approve }) => {
                if self.remote_control_state.phase()
                    == RemoteControlTargetPairingPhase::Confirmation
                {
                    self.mode = UiMode::RemoteControlPairing { approve: !approve };
                }
                UiAction::None
            }
            #[cfg(feature = "remote-control-pairing")]
            (InputEvent::LongPress, UiMode::RemoteControlPairing { approve }) => {
                if self.remote_control_state.phase()
                    == RemoteControlTargetPairingPhase::Confirmation
                {
                    if let Some(attempt_id) = self.remote_control_state.attempt_id() {
                        if approve {
                            UiAction::ApproveRemoteControlTargetPairing(attempt_id)
                        } else {
                            UiAction::RejectRemoteControlTargetPairing(attempt_id)
                        }
                    } else {
                        self.mode = UiMode::Cards;
                        UiAction::CloseRemoteControlPairing
                    }
                } else {
                    self.mode = UiMode::Cards;
                    UiAction::CloseRemoteControlPairing
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
                    (CardKind::SubG(SubGCardState::Setup), SUBG_SETUP_CONFIGURE_MENU_ITEM) => {
                        UiAction::OpenSubGEditor
                    }
                    (CardKind::SharedInstance, SHARED_INSTANCE_CONFIG_MENU_ITEM)
                        if self.shared_instance_config_export
                            == SharedInstanceConfigExport::Available =>
                    {
                        UiAction::CopySharedInstanceConfig
                    }
                    (CardKind::SubG(SubGCardState::Setup), _) => UiAction::None,
                    (_, POWER_MENU_ITEM) => UiAction::ToggleSelectedInterface,
                    (
                        CardKind::WifiStation | CardKind::WifiStationDisabled,
                        STATION_UPLINK_MENU_ITEM,
                    ) => UiAction::ToggleStationUplink,
                    (CardKind::SubG(_), SUBG_CONFIGURE_MENU_ITEM) => UiAction::OpenSubGEditor,
                    (CardKind::SubG(_), SUBG_CLEAR_MENU_ITEM) => {
                        self.mode = UiMode::ConfirmSubGClear { confirm: false };
                        UiAction::None
                    }
                    _ => UiAction::None,
                }
            }
            (InputEvent::ShortPress, UiMode::SubGEditor { screen, profile }) => {
                let (screen, profile) = subg_editor_tap(screen, profile);
                self.mode = UiMode::SubGEditor { screen, profile };
                UiAction::None
            }
            (InputEvent::LongPress, UiMode::SubGEditor { screen, profile }) => {
                match subg_editor_hold(screen, profile) {
                    SubGEditorOutcome::Stay { screen, profile } => {
                        self.mode = UiMode::SubGEditor { screen, profile };
                        UiAction::None
                    }
                    SubGEditorOutcome::Commit(configuration) => {
                        self.mode = UiMode::Cards;
                        UiAction::SetSubGConfiguration(configuration)
                    }
                    SubGEditorOutcome::Cancel => {
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
