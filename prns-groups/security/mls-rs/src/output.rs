use crate::{GroupId, Limit, Limits, MemberBinding};
use alloc::{vec, vec::Vec};
use core::fmt;
use zeroize::{Zeroize, Zeroizing};

#[derive(PartialEq, Eq)]
pub enum Event {
    Created,
    KeyPackage,
    Joined,
    Sent,
    Control,
    Proposal,
    Removed,
    Retired,
    Application(Vec<u8>),
    Rejected(u16),
}
/// Operation result, returned to the caller only after definite commit. A
/// composition callback borrows it provisionally before publication. Application
/// bytes and rejection evidence become durable with consumed receive keys.
#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub messages: Vec<Vec<u8>>,
    pub event: Event,
    pub sender: Option<MemberBinding>,
    pub context: Option<OutputContext>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputContext {
    pub group: GroupId,
    pub epoch: u64,
}
impl Output {
    pub(crate) fn empty(event: Event) -> Self {
        Self {
            messages: Vec::new(),
            event,
            sender: None,
            context: None,
        }
    }
    pub(crate) fn one(event: Event, bytes: Vec<u8>) -> Self {
        Self {
            messages: vec![bytes],
            event,
            sender: None,
            context: None,
        }
    }
    /// Exact encoded retention size of an owner-authored Control output with
    /// one opaque MLS message and its group/epoch context. This does not allocate
    /// the message and excludes the backend key/receipt or application facts.
    pub fn control_retention_bytes(message_bytes: usize) -> Result<usize, Limit> {
        u32::try_from(message_bytes).map_err(|_| Limit::Configuration)?;
        encoded_len(0, false, true, core::iter::once(message_bytes))
    }
    pub(crate) fn encode(&self, limits: Limits) -> Result<Zeroizing<Vec<u8>>, Limit> {
        if self.messages.len() > limits.max_members + 1 {
            return Err(Limit::Members);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        let (tag, body): (u8, &[u8]) = match &self.event {
            Event::Created => (0, &[]),
            Event::KeyPackage => (1, &[]),
            Event::Joined => (2, &[]),
            Event::Sent => (3, &[]),
            Event::Control => (4, &[]),
            Event::Proposal => (5, &[]),
            Event::Removed => (6, &[]),
            Event::Application(body) => (7, body),
            Event::Rejected(_) => (8, &[]),
            Event::Retired => (9, &[]),
        };
        if body.len() > limits.max_plaintext_bytes {
            return Err(Limit::PlaintextBytes);
        }
        for message in &self.messages {
            if message.len() > limits.max_wire_bytes {
                return Err(Limit::WireBytes);
            }
        }
        let total = encoded_len(
            body.len(),
            self.sender.is_some(),
            self.context.is_some(),
            self.messages.iter().map(Vec::len),
        )?;
        bytes
            .try_reserve_exact(total)
            .map_err(|_| Limit::Allocation)?;
        bytes.push(tag);
        match self.context {
            Some(context) => {
                bytes.push(1);
                bytes.extend_from_slice(&context.group.0);
                bytes.extend_from_slice(&context.epoch.to_be_bytes());
            }
            None => bytes.push(0),
        }
        match &self.sender {
            Some(sender) => {
                bytes.push(1);
                bytes.extend_from_slice(&sender.endpoint);
                bytes.extend_from_slice(&sender.incarnation.to_be_bytes());
                bytes.extend_from_slice(&sender.signing_key);
            }
            None => bytes.push(0),
        }
        let reason = match self.event {
            Event::Rejected(code) => code,
            _ => 0,
        };
        bytes.extend_from_slice(&reason.to_be_bytes());
        append(&mut bytes, body)?;
        bytes.extend_from_slice(&(self.messages.len() as u32).to_be_bytes());
        for message in &self.messages {
            if message.len() > limits.max_wire_bytes {
                return Err(Limit::WireBytes);
            }
            append(&mut bytes, message)?;
        }
        Ok(bytes)
    }
    pub(crate) fn decode(bytes: &[u8], limits: Limits) -> Result<Self, ()> {
        let mut cursor = bytes;
        let tag = take(&mut cursor, 1)?[0];
        let context = match take(&mut cursor, 1)?[0] {
            0 => None,
            1 => Some(OutputContext {
                group: GroupId(take(&mut cursor, 32)?.try_into().map_err(|_| ())?),
                epoch: u64::from_be_bytes(take(&mut cursor, 8)?.try_into().map_err(|_| ())?),
            }),
            _ => return Err(()),
        };
        let sender = match take(&mut cursor, 1)?[0] {
            0 => None,
            1 => Some(MemberBinding {
                endpoint: take(&mut cursor, 32)?.try_into().map_err(|_| ())?,
                incarnation: u64::from_be_bytes(take(&mut cursor, 8)?.try_into().map_err(|_| ())?),
                signing_key: take(&mut cursor, 32)?.try_into().map_err(|_| ())?,
            }),
            _ => return Err(()),
        };
        if sender.is_some() != matches!(tag, 7 | 8) {
            return Err(());
        }
        let code = u16::from_be_bytes(take(&mut cursor, 2)?.try_into().map_err(|_| ())?);
        let body = field(&mut cursor, limits.max_plaintext_bytes)?;
        let count = u32::from_be_bytes(take(&mut cursor, 4)?.try_into().map_err(|_| ())?) as usize;
        if count > limits.max_members + 1 {
            return Err(());
        }
        let mut messages = Vec::new();
        messages.try_reserve_exact(count).map_err(|_| ())?;
        for _ in 0..count {
            messages.push(copy_bytes(field(&mut cursor, limits.max_wire_bytes)?).map_err(|_| ())?);
        }
        if !cursor.is_empty() || (tag != 7 && !body.is_empty()) || (tag != 8 && code != 0) {
            return Err(());
        }
        let event = match tag {
            0 => Event::Created,
            1 => Event::KeyPackage,
            2 => Event::Joined,
            3 => Event::Sent,
            4 => Event::Control,
            5 => Event::Proposal,
            6 => Event::Removed,
            7 => Event::Application(copy_bytes(body).map_err(|_| ())?),
            8 => Event::Rejected(code),
            9 => Event::Retired,
            _ => return Err(()),
        };
        Ok(Self {
            messages,
            event,
            sender,
            context,
        })
    }
}
fn encoded_len(
    body_bytes: usize,
    sender: bool,
    context: bool,
    messages: impl IntoIterator<Item = usize>,
) -> Result<usize, Limit> {
    let mut total = 1usize + 1 + 1 + 2 + 4 + 4;
    total = total.checked_add(body_bytes).ok_or(Limit::Allocation)?;
    if sender {
        total = total.checked_add(72).ok_or(Limit::Allocation)?;
    }
    if context {
        total = total.checked_add(40).ok_or(Limit::Allocation)?;
    }
    for message in messages {
        total = total
            .checked_add(4)
            .and_then(|n| n.checked_add(message))
            .ok_or(Limit::Allocation)?;
    }
    Ok(total)
}

fn append(bytes: &mut Vec<u8>, value: &[u8]) -> Result<(), Limit> {
    let size = u32::try_from(value.len()).map_err(|_| Limit::Configuration)?;
    bytes
        .try_reserve_exact(value.len().checked_add(4).ok_or(Limit::Allocation)?)
        .map_err(|_| Limit::Allocation)?;
    bytes.extend_from_slice(&size.to_be_bytes());
    bytes.extend_from_slice(value);
    Ok(())
}
fn take<'a>(cursor: &mut &'a [u8], size: usize) -> Result<&'a [u8], ()> {
    if size > cursor.len() {
        return Err(());
    }
    let (value, rest) = cursor.split_at(size);
    *cursor = rest;
    Ok(value)
}
fn field<'a>(cursor: &mut &'a [u8], max: usize) -> Result<&'a [u8], ()> {
    let size = u32::from_be_bytes(take(cursor, 4)?.try_into().map_err(|_| ())?) as usize;
    if size > max {
        return Err(());
    }
    take(cursor, size)
}

impl Drop for Output {
    fn drop(&mut self) {
        if let Event::Application(body) = &mut self.event {
            body.zeroize();
        }
    }
}

pub(crate) fn copy_bytes(bytes: &[u8]) -> Result<Vec<u8>, Limit> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| Limit::Allocation)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}
impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => f.write_str("Created"),
            Self::KeyPackage => f.write_str("KeyPackage"),
            Self::Joined => f.write_str("Joined"),
            Self::Sent => f.write_str("Sent"),
            Self::Control => f.write_str("Control"),
            Self::Proposal => f.write_str("Proposal"),
            Self::Removed => f.write_str("Removed"),
            Self::Retired => f.write_str("Retired"),
            Self::Application(body) => f
                .debug_struct("Application")
                .field("bytes", &body.len())
                .finish(),
            Self::Rejected(code) => f.debug_tuple("Rejected").field(code).finish(),
        }
    }
}

#[cfg(test)]
mod retention_size_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn control_retention_size_matches_actual_encoder_and_checks_overflow() {
        for length in [0, 1, 63, 1024, 8192] {
            let output = Output {
                messages: vec![vec![7; length]],
                event: Event::Control,
                sender: None,
                context: Some(OutputContext {
                    group: GroupId([8; 32]),
                    epoch: 42,
                }),
            };
            let actual = output
                .encode(Limits {
                    max_members: 16,
                    max_groups: 2,
                    max_wire_bytes: 65_536,
                    max_plaintext_bytes: 4_096,
                    max_snapshot_bytes: 65_536,
                    max_key_packages: 8,
                    max_retained_epochs: 2,
                    max_pending_proposals: 16,
                    max_pending_outputs: 64,
                })
                .unwrap();
            assert_eq!(
                actual.len(),
                Output::control_retention_bytes(length).unwrap()
            );
        }
        assert!(Output::control_retention_bytes(usize::MAX).is_err());
    }
}
