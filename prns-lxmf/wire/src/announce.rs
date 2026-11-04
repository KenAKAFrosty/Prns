//! Allocation-free LXMF delivery announce codec.

use core::str;

use crate::msgpack::Scanner;
use crate::{MessagePackKind, MessagePackValue, WireLimits};

/// Reticulum application name for an LXMF delivery destination.
pub const LXMF_APP_NAME: &str = "lxmf";
/// Reticulum aspects for an LXMF delivery destination.
pub const LXMF_DELIVERY_ASPECTS: &[&str] = &["delivery"];
/// `sha256("lxmf.delivery")[..10]`.
pub const LXMF_DELIVERY_DOTTED_NAME_HASH: [u8; 10] =
    [0x6e, 0xc6, 0x0b, 0xc3, 0x18, 0xe2, 0xc0, 0xf0, 0xd9, 0x08];

/// Structured LXMF delivery announce representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceFormat {
    /// Historical `[display_name, stamp_cost]` representation.
    LegacyTwoItem,
    /// Current `[display_name, stamp_cost, supported_functionality]` representation.
    CurrentThreeItem,
}

/// Borrowed structured LXMF delivery announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LxmfAnnounce<'a> {
    /// UTF-8 display name bytes, if advertised.
    pub display_name: Option<&'a [u8]>,
    /// Remote proof-of-work requirement, if advertised.
    pub required_stamp_cost: Option<u64>,
    /// Exact current-format supported-functionality array.
    pub supported_functionality: Option<MessagePackValue<'a>>,
    /// Which compatible structured representation was received.
    pub format: AnnounceFormat,
}

/// Why announce app-data is not a supported structured LXMF representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceError {
    /// App-data is not an exact two- or three-item MessagePack array.
    UnsupportedShape,
    /// A declared value is truncated or app-data has trailing bytes.
    Malformed,
    /// The display name is neither nil nor valid UTF-8 MessagePack binary data.
    InvalidDisplayName,
    /// The display name exceeds the caller's bound.
    DisplayNameTooLong { actual: usize, maximum: usize },
    /// Stamp cost is neither nil nor a non-negative MessagePack integer.
    InvalidStampCost,
    /// The current third item is not an array.
    InvalidSupportedFunctionality,
    /// Caller-owned output cannot hold the current announce.
    OutputTooSmall { required: usize, available: usize },
}

/// Apply Python LXMF's display-name projection without allocating.
///
/// Python decodes UTF-8, removes every NUL character, then strips leading and
/// trailing Unicode whitespace. The returned string borrows caller storage.
pub fn normalize_lxmf_display_name<'a>(
    display_name: &[u8],
    output: &'a mut [u8],
) -> Result<&'a str, AnnounceError> {
    let decoded = str::from_utf8(display_name).map_err(|_| AnnounceError::InvalidDisplayName)?;
    let required = decoded
        .chars()
        .filter(|character| *character != '\0')
        .map(char::len_utf8)
        .sum::<usize>();
    if output.len() < required {
        return Err(AnnounceError::OutputTooSmall {
            required,
            available: output.len(),
        });
    }
    let mut cursor = 0;
    for character in decoded.chars().filter(|character| *character != '\0') {
        cursor += character.encode_utf8(&mut output[cursor..]).len();
    }
    str::from_utf8(&output[..cursor])
        .map(str::trim)
        .map_err(|_| AnnounceError::InvalidDisplayName)
}

/// Parse only the legacy two-item and current three-item structured forms.
pub fn parse_lxmf_announce(
    raw: &[u8],
    maximum_display_name_bytes: usize,
) -> Result<LxmfAnnounce<'_>, AnnounceError> {
    let (count, mut cursor) = array_header(raw)?;
    let format = match count {
        2 => AnnounceFormat::LegacyTwoItem,
        3 => AnnounceFormat::CurrentThreeItem,
        _ => return Err(AnnounceError::UnsupportedShape),
    };
    let limits = WireLimits::new(
        raw.len(),
        raw.len(),
        64,
        128,
        raw.len().saturating_mul(16).max(32),
        8,
    );
    let mut scanner = Scanner::new(raw, limits);
    scanner
        .note_container(0, 1, count)
        .map_err(|_| AnnounceError::Malformed)?;

    let display = scanner
        .scan_value(cursor, 2)
        .map_err(|_| AnnounceError::Malformed)?;
    let display_raw = raw
        .get(cursor..display.end)
        .ok_or(AnnounceError::Malformed)?;
    let display_name = match display.kind {
        MessagePackKind::Nil => None,
        MessagePackKind::Binary => {
            let value = decode_binary(display_raw).ok_or(AnnounceError::InvalidDisplayName)?;
            if value.len() > maximum_display_name_bytes {
                return Err(AnnounceError::DisplayNameTooLong {
                    actual: value.len(),
                    maximum: maximum_display_name_bytes,
                });
            }
            str::from_utf8(value).map_err(|_| AnnounceError::InvalidDisplayName)?;
            Some(value)
        }
        _ => return Err(AnnounceError::InvalidDisplayName),
    };
    cursor = display.end;

    let stamp = scanner
        .scan_value(cursor, 2)
        .map_err(|_| AnnounceError::Malformed)?;
    let stamp_raw = raw.get(cursor..stamp.end).ok_or(AnnounceError::Malformed)?;
    let required_stamp_cost = match stamp.kind {
        MessagePackKind::Nil => None,
        MessagePackKind::Integer => {
            Some(decode_nonnegative_integer(stamp_raw).ok_or(AnnounceError::InvalidStampCost)?)
        }
        _ => return Err(AnnounceError::InvalidStampCost),
    };
    cursor = stamp.end;

    let supported_functionality = if count == 3 {
        let supported = scanner
            .scan_value(cursor, 2)
            .map_err(|_| AnnounceError::Malformed)?;
        if supported.kind != MessagePackKind::Array {
            return Err(AnnounceError::InvalidSupportedFunctionality);
        }
        let supported_raw = raw
            .get(cursor..supported.end)
            .ok_or(AnnounceError::Malformed)?;
        cursor = supported.end;
        Some(MessagePackValue::from_scanned(
            supported_raw,
            supported.kind,
            supported.canonicality,
        ))
    } else {
        None
    };
    if cursor != raw.len() {
        return Err(AnnounceError::Malformed);
    }
    Ok(LxmfAnnounce {
        display_name,
        required_stamp_cost,
        supported_functionality,
        format,
    })
}

/// Encode the current direct-only form `[display_name, nil, []]`.
pub fn encode_current_lxmf_announce(
    display_name: &[u8],
    output: &mut [u8],
) -> Result<usize, AnnounceError> {
    str::from_utf8(display_name).map_err(|_| AnnounceError::InvalidDisplayName)?;
    let header = binary_header_len(display_name.len()).ok_or(AnnounceError::InvalidDisplayName)?;
    let required = 1usize
        .checked_add(header)
        .and_then(|value| value.checked_add(display_name.len()))
        .and_then(|value| value.checked_add(2))
        .ok_or(AnnounceError::InvalidDisplayName)?;
    if output.len() < required {
        return Err(AnnounceError::OutputTooSmall {
            required,
            available: output.len(),
        });
    }
    output[0] = 0x93;
    let mut cursor = 1;
    write_binary_header(display_name.len(), output, &mut cursor);
    output[cursor..cursor + display_name.len()].copy_from_slice(display_name);
    cursor += display_name.len();
    output[cursor] = 0xc0;
    output[cursor + 1] = 0x90;
    Ok(cursor + 2)
}

fn array_header(raw: &[u8]) -> Result<(usize, usize), AnnounceError> {
    match raw.first().copied() {
        Some(marker @ 0x90..=0x9f) => Ok((usize::from(marker & 0x0f), 1)),
        Some(0xdc) => {
            let [_, first, second, ..] = raw else {
                return Err(AnnounceError::Malformed);
            };
            Ok((usize::from(u16::from_be_bytes([*first, *second])), 3))
        }
        _ => Err(AnnounceError::UnsupportedShape),
    }
}

fn decode_binary(raw: &[u8]) -> Option<&[u8]> {
    let (header, length): (usize, usize) = match raw.first().copied()? {
        0xc4 if raw.len() >= 2 => (2, usize::from(raw[1])),
        0xc5 if raw.len() >= 3 => (3, usize::from(u16::from_be_bytes([raw[1], raw[2]]))),
        0xc6 if raw.len() >= 5 => (
            5,
            usize::try_from(u32::from_be_bytes([raw[1], raw[2], raw[3], raw[4]])).ok()?,
        ),
        _ => return None,
    };
    raw.get(header..header.checked_add(length)?)
        .filter(|_| raw.len() == header + length)
}

fn decode_nonnegative_integer(raw: &[u8]) -> Option<u64> {
    match raw {
        [value @ 0x00..=0x7f] => Some(u64::from(*value)),
        [0xcc, value] => Some(u64::from(*value)),
        [0xcd, a, b] => Some(u64::from(u16::from_be_bytes([*a, *b]))),
        [0xce, a, b, c, d] => Some(u64::from(u32::from_be_bytes([*a, *b, *c, *d]))),
        [0xcf, a, b, c, d, e, f, g, h] => {
            Some(u64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h]))
        }
        _ => None,
    }
}

fn binary_header_len(length: usize) -> Option<usize> {
    if length <= u8::MAX as usize {
        Some(2)
    } else if length <= u16::MAX as usize {
        Some(3)
    } else if u32::try_from(length).is_ok() {
        Some(5)
    } else {
        None
    }
}

fn write_binary_header(length: usize, output: &mut [u8], cursor: &mut usize) {
    if length <= u8::MAX as usize {
        output[*cursor] = 0xc4;
        output[*cursor + 1] = length as u8;
        *cursor += 2;
    } else if length <= u16::MAX as usize {
        output[*cursor] = 0xc5;
        output[*cursor + 1..*cursor + 3].copy_from_slice(&(length as u16).to_be_bytes());
        *cursor += 3;
    } else {
        output[*cursor] = 0xc6;
        output[*cursor + 1..*cursor + 5].copy_from_slice(&(length as u32).to_be_bytes());
        *cursor += 5;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_display_name_like_python() {
        let mut output = [0u8; 32];
        let normalized = normalize_lxmf_display_name(b" \0 Python \0 peer \t", &mut output)
            .expect("valid UTF-8 display name");
        assert_eq!(normalized, "Python  peer");
    }

    #[test]
    fn emits_only_current_direct_only_form() {
        let mut output = [0u8; 64];
        let len = encode_current_lxmf_announce(b"prns", &mut output).expect("encode");
        assert_eq!(&output[..len], b"\x93\xc4\x04prns\xc0\x90");
        let parsed = parse_lxmf_announce(&output[..len], 255).expect("current fixture");
        assert_eq!(parsed.format, AnnounceFormat::CurrentThreeItem);
        assert_eq!(parsed.display_name, Some(&b"prns"[..]));
        assert_eq!(parsed.required_stamp_cost, None);
        assert_eq!(
            parsed.supported_functionality.expect("functionality").raw(),
            b"\x90"
        );
    }

    #[test]
    fn retains_a_remote_stamp_requirement_but_rejects_private_shapes() {
        let parsed = parse_lxmf_announce(b"\x93\xc4\x04peer\x0c\x90", 255).expect("stamped peer");
        assert_eq!(parsed.required_stamp_cost, Some(12));
        assert_eq!(
            parse_lxmf_announce(b"Personal Hopspot (Desktop)", 255),
            Err(AnnounceError::UnsupportedShape)
        );
        assert_eq!(
            parse_lxmf_announce(b"\x91\xc4\x04peer", 255),
            Err(AnnounceError::UnsupportedShape)
        );
        assert_eq!(
            parse_lxmf_announce(b"\x94\xc4\x04peer\xc0\x90\xc0", 255),
            Err(AnnounceError::UnsupportedShape)
        );
        assert_eq!(
            parse_lxmf_announce(b"\xdc", 255),
            Err(AnnounceError::Malformed)
        );
        assert_eq!(
            parse_lxmf_announce(b"\xdc\x00", 255),
            Err(AnnounceError::Malformed)
        );
    }
}
