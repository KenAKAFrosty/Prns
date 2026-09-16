use crate::identity::IdentityHash;
use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

use super::{RemoteControlMessageWriteError, RemoteControlRequestParseError};

const FIRST_PAGE_TAG: u8 = 0x00;
const AFTER_CURSOR_TAG: u8 = 0x01;
const COMPLETE_TAG: u8 = 0x00;
const MORE_TAG: u8 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlInterfaceCursor(InterfaceId);

impl RemoteControlInterfaceCursor {
    #[must_use]
    pub const fn after(id: InterfaceId) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn id(self) -> InterfaceId {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlInterfacePage {
    First,
    After(RemoteControlInterfaceCursor),
}

impl RemoteControlInterfacePage {
    pub(crate) const MAX_ENCODED_LEN: usize = 1usize.saturating_add(INTERFACE_ID_LEN);

    pub(crate) const fn encoded_len(self) -> usize {
        match self {
            Self::First => 1,
            Self::After(_) => Self::MAX_ENCODED_LEN,
        }
    }

    pub(crate) fn write_into(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        write_interface_page(self, out)
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, RemoteControlRequestParseError> {
        parse_interface_page(bytes).map(|cursor| match cursor {
            ParsedInterfacePage::First => Self::First,
            ParsedInterfacePage::After(id) => Self::After(RemoteControlInterfaceCursor::after(id)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlInterfaceContinuation {
    Complete,
    More(RemoteControlInterfaceCursor),
}

impl RemoteControlInterfaceContinuation {
    pub(crate) const MAX_ENCODED_LEN: usize = 1usize.saturating_add(INTERFACE_ID_LEN);

    pub(crate) const fn encoded_len(self) -> usize {
        match self {
            Self::Complete => 1,
            Self::More(_) => Self::MAX_ENCODED_LEN,
        }
    }

    pub(crate) fn write_into(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        match self {
            Self::Complete => write_complete(out),
            Self::More(cursor) => write_interface_cursor(MORE_TAG, cursor.id(), out),
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [COMPLETE_TAG] => Some(Self::Complete),
            [MORE_TAG, rest @ ..] if rest.len() == INTERFACE_ID_LEN => {
                let mut id = [0u8; INTERFACE_ID_LEN];
                id.copy_from_slice(rest);
                Some(Self::More(RemoteControlInterfaceCursor::after(
                    InterfaceId::new(id),
                )))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPeerCursor(InterfaceId);

impl RemoteControlPeerCursor {
    #[must_use]
    pub const fn after(id: InterfaceId) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn id(self) -> InterfaceId {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPeerPage {
    First,
    After(RemoteControlPeerCursor),
}

impl RemoteControlPeerPage {
    pub(crate) const MAX_ENCODED_LEN: usize = 1usize.saturating_add(INTERFACE_ID_LEN);

    pub(crate) const fn encoded_len(self) -> usize {
        match self {
            Self::First => 1,
            Self::After(_) => Self::MAX_ENCODED_LEN,
        }
    }

    pub(crate) fn write_into(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        match self {
            Self::First => write_first(out),
            Self::After(cursor) => write_interface_cursor(AFTER_CURSOR_TAG, cursor.id(), out),
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, RemoteControlRequestParseError> {
        parse_interface_page(bytes).map(|cursor| match cursor {
            ParsedInterfacePage::First => Self::First,
            ParsedInterfacePage::After(id) => Self::After(RemoteControlPeerCursor::after(id)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPeerContinuation {
    Complete,
    More(RemoteControlPeerCursor),
}

impl RemoteControlPeerContinuation {
    pub(crate) const MAX_ENCODED_LEN: usize = 1usize.saturating_add(INTERFACE_ID_LEN);

    pub(crate) const fn encoded_len(self) -> usize {
        match self {
            Self::Complete => 1,
            Self::More(_) => Self::MAX_ENCODED_LEN,
        }
    }

    pub(crate) fn write_into(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        match self {
            Self::Complete => write_complete(out),
            Self::More(cursor) => write_interface_cursor(MORE_TAG, cursor.id(), out),
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [COMPLETE_TAG] => Some(Self::Complete),
            [MORE_TAG, rest @ ..] if rest.len() == INTERFACE_ID_LEN => {
                let mut id = [0u8; INTERFACE_ID_LEN];
                id.copy_from_slice(rest);
                Some(Self::More(RemoteControlPeerCursor::after(
                    InterfaceId::new(id),
                )))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlControllerCursor(IdentityHash);

impl RemoteControlControllerCursor {
    #[must_use]
    pub const fn after(identity: IdentityHash) -> Self {
        Self(identity)
    }

    #[must_use]
    pub const fn identity(self) -> IdentityHash {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerPage {
    First,
    After(RemoteControlControllerCursor),
}

impl RemoteControlControllerPage {
    pub(crate) const MAX_ENCODED_LEN: usize = 1usize.saturating_add(TRUNCATED_HASH_BYTE_LEN);

    pub(crate) const fn encoded_len(self) -> usize {
        match self {
            Self::First => 1,
            Self::After(_) => Self::MAX_ENCODED_LEN,
        }
    }

    pub(crate) fn write_into(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        match self {
            Self::First => write_first(out),
            Self::After(cursor) => {
                let Some(target) = out.get_mut(..Self::MAX_ENCODED_LEN) else {
                    return Err(RemoteControlMessageWriteError::BufferTooShort);
                };
                let Some((tag, identity)) = target.split_first_mut() else {
                    return Err(RemoteControlMessageWriteError::BufferTooShort);
                };
                *tag = AFTER_CURSOR_TAG;
                identity.copy_from_slice(cursor.identity().as_bytes());
                Ok(())
            }
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, RemoteControlRequestParseError> {
        match bytes {
            [FIRST_PAGE_TAG] => Ok(Self::First),
            [AFTER_CURSOR_TAG, rest @ ..] if rest.len() == TRUNCATED_HASH_BYTE_LEN => {
                let mut identity = [0u8; TRUNCATED_HASH_BYTE_LEN];
                identity.copy_from_slice(rest);
                Ok(Self::After(RemoteControlControllerCursor::after(
                    IdentityHash::new(identity),
                )))
            }
            [] => Err(RemoteControlRequestParseError::Truncated),
            _ => Err(RemoteControlRequestParseError::Malformed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerContinuation {
    Complete,
    More(RemoteControlControllerCursor),
}

impl RemoteControlControllerContinuation {
    pub(crate) const MAX_ENCODED_LEN: usize = 1usize.saturating_add(TRUNCATED_HASH_BYTE_LEN);

    pub(crate) const fn encoded_len(self) -> usize {
        match self {
            Self::Complete => 1,
            Self::More(_) => Self::MAX_ENCODED_LEN,
        }
    }

    pub(crate) fn write_into(self, out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        match self {
            Self::Complete => write_complete(out),
            Self::More(cursor) => {
                let Some(target) = out.get_mut(..Self::MAX_ENCODED_LEN) else {
                    return Err(RemoteControlMessageWriteError::BufferTooShort);
                };
                let Some((tag, identity)) = target.split_first_mut() else {
                    return Err(RemoteControlMessageWriteError::BufferTooShort);
                };
                *tag = MORE_TAG;
                identity.copy_from_slice(cursor.identity().as_bytes());
                Ok(())
            }
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [COMPLETE_TAG] => Some(Self::Complete),
            [MORE_TAG, rest @ ..] if rest.len() == TRUNCATED_HASH_BYTE_LEN => {
                let mut identity = [0u8; TRUNCATED_HASH_BYTE_LEN];
                identity.copy_from_slice(rest);
                Some(Self::More(RemoteControlControllerCursor::after(
                    IdentityHash::new(identity),
                )))
            }
            _ => None,
        }
    }
}

enum ParsedInterfacePage {
    First,
    After(InterfaceId),
}

fn parse_interface_page(
    bytes: &[u8],
) -> Result<ParsedInterfacePage, RemoteControlRequestParseError> {
    match bytes {
        [FIRST_PAGE_TAG] => Ok(ParsedInterfacePage::First),
        [AFTER_CURSOR_TAG, rest @ ..] if rest.len() == INTERFACE_ID_LEN => {
            let mut id = [0u8; INTERFACE_ID_LEN];
            id.copy_from_slice(rest);
            Ok(ParsedInterfacePage::After(InterfaceId::new(id)))
        }
        [] => Err(RemoteControlRequestParseError::Truncated),
        _ => Err(RemoteControlRequestParseError::Malformed),
    }
}

fn write_interface_page(
    page: RemoteControlInterfacePage,
    out: &mut [u8],
) -> Result<(), RemoteControlMessageWriteError> {
    match page {
        RemoteControlInterfacePage::First => write_first(out),
        RemoteControlInterfacePage::After(cursor) => {
            write_interface_cursor(AFTER_CURSOR_TAG, cursor.id(), out)
        }
    }
}

fn write_first(out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
    let [tag] = out else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *tag = FIRST_PAGE_TAG;
    Ok(())
}

fn write_complete(out: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
    let [tag] = out else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *tag = COMPLETE_TAG;
    Ok(())
}

fn write_interface_cursor(
    tag_value: u8,
    id: InterfaceId,
    out: &mut [u8],
) -> Result<(), RemoteControlMessageWriteError> {
    let expected = 1usize.saturating_add(INTERFACE_ID_LEN);
    if out.len() != expected {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    }
    let Some(target) = out.get_mut(..expected) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    let Some((tag, id_out)) = target.split_first_mut() else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    *tag = tag_value;
    id_out.copy_from_slice(id.as_bytes());
    Ok(())
}
