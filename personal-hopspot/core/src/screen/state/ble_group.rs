/// Default BLE Auto discovery group. Must match `prns_core` `GROUP_NAME`.
pub const DEFAULT_BLE_GROUP: &str = "reticulum";
pub const BLE_GROUP_NAME_MAX: usize = 16;
pub const BLE_GROUP_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-_";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BleGroupName {
    bytes: [u8; BLE_GROUP_NAME_MAX],
    len: u8,
}

impl BleGroupName {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            bytes: [0; BLE_GROUP_NAME_MAX],
            len: 0,
        }
    }

    #[must_use]
    pub const fn reticulum() -> Self {
        Self::from_bytes(b"reticulum")
    }

    const fn from_bytes(bytes: &[u8]) -> Self {
        let mut name = Self::empty();
        let mut index = 0;
        while index < bytes.len() && index < BLE_GROUP_NAME_MAX {
            name.bytes[index] = bytes[index];
            index += 1;
        }
        name.len = index as u8;
        name
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().as_bytes();
        if value.is_empty() || value.len() > BLE_GROUP_NAME_MAX {
            return None;
        }
        if !value.iter().all(|byte| BLE_GROUP_CHARSET.contains(byte)) {
            return None;
        }
        Some(Self::from_bytes(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    #[must_use]
    pub fn is_reticulum(self) -> bool {
        self.as_str() == DEFAULT_BLE_GROUP
    }

    fn cycle_char(&mut self, index: usize) {
        let current = self.bytes[index];
        let next = BLE_GROUP_CHARSET
            .iter()
            .position(|&candidate| candidate == current)
            .map(|position| BLE_GROUP_CHARSET[(position + 1) % BLE_GROUP_CHARSET.len()])
            .unwrap_or(BLE_GROUP_CHARSET[0]);
        self.bytes[index] = next;
    }

    fn ensure_char(&mut self, index: usize) -> bool {
        if index >= BLE_GROUP_NAME_MAX {
            return false;
        }
        if index >= self.len as usize {
            self.bytes[index] = b'a';
            self.len = (index + 1) as u8;
        }
        true
    }

    fn delete_last(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            self.bytes[self.len as usize] = 0;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum BleGroupScreen {
    Choice {
        cursor: usize,
    },
    Custom {
        cursor: BleGroupCustomRow,
        edit: BleGroupEdit,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum BleGroupChoice {
    UseDefault,
    Custom,
    Back,
}

pub(in crate::screen) const BLE_GROUP_CHOICES: [BleGroupChoice; 3] = [
    BleGroupChoice::UseDefault,
    BleGroupChoice::Custom,
    BleGroupChoice::Back,
];

impl BleGroupChoice {
    pub(in crate::screen) const fn label(self) -> &'static str {
        match self {
            Self::UseDefault => "Use default",
            Self::Custom => "Custom",
            Self::Back => "Back",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum BleGroupCustomRow {
    Name,
    Del,
    Save,
    Back,
}

pub(in crate::screen) const BLE_GROUP_CUSTOM_ROWS: [BleGroupCustomRow; 4] = [
    BleGroupCustomRow::Name,
    BleGroupCustomRow::Del,
    BleGroupCustomRow::Save,
    BleGroupCustomRow::Back,
];

impl BleGroupCustomRow {
    const FIRST: Self = Self::Name;

    fn next(self) -> Self {
        match self {
            Self::Name => Self::Del,
            Self::Del => Self::Save,
            Self::Save => Self::Back,
            Self::Back => Self::Name,
        }
    }

    pub(in crate::screen) const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Del => "Del",
            Self::Save => "Save",
            Self::Back => "Back",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum BleGroupEdit {
    Browsing,
    Char { index: usize },
}

pub(in crate::screen) enum BleGroupHold {
    Stay {
        screen: BleGroupScreen,
        name: BleGroupName,
    },
    Commit(BleGroupName),
    Cancel,
}

pub(in crate::screen) fn choice_cursor_for(name: BleGroupName) -> usize {
    if name.is_reticulum() {
        0
    } else {
        1
    }
}

pub(in crate::screen) fn ble_group_editor_tap(
    screen: BleGroupScreen,
    name: BleGroupName,
) -> (BleGroupScreen, BleGroupName) {
    match screen {
        BleGroupScreen::Choice { cursor } => (
            BleGroupScreen::Choice {
                cursor: (cursor + 1) % BLE_GROUP_CHOICES.len(),
            },
            name,
        ),
        BleGroupScreen::Custom { cursor, edit } => match edit {
            BleGroupEdit::Browsing => (
                BleGroupScreen::Custom {
                    cursor: cursor.next(),
                    edit,
                },
                name,
            ),
            BleGroupEdit::Char { index } => {
                let mut name = name;
                if index >= (name.len as usize) {
                    name.ensure_char(index);
                } else {
                    name.cycle_char(index);
                }
                (
                    BleGroupScreen::Custom {
                        cursor,
                        edit: BleGroupEdit::Char { index },
                    },
                    name,
                )
            }
        },
    }
}

pub(in crate::screen) fn ble_group_editor_hold(
    screen: BleGroupScreen,
    name: BleGroupName,
) -> BleGroupHold {
    match screen {
        BleGroupScreen::Choice { cursor } => match BLE_GROUP_CHOICES[cursor.min(2)] {
            BleGroupChoice::UseDefault => BleGroupHold::Commit(BleGroupName::reticulum()),
            BleGroupChoice::Custom => BleGroupHold::Stay {
                screen: BleGroupScreen::Custom {
                    cursor: BleGroupCustomRow::FIRST,
                    edit: BleGroupEdit::Browsing,
                },
                name: if name.is_reticulum() {
                    BleGroupName::empty()
                } else {
                    name
                },
            },
            BleGroupChoice::Back => BleGroupHold::Cancel,
        },
        BleGroupScreen::Custom { cursor, edit } => match edit {
            BleGroupEdit::Browsing => match cursor {
                BleGroupCustomRow::Name => {
                    let mut name = name;
                    if name.len == 0 {
                        name.ensure_char(0);
                    }
                    BleGroupHold::Stay {
                        screen: BleGroupScreen::Custom {
                            cursor,
                            edit: BleGroupEdit::Char { index: 0 },
                        },
                        name,
                    }
                }
                BleGroupCustomRow::Del => {
                    let mut name = name;
                    name.delete_last();
                    BleGroupHold::Stay {
                        screen: BleGroupScreen::Custom { cursor, edit },
                        name,
                    }
                }
                BleGroupCustomRow::Save => {
                    if name.len == 0 {
                        BleGroupHold::Stay {
                            screen: BleGroupScreen::Custom { cursor, edit },
                            name,
                        }
                    } else {
                        BleGroupHold::Commit(name)
                    }
                }
                BleGroupCustomRow::Back => BleGroupHold::Stay {
                    screen: BleGroupScreen::Choice {
                        cursor: choice_cursor_for(if name.len == 0 {
                            BleGroupName::reticulum()
                        } else {
                            name
                        }),
                    },
                    name: if name.len == 0 {
                        BleGroupName::reticulum()
                    } else {
                        name
                    },
                },
            },
            BleGroupEdit::Char { index } => {
                if index < (name.len as usize) {
                    BleGroupHold::Stay {
                        screen: BleGroupScreen::Custom {
                            cursor,
                            edit: BleGroupEdit::Char { index: index + 1 },
                        },
                        name,
                    }
                } else {
                    BleGroupHold::Stay {
                        screen: BleGroupScreen::Custom {
                            cursor,
                            edit: BleGroupEdit::Browsing,
                        },
                        name,
                    }
                }
            }
        },
    }
}
