use personal_rns::interfaces::{
    DiscoveryGroupId, DiscoveryGroupSet, InterfaceId, MAX_DISCOVERY_GROUPS,
    MAX_DISCOVERY_GROUP_ID_LEN,
};

const PORTABLE_GROUP_CHARACTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-_";
const TEXT_DONE: usize = 0;
const TEXT_BACKSPACE: usize = 1;
const TEXT_CHOICE_COUNT: usize = PORTABLE_GROUP_CHARACTERS.len() + 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) struct GroupEditorName {
    bytes: [u8; MAX_DISCOVERY_GROUP_ID_LEN],
    len: u8,
}

impl GroupEditorName {
    const EMPTY: Self = Self {
        bytes: [0; MAX_DISCOVERY_GROUP_ID_LEN],
        len: 0,
    };

    fn from_id(id: &DiscoveryGroupId) -> Self {
        let mut name = Self::EMPTY;
        name.bytes[..id.as_bytes().len()].copy_from_slice(id.as_bytes());
        name.len = id.as_bytes().len() as u8;
        name
    }

    pub(in crate::screen) fn as_str(&self) -> &str {
        self.bytes
            .get(..usize::from(self.len))
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .unwrap_or("")
    }

    fn push(&mut self, byte: u8) -> bool {
        let index = usize::from(self.len);
        let Some(slot) = self.bytes.get_mut(index) else {
            return false;
        };
        *slot = byte;
        self.len = self.len.saturating_add(1);
        true
    }

    fn pop(&mut self) {
        let text = self.as_str();
        let Some((index, _)) = text.char_indices().next_back() else {
            return;
        };
        self.bytes[index..usize::from(self.len)].fill(0);
        self.len = index as u8;
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) enum GroupEditorScreen {
    List { cursor: usize },
    Item { index: usize, cursor: usize },
    Text { index: usize, choice: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::screen) struct DiscoveryGroupEditor {
    interface_id: InterfaceId,
    names: [GroupEditorName; MAX_DISCOVERY_GROUPS],
    count: u8,
    pub(in crate::screen) screen: GroupEditorScreen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiscoveryGroupReplacement {
    interface_id: InterfaceId,
    groups: DiscoveryGroupSet,
}

impl DiscoveryGroupReplacement {
    #[must_use]
    pub const fn interface_id(&self) -> InterfaceId {
        self.interface_id
    }

    pub const fn groups(&self) -> &DiscoveryGroupSet {
        &self.groups
    }

    #[must_use]
    pub fn into_groups(self) -> DiscoveryGroupSet {
        self.groups
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::screen) enum DiscoveryGroupEditorOutcome {
    Stay(DiscoveryGroupEditor),
    Commit(DiscoveryGroupReplacement),
    Invalid(DiscoveryGroupEditor),
    Cancel,
}

impl DiscoveryGroupEditor {
    pub(in crate::screen) fn new(interface_id: InterfaceId, groups: &DiscoveryGroupSet) -> Self {
        let mut names = [GroupEditorName::EMPTY; MAX_DISCOVERY_GROUPS];
        for (slot, group) in names.iter_mut().zip(groups.iter()) {
            *slot = GroupEditorName::from_id(group);
        }
        Self {
            interface_id,
            names,
            count: groups.len() as u8,
            screen: GroupEditorScreen::List { cursor: 0 },
        }
    }

    pub(in crate::screen) fn names(&self) -> &[GroupEditorName] {
        &self.names[..usize::from(self.count)]
    }

    pub(in crate::screen) const fn can_add(&self) -> bool {
        (self.count as usize) < MAX_DISCOVERY_GROUPS
    }

    pub(in crate::screen) fn text_choice(&self) -> Option<&'static str> {
        let GroupEditorScreen::Text { choice, .. } = self.screen else {
            return None;
        };
        match choice {
            TEXT_DONE => Some("Done"),
            TEXT_BACKSPACE => Some("Delete"),
            choice => {
                core::str::from_utf8(PORTABLE_GROUP_CHARACTERS.get(choice - 2..choice - 1)?).ok()
            }
        }
    }

    pub(in crate::screen) fn active_name(&self) -> Option<&str> {
        let index = match self.screen {
            GroupEditorScreen::Item { index, .. } | GroupEditorScreen::Text { index, .. } => index,
            GroupEditorScreen::List { .. } => return None,
        };
        self.names.get(index).map(GroupEditorName::as_str)
    }

    fn list_item_count(&self) -> usize {
        usize::from(self.count) + usize::from(self.can_add()) + 2
    }

    fn save_index(&self) -> usize {
        usize::from(self.count) + usize::from(self.can_add())
    }

    fn back_index(&self) -> usize {
        self.save_index() + 1
    }

    fn groups(&self) -> Option<DiscoveryGroupSet> {
        let mut groups: heapless::Vec<DiscoveryGroupId, MAX_DISCOVERY_GROUPS> =
            heapless::Vec::new();
        for name in self.names() {
            groups
                .push(DiscoveryGroupId::parse(name.as_str()).ok()?)
                .ok()?;
        }
        DiscoveryGroupSet::try_from_slice(&groups).ok()
    }
}

pub(in crate::screen) fn group_editor_tap(
    mut editor: DiscoveryGroupEditor,
) -> DiscoveryGroupEditor {
    editor.screen = match editor.screen {
        GroupEditorScreen::List { cursor } => GroupEditorScreen::List {
            cursor: (cursor + 1) % editor.list_item_count(),
        },
        GroupEditorScreen::Item { index, cursor } => GroupEditorScreen::Item {
            index,
            cursor: (cursor + 1) % 3,
        },
        GroupEditorScreen::Text { index, choice } => GroupEditorScreen::Text {
            index,
            choice: (choice + 1) % TEXT_CHOICE_COUNT,
        },
    };
    editor
}

pub(in crate::screen) fn group_editor_hold(
    mut editor: DiscoveryGroupEditor,
) -> DiscoveryGroupEditorOutcome {
    match editor.screen {
        GroupEditorScreen::List { cursor } if cursor < usize::from(editor.count) => {
            editor.screen = GroupEditorScreen::Item {
                index: cursor,
                cursor: 0,
            };
            DiscoveryGroupEditorOutcome::Stay(editor)
        }
        GroupEditorScreen::List { cursor }
            if editor.can_add() && cursor == usize::from(editor.count) =>
        {
            let index = usize::from(editor.count);
            editor.names[index] = GroupEditorName::EMPTY;
            editor.count = editor.count.saturating_add(1);
            editor.screen = GroupEditorScreen::Text { index, choice: 0 };
            DiscoveryGroupEditorOutcome::Stay(editor)
        }
        GroupEditorScreen::List { cursor } if cursor == editor.save_index() => {
            match editor.groups() {
                Some(groups) => DiscoveryGroupEditorOutcome::Commit(DiscoveryGroupReplacement {
                    interface_id: editor.interface_id,
                    groups,
                }),
                None => DiscoveryGroupEditorOutcome::Invalid(editor),
            }
        }
        GroupEditorScreen::List { cursor } if cursor == editor.back_index() => {
            DiscoveryGroupEditorOutcome::Cancel
        }
        GroupEditorScreen::List { .. } => DiscoveryGroupEditorOutcome::Stay(editor),
        GroupEditorScreen::Item { index, cursor: 0 } => {
            editor.screen = GroupEditorScreen::Text { index, choice: 0 };
            DiscoveryGroupEditorOutcome::Stay(editor)
        }
        GroupEditorScreen::Item { index, cursor: 1 } => {
            if editor.count > 1 {
                let count = usize::from(editor.count);
                editor.names.copy_within(index + 1..count, index);
                editor.names[count - 1] = GroupEditorName::EMPTY;
                editor.count -= 1;
            }
            editor.screen = GroupEditorScreen::List {
                cursor: index.min(usize::from(editor.count).saturating_sub(1)),
            };
            DiscoveryGroupEditorOutcome::Stay(editor)
        }
        GroupEditorScreen::Item { .. } => {
            editor.screen = GroupEditorScreen::List { cursor: 0 };
            DiscoveryGroupEditorOutcome::Stay(editor)
        }
        GroupEditorScreen::Text {
            index,
            choice: TEXT_DONE,
        } => {
            if editor.names[index].is_empty() {
                if editor.count == 1 {
                    return DiscoveryGroupEditorOutcome::Invalid(editor);
                }
                let count = usize::from(editor.count);
                editor.names.copy_within(index + 1..count, index);
                editor.names[count - 1] = GroupEditorName::EMPTY;
                editor.count -= 1;
            }
            editor.screen = GroupEditorScreen::List {
                cursor: index.min(usize::from(editor.count).saturating_sub(1)),
            };
            DiscoveryGroupEditorOutcome::Stay(editor)
        }
        GroupEditorScreen::Text {
            index,
            choice: TEXT_BACKSPACE,
        } => {
            editor.names[index].pop();
            DiscoveryGroupEditorOutcome::Stay(editor)
        }
        GroupEditorScreen::Text { index, choice } => {
            let _ = editor.names[index].push(PORTABLE_GROUP_CHARACTERS[choice - 2]);
            editor.screen = GroupEditorScreen::Text { index, choice: 0 };
            DiscoveryGroupEditorOutcome::Stay(editor)
        }
    }
}
