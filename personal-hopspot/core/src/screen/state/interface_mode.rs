use personal_rns::interfaces::InterfaceMode;

use crate::interface_mode::{
    interface_mode_menu_label, AnnouncesToInternal, InterfaceModeSelection, InterfaceModeSlot,
    INTERFACE_MODE_CHOICES,
};
use crate::screen::model::CardKind;

#[must_use]
pub fn interface_mode_slot(kind: CardKind) -> Option<InterfaceModeSlot> {
    match kind {
        CardKind::Wifi | CardKind::WifiStation | CardKind::WifiStationDisabled => {
            Some(InterfaceModeSlot::Wifi)
        }
        CardKind::Usb => Some(InterfaceModeSlot::Usb),
        CardKind::Ble => Some(InterfaceModeSlot::Ble),
        CardKind::LoRa => Some(InterfaceModeSlot::LoRa),
        CardKind::EspNow => Some(InterfaceModeSlot::EspNow),
        CardKind::SharedInstance => Some(InterfaceModeSlot::SharedInstance),
        CardKind::Tcp => Some(InterfaceModeSlot::Tcp),
        CardKind::Peer => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum InterfaceModeEditorRow {
    Mode(InterfaceMode),
    Back,
}

#[must_use]
pub(in crate::screen) fn interface_mode_editor_row_count() -> usize {
    INTERFACE_MODE_CHOICES.len() + 1
}

#[must_use]
pub(in crate::screen) fn interface_mode_editor_row(cursor: usize) -> Option<InterfaceModeEditorRow> {
    let mode_count = INTERFACE_MODE_CHOICES.len();
    if cursor < mode_count {
        return Some(InterfaceModeEditorRow::Mode(INTERFACE_MODE_CHOICES[cursor]));
    }
    if cursor == mode_count {
        return Some(InterfaceModeEditorRow::Back);
    }
    None
}

#[must_use]
pub(in crate::screen) fn interface_mode_editor_row_label(cursor: usize) -> Option<&'static str> {
    match interface_mode_editor_row(cursor)? {
        InterfaceModeEditorRow::Mode(mode) => Some(interface_mode_menu_label(mode)),
        InterfaceModeEditorRow::Back => Some("Back"),
    }
}

#[must_use]
pub(in crate::screen) fn clamp_interface_mode_editor_cursor(cursor: usize) -> usize {
    cursor.min(interface_mode_editor_row_count().saturating_sub(1))
}

/// Build the selection to persist when the operator picks a mode row.
///
/// Re-selecting Boundary keeps an existing `announces_to_internal` preference; any other
/// choice uses Denied.
#[must_use]
pub(in crate::screen) fn selection_for_mode_choice(
    current: InterfaceModeSelection,
    mode: InterfaceMode,
) -> InterfaceModeSelection {
    let announces_to_internal =
        if mode == InterfaceMode::Boundary && current.mode == InterfaceMode::Boundary {
            current.announces_to_internal
        } else {
            AnnouncesToInternal::Denied
        };
    InterfaceModeSelection {
        mode,
        announces_to_internal,
    }
}

#[must_use]
pub(in crate::screen) fn initial_interface_mode_editor_cursor(
    selection: InterfaceModeSelection,
) -> usize {
    INTERFACE_MODE_CHOICES
        .iter()
        .position(|mode| *mode == selection.mode)
        .unwrap_or(0)
}
