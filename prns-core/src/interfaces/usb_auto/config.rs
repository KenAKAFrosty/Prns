//! Device-side configuration actions carried inside a [`super::protocol::Message::ConfigRequest`].
//!
//! The headless config lane (webUI / `hopspot configure`) sends a
//! `ConfigRequest` whose `action` bytes are encoded by this codec. The device
//! decodes them into a [`ConfigAction`], applies it through the same primitives
//! the e-ink screen uses, and answers with a
//! [`super::protocol::Message::ConfigResponse`]. See `T1000E_HEADLESS_CONFIG.md`.
//!
//! This is the authoritative home for the config action vocabulary: the
//! persistent profile store, the embassy config task, the CLI, and the webUI
//! all share these tags. Tag bytes are stable across releases; never reuse a
//! retired tag.

use crate::interfaces::lora::{RadioProfile, PROFILE_WIRE_LEN};
use heapless::Vec as HeaplessVec;

use super::protocol::{ConfigResult, MAX_CONFIG_DETAIL_BYTES, MAX_SNAPSHOT_BODY_BYTES};

/// One trailing byte for the [`ConfigAction`] tag.
const TAG_LEN: usize = 1;

/// Largest wire encoding of any [`ConfigAction`]: the tag plus a full
/// [`RadioProfile`]. The protocol frame limit
/// ([`super::protocol::MAX_ACTION_BYTES`]) is far larger, so every action fits.
pub const MAX_CONFIG_ACTION_BYTES: usize = TAG_LEN + PROFILE_WIRE_LEN;

/// A user-facing interface the config lane can toggle on or off.
///
/// Deliberately a small subset of the manifold `InterfaceKind` taxonomy: only
/// the radios a Hopspot screen (or the webUI that replaces it) exposes to an
/// operator. Internal transport interfaces are not toggleable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigInterface {
    Lora,
    Usb,
    Ble,
}

impl ConfigInterface {
    /// Canonical 1-byte wire code. Stable; never reuse a retired code.
    pub const fn to_wire_code(self) -> u8 {
        match self {
            Self::Lora => 1,
            Self::Usb => 2,
            Self::Ble => 3,
        }
    }

    /// Inverse of [`ConfigInterface::to_wire_code`]. Returns `None` for
    /// unknown codes (including the reserved `0`).
    pub const fn from_wire_code(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Lora),
            2 => Some(Self::Usb),
            3 => Some(Self::Ble),
            _ => None,
        }
    }
}

/// An action the device applies on behalf of a remote operator.
///
/// Settable-and-persisted: [`ConfigAction::SetLoRaProfile`] and
/// [`ConfigAction::ResetLoRaProfile`] go through the persistent profile store.
/// The rest are ephemeral (RAM-only) state changes, matching what the T-Echo
/// screen applies without writing flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigAction {
    /// Apply and persist a complete LoRa [`RadioProfile`].
    SetLoRaProfile(RadioProfile),
    /// Forget the stored profile and return to the build-time default.
    ResetLoRaProfile,
    /// Toggle a named interface on or off.
    ToggleInterface(ConfigInterface),
    /// Stop the node (drop the interface, park the radio).
    Sleep,
    /// Resume the node from sleep.
    Wake,
    /// Force an immediate announce on the live interfaces.
    Announce,
    /// Ask the device to emit a fresh [`super::protocol::Message::Snapshot`].
    RequestSnapshot,
}

impl ConfigAction {
    /// Encode the action into `out`, returning the number of bytes written.
    /// `out` must be at least [`MAX_CONFIG_ACTION_BYTES`] bytes.
    pub fn encode(self, out: &mut [u8]) -> usize {
        match self {
            Self::SetLoRaProfile(profile) => {
                out[0] = Self::SET_LORA_PROFILE_TAG;
                profile.encode(&mut out[TAG_LEN..TAG_LEN + PROFILE_WIRE_LEN]);
                TAG_LEN + PROFILE_WIRE_LEN
            }
            Self::ResetLoRaProfile => {
                out[0] = Self::RESET_LORA_PROFILE_TAG;
                TAG_LEN
            }
            Self::ToggleInterface(interface) => {
                out[0] = Self::TOGGLE_INTERFACE_TAG;
                out[1] = interface.to_wire_code();
                TAG_LEN + 1
            }
            Self::Sleep => {
                out[0] = Self::SLEEP_TAG;
                TAG_LEN
            }
            Self::Wake => {
                out[0] = Self::WAKE_TAG;
                TAG_LEN
            }
            Self::Announce => {
                out[0] = Self::ANNOUNCE_TAG;
                TAG_LEN
            }
            Self::RequestSnapshot => {
                out[0] = Self::REQUEST_SNAPSHOT_TAG;
                TAG_LEN
            }
        }
    }

    /// Decode the action. Returns `None` for unknown tags, a truncated
    /// profile, or a profile that fails [`RadioProfile::validate`].
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let tag = *bytes.first()?;
        match tag {
            Self::SET_LORA_PROFILE_TAG => {
                let profile = RadioProfile::decode(&bytes[TAG_LEN..])?;
                Some(Self::SetLoRaProfile(profile))
            }
            Self::RESET_LORA_PROFILE_TAG => Some(Self::ResetLoRaProfile),
            Self::TOGGLE_INTERFACE_TAG => {
                let interface = ConfigInterface::from_wire_code(*bytes.get(1)?)?;
                Some(Self::ToggleInterface(interface))
            }
            Self::SLEEP_TAG => Some(Self::Sleep),
            Self::WAKE_TAG => Some(Self::Wake),
            Self::ANNOUNCE_TAG => Some(Self::Announce),
            Self::REQUEST_SNAPSHOT_TAG => Some(Self::RequestSnapshot),
            _ => None,
        }
    }

    const SET_LORA_PROFILE_TAG: u8 = 0x01;
    const RESET_LORA_PROFILE_TAG: u8 = 0x02;
    const TOGGLE_INTERFACE_TAG: u8 = 0x03;
    const SLEEP_TAG: u8 = 0x04;
    const WAKE_TAG: u8 = 0x05;
    const ANNOUNCE_TAG: u8 = 0x06;
    const REQUEST_SNAPSHOT_TAG: u8 = 0x07;
}

/// A config request forwarded from the USB Auto device lane to the headless
/// config task. The lane copies the action bytes (already length-checked
/// against [`MAX_CONFIG_ACTION_BYTES`]) into this owned payload so the config
/// task can decode it without borrowing the receive buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRequest {
    pub request_id: u8,
    pub action: HeaplessVec<u8, MAX_CONFIG_ACTION_BYTES>,
}

impl ConfigRequest {
    /// Build a request from a borrowed action slice. Returns `None` only if the
    /// action is longer than [`MAX_CONFIG_ACTION_BYTES`]; callers that have
    /// already length-checked the slice always get `Some`.
    pub fn from_action(request_id: u8, action: &[u8]) -> Option<Self> {
        let mut buf = HeaplessVec::new();
        buf.extend_from_slice(action).ok()?;
        Some(Self {
            request_id,
            action: buf,
        })
    }
}

/// Wire schema version stamped into every [`ConfigReply::Snapshot`]. Bump
/// when the snapshot body layout changes; the host rejects unknown versions.
/// See `T1000E_HEADLESS_CONFIG.md` (task #4 owns the body layout).
pub const SNAPSHOT_SCHEMA_VERSION: u16 = 1;

/// The config task's reply to a [`ConfigRequest`]. The device lane turns the
/// reply into a `Message::ConfigResponse` or `Message::Snapshot` on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigReply {
    /// Answer a config action.
    Response {
        request_id: u8,
        result: ConfigResult,
        detail: HeaplessVec<u8, MAX_CONFIG_DETAIL_BYTES>,
    },
    /// A fresh snapshot, emitted in answer to
    /// [`ConfigAction::RequestSnapshot`] (and proactively on connect). The
    /// config task stamps the [`SNAPSHOT_SCHEMA_VERSION`] it encoded the body
    /// against.
    Snapshot {
        schema_version: u16,
        body: HeaplessVec<u8, MAX_SNAPSHOT_BODY_BYTES>,
    },
}

impl ConfigReply {
    /// Build a bare `Response` reply with no detail.
    pub fn response(request_id: u8, result: ConfigResult) -> Self {
        Self::Response {
            request_id,
            result,
            detail: HeaplessVec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::lora::{ModemPreset, PreambleSymbols, Region, TxPower};

    // The protocol frame limit must always accommodate the largest action.
    const _: () = assert!(super::super::protocol::MAX_ACTION_BYTES >= MAX_CONFIG_ACTION_BYTES);

    fn sample_profile() -> RadioProfile {
        RadioProfile {
            frequency: Region::Eu868.default_frequency(),
            modulation: ModemPreset::LongSlow.modulation(),
            tx_power: TxPower::new(14),
            preamble: PreambleSymbols::new(32),
            region: Region::Eu868,
        }
    }

    #[test]
    fn every_action_round_trips() {
        let mut buf = [0u8; MAX_CONFIG_ACTION_BYTES];
        let actions = [
            ConfigAction::SetLoRaProfile(sample_profile()),
            ConfigAction::ResetLoRaProfile,
            ConfigAction::ToggleInterface(ConfigInterface::Lora),
            ConfigAction::ToggleInterface(ConfigInterface::Usb),
            ConfigAction::ToggleInterface(ConfigInterface::Ble),
            ConfigAction::Sleep,
            ConfigAction::Wake,
            ConfigAction::Announce,
            ConfigAction::RequestSnapshot,
        ];
        for action in actions {
            let n = action.encode(&mut buf);
            assert!(n <= MAX_CONFIG_ACTION_BYTES);
            assert_eq!(ConfigAction::decode(&buf[..n]), Some(action));
        }
    }

    #[test]
    fn set_lora_profile_writes_the_full_profile_after_the_tag() {
        let mut buf = [0u8; MAX_CONFIG_ACTION_BYTES];
        let n = ConfigAction::SetLoRaProfile(sample_profile()).encode(&mut buf);
        assert_eq!(n, TAG_LEN + PROFILE_WIRE_LEN);
        assert_eq!(buf[0], 0x01);
        assert_eq!(
            RadioProfile::decode(&buf[TAG_LEN..n]),
            Some(sample_profile())
        );
    }

    #[test]
    fn toggle_interface_round_trips_each_code() {
        let mut buf = [0u8; MAX_CONFIG_ACTION_BYTES];
        for interface in [
            ConfigInterface::Lora,
            ConfigInterface::Usb,
            ConfigInterface::Ble,
        ] {
            let n = ConfigAction::ToggleInterface(interface).encode(&mut buf);
            assert_eq!(n, 2);
            assert_eq!(
                ConfigAction::decode(&buf[..n]),
                Some(ConfigAction::ToggleInterface(interface))
            );
        }
    }

    #[test]
    fn decode_rejects_unknown_and_truncated_actions() {
        let mut buf = [0u8; MAX_CONFIG_ACTION_BYTES];
        // Unknown tag.
        buf[0] = 0xFF;
        assert_eq!(ConfigAction::decode(&buf[..1]), None);
        // SetLoRaProfile with no profile body.
        buf[0] = 0x01;
        assert_eq!(ConfigAction::decode(&buf[..1]), None);
        // ToggleInterface with no interface byte.
        buf[0] = 0x03;
        assert_eq!(ConfigAction::decode(&buf[..1]), None);
        // ToggleInterface with an unknown interface code.
        buf[0] = 0x03;
        buf[1] = 9;
        assert_eq!(ConfigAction::decode(&buf[..2]), None);
        // Empty input.
        assert_eq!(ConfigAction::decode(&[]), None);
    }

    #[test]
    fn decode_rejects_an_invalid_profile() {
        let mut buf = [0u8; MAX_CONFIG_ACTION_BYTES];
        let mut profile = sample_profile();
        // Frequency outside the EU868 band.
        profile.frequency = crate::interfaces::lora::Frequency::new(100_000_000);
        // A profile that fails validate still encodes (encode does not
        // validate), but decode must refuse it.
        profile.encode(&mut buf);
        let mut action_buf = [0u8; MAX_CONFIG_ACTION_BYTES];
        action_buf[0] = 0x01;
        action_buf[TAG_LEN..TAG_LEN + PROFILE_WIRE_LEN].copy_from_slice(&buf[..PROFILE_WIRE_LEN]);
        assert_eq!(
            ConfigAction::decode(&action_buf[..TAG_LEN + PROFILE_WIRE_LEN]),
            None
        );
    }

    #[test]
    fn config_interface_codes_are_stable_and_distinct() {
        let codes = [
            ConfigInterface::Lora.to_wire_code(),
            ConfigInterface::Usb.to_wire_code(),
            ConfigInterface::Ble.to_wire_code(),
        ];
        assert_eq!(codes, [1, 2, 3]);
        for code in codes {
            assert!(ConfigInterface::from_wire_code(code).is_some());
        }
        assert_eq!(ConfigInterface::from_wire_code(0), None);
        assert_eq!(ConfigInterface::from_wire_code(4), None);
    }

    #[test]
    fn config_request_from_action_round_trips() {
        let mut action = [0u8; MAX_CONFIG_ACTION_BYTES];
        let n = ConfigAction::ResetLoRaProfile.encode(&mut action);
        let request = ConfigRequest::from_action(0x42, &action[..n]).expect("fits");
        assert_eq!(request.request_id, 0x42);
        assert_eq!(request.action.as_slice(), &action[..n]);
        assert_eq!(
            ConfigAction::decode(&request.action),
            Some(ConfigAction::ResetLoRaProfile)
        );
    }

    #[test]
    fn config_request_from_action_rejects_oversize() {
        let too_long = [0u8; MAX_CONFIG_ACTION_BYTES + 1];
        assert_eq!(ConfigRequest::from_action(0, &too_long), None);
    }

    #[test]
    fn config_reply_response_helper_is_empty() {
        let reply = ConfigReply::response(0x11, ConfigResult::Ok);
        let ConfigReply::Response {
            request_id,
            result,
            detail,
        } = reply
        else {
            panic!("expected Response");
        };
        assert_eq!(request_id, 0x11);
        assert_eq!(result, ConfigResult::Ok);
        assert!(detail.is_empty());
    }
}
