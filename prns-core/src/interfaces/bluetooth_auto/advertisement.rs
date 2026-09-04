use super::identity::{default_group_tag, BleAddress, BleIdentity, GROUP_TAG_LEN};

pub const MAX_ADVERTISEMENT_LEN: usize = 31;

const fn ble_reticulum_uuid(last: u8) -> [u8; 16] {
    [
        0x37, 0x14, 0x5b, 0x00, 0x44, 0x2d, 0x4a, 0x94, 0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda, 0x28,
        last,
    ]
}

pub const BLE_SERVICE_UUID_BYTES: [u8; 16] = ble_reticulum_uuid(0xe3);
pub const BLE_SERVICE_UUID: BleUuid = BleUuid::Bit128(BLE_SERVICE_UUID_BYTES);
pub const COLUMBA_RX_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe5));
pub const COLUMBA_TX_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe4));
pub const COLUMBA_IDENTITY_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe6));
pub const NATIVE_CONTROL_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe7));
pub const NATIVE_DATA_UUID: BleUuid = BleUuid::Bit128(ble_reticulum_uuid(0xe8));

const AD_FLAGS: u8 = 0x01;
const AD_INCOMPLETE_SERVICE_UUID128: u8 = 0x06;
const AD_SERVICE_UUID128: u8 = 0x07;
pub(super) const AD_MANUFACTURER_SPECIFIC: u8 = 0xff;
const FLAGS_LE_GENERAL_DISCOVERABLE: u8 = 0x06;
const EXPERIMENTAL_ROLE_COMPANY_ID: [u8; 2] = [0xff, 0xff];
/// Oldest manufacturer payload we still parse for role flags.
pub(super) const EXPERIMENTAL_ROLE_VERSION_MIN: u8 = 0x03;
/// Manufacturer payload version that carries a discovery group tag.
pub(super) const EXPERIMENTAL_ROLE_VERSION: u8 = 0x04;
/// Host manufacturer payload that also carries a dial-election key (first 6 identity bytes).
pub(super) const EXPERIMENTAL_ROLE_VERSION_WITH_DIAL_KEY: u8 = 0x05;
pub(super) const EXPERIMENTAL_ROLE_PERIPHERAL_ONLY: u8 = 0x01;
pub const DIAL_KEY_LEN: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleUuid {
    Bit16(u16),
    Bit128([u8; 16]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleRoleCapabilities {
    DualRole,
    PeripheralOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumbaConnectionRole {
    Dial,
    Accept,
    Unavailable,
}

/// Encode a SoftDevice-sized ADV including the Prns service UUID, role flags, and discovery group tag.
///
/// Layout fills all 31 classic ADV bytes when successful.
pub fn encode_advertisement(
    out: &mut [u8],
    role_capabilities: BleRoleCapabilities,
    group_tag: [u8; GROUP_TAG_LEN],
) -> Option<usize> {
    let mut writer = AdWriter::new(out);
    writer.put(AD_FLAGS, &[FLAGS_LE_GENERAL_DISCOVERABLE])?;
    let mut little_endian = BLE_SERVICE_UUID_BYTES;
    little_endian.reverse();
    writer.put(AD_SERVICE_UUID128, &little_endian)?;
    let flags = match role_capabilities {
        BleRoleCapabilities::DualRole => 0,
        BleRoleCapabilities::PeripheralOnly => EXPERIMENTAL_ROLE_PERIPHERAL_ONLY,
    };
    writer.put(
        AD_MANUFACTURER_SPECIFIC,
        &[
            EXPERIMENTAL_ROLE_COMPANY_ID[0],
            EXPERIMENTAL_ROLE_COMPANY_ID[1],
            EXPERIMENTAL_ROLE_VERSION,
            flags,
            group_tag[0],
            group_tag[1],
            group_tag[2],
            group_tag[3],
        ],
    )?;
    Some(writer.len())
}

pub fn contains_service(adv: &[u8]) -> bool {
    let mut little_endian = BLE_SERVICE_UUID_BYTES;
    little_endian.reverse();
    AdReader::new(adv).any(|(ad_type, body)| {
        (ad_type == AD_SERVICE_UUID128 || ad_type == AD_INCOMPLETE_SERVICE_UUID128)
            && body == little_endian
    })
}

pub fn columba_role_capabilities(adv: &[u8]) -> Option<BleRoleCapabilities> {
    AdReader::new(adv).find_map(|(ad_type, body)| {
        if ad_type != AD_MANUFACTURER_SPECIFIC {
            return None;
        }
        let company_id: [u8; 2] = body.get(..2)?.try_into().ok()?;
        columba_role_capabilities_from_manufacturer(u16::from_le_bytes(company_id), body.get(2..)?)
    })
}

pub fn columba_role_capabilities_from_manufacturer(
    company_id: u16,
    data: &[u8],
) -> Option<BleRoleCapabilities> {
    if company_id != u16::from_le_bytes(EXPERIMENTAL_ROLE_COMPANY_ID)
        || *data.first()? < EXPERIMENTAL_ROLE_VERSION_MIN
    {
        return None;
    }
    if data.get(1)? & EXPERIMENTAL_ROLE_PERIPHERAL_ONLY == 0 {
        Some(BleRoleCapabilities::DualRole)
    } else {
        Some(BleRoleCapabilities::PeripheralOnly)
    }
}

/// Discovery group tag from manufacturer data, or the default group when the peer is legacy (v3).
pub fn advertisement_group_tag(adv: &[u8]) -> [u8; GROUP_TAG_LEN] {
    AdReader::new(adv)
        .find_map(|(ad_type, body)| {
            if ad_type != AD_MANUFACTURER_SPECIFIC {
                return None;
            }
            let company_id: [u8; 2] = body.get(..2)?.try_into().ok()?;
            group_tag_from_manufacturer(u16::from_le_bytes(company_id), body.get(2..)?)
        })
        .unwrap_or_else(default_group_tag)
}

/// Group tag from a parsed manufacturer payload (`version | flags | tag…`), if present.
pub fn group_tag_from_manufacturer(company_id: u16, data: &[u8]) -> Option<[u8; GROUP_TAG_LEN]> {
    if company_id != u16::from_le_bytes(EXPERIMENTAL_ROLE_COMPANY_ID) {
        return None;
    }
    if *data.first()? < EXPERIMENTAL_ROLE_VERSION {
        return None;
    }
    data.get(2..2 + GROUP_TAG_LEN)?.try_into().ok()
}

/// Effective discovery group for a manufacturer payload (legacy/missing → default group).
pub fn manufacturer_discovery_group_tag(company_id: u16, data: &[u8]) -> [u8; GROUP_TAG_LEN] {
    group_tag_from_manufacturer(company_id, data).unwrap_or_else(default_group_tag)
}

/// True when the advertisement's discovery group matches `local_tag`.
///
/// Legacy (v3) advertisements are treated as the default group, so custom-group
/// nodes do not peer with untagged firmware.
pub fn discovery_groups_match(local_tag: [u8; GROUP_TAG_LEN], adv: &[u8]) -> bool {
    advertisement_group_tag(adv) == local_tag
}

/// True when a manufacturer payload's discovery group matches `local_tag`.
pub fn manufacturer_discovery_groups_match(
    local_tag: [u8; GROUP_TAG_LEN],
    company_id: u16,
    data: &[u8],
) -> bool {
    manufacturer_discovery_group_tag(company_id, data) == local_tag
}

/// Manufacturer-specific body for a DualRole advertisement in the local discovery group.
///
/// SoftDevice primary ADV is capped at [`MAX_ADVERTISEMENT_LEN`]; this v4 shape fits beside the
/// 128-bit service UUID. Host stacks that can carry a larger manufacturer field should prefer
/// [`manufacturer_role_payload_with_dial_key`].
pub fn manufacturer_role_payload(
    role_capabilities: BleRoleCapabilities,
    group_tag: [u8; GROUP_TAG_LEN],
) -> [u8; 2 + GROUP_TAG_LEN] {
    let flags = match role_capabilities {
        BleRoleCapabilities::DualRole => 0,
        BleRoleCapabilities::PeripheralOnly => EXPERIMENTAL_ROLE_PERIPHERAL_ONLY,
    };
    [
        EXPERIMENTAL_ROLE_VERSION,
        flags,
        group_tag[0],
        group_tag[1],
        group_tag[2],
        group_tag[3],
    ]
}

/// Host manufacturer payload including a dial-election key shared across address spaces.
///
/// CoreBluetooth does not expose peer public MACs, so Mac/Android elect on
/// [`dial_key_from_identity`] carried here instead of radio addresses.
pub fn manufacturer_role_payload_with_dial_key(
    role_capabilities: BleRoleCapabilities,
    group_tag: [u8; GROUP_TAG_LEN],
    dial_key: BleAddress,
) -> [u8; 2 + GROUP_TAG_LEN + DIAL_KEY_LEN] {
    let flags = match role_capabilities {
        BleRoleCapabilities::DualRole => 0,
        BleRoleCapabilities::PeripheralOnly => EXPERIMENTAL_ROLE_PERIPHERAL_ONLY,
    };
    let key = *dial_key.octets();
    [
        EXPERIMENTAL_ROLE_VERSION_WITH_DIAL_KEY,
        flags,
        group_tag[0],
        group_tag[1],
        group_tag[2],
        group_tag[3],
        key[0],
        key[1],
        key[2],
        key[3],
        key[4],
        key[5],
    ]
}

/// First six bytes of a Bluetooth Auto identity, used as a cross-platform dial sort key.
pub fn dial_key_from_identity(identity: BleIdentity) -> BleAddress {
    let bytes = identity.as_bytes();
    BleAddress::new([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]])
}

/// Dial-election key from a manufacturer payload, when the peer advertised v5+.
pub fn dial_key_from_manufacturer(company_id: u16, data: &[u8]) -> Option<BleAddress> {
    if company_id != u16::from_le_bytes(EXPERIMENTAL_ROLE_COMPANY_ID) {
        return None;
    }
    if *data.first()? < EXPERIMENTAL_ROLE_VERSION_WITH_DIAL_KEY {
        return None;
    }
    let key: [u8; DIAL_KEY_LEN] = data
        .get(2 + GROUP_TAG_LEN..2 + GROUP_TAG_LEN + DIAL_KEY_LEN)?
        .try_into()
        .ok()?;
    Some(BleAddress::new(key))
}

pub fn columba_connection_role(
    local_address: BleAddress,
    local_capabilities: BleRoleCapabilities,
    peer_address: BleAddress,
    peer_capabilities: BleRoleCapabilities,
) -> ColumbaConnectionRole {
    match (local_capabilities, peer_capabilities) {
        (BleRoleCapabilities::DualRole, BleRoleCapabilities::PeripheralOnly) => {
            ColumbaConnectionRole::Dial
        }
        (BleRoleCapabilities::PeripheralOnly, BleRoleCapabilities::DualRole) => {
            ColumbaConnectionRole::Accept
        }
        (BleRoleCapabilities::PeripheralOnly, BleRoleCapabilities::PeripheralOnly) => {
            ColumbaConnectionRole::Unavailable
        }
        (BleRoleCapabilities::DualRole, BleRoleCapabilities::DualRole) => {
            match local_address.cmp(&peer_address) {
                core::cmp::Ordering::Less => ColumbaConnectionRole::Dial,
                core::cmp::Ordering::Greater => ColumbaConnectionRole::Accept,
                core::cmp::Ordering::Equal => ColumbaConnectionRole::Unavailable,
            }
        }
    }
}

struct AdWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> AdWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn put(&mut self, ad_type: u8, body: &[u8]) -> Option<()> {
        let field_len = 1 + body.len();
        let end = self.pos + 1 + field_len;
        let slot = self.buf.get_mut(self.pos..end)?;
        slot[0] = u8::try_from(field_len).ok()?;
        slot[1] = ad_type;
        slot[2..].copy_from_slice(body);
        self.pos = end;
        Some(())
    }

    fn len(&self) -> usize {
        self.pos
    }
}

struct AdReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> AdReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
}

impl<'a> Iterator for AdReader<'a> {
    type Item = (u8, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let field_len = *self.buf.get(self.pos)? as usize;
        if field_len == 0 {
            return None;
        }
        let ad_type = *self.buf.get(self.pos + 1)?;
        let body = self.buf.get(self.pos + 2..self.pos + 1 + field_len)?;
        self.pos += 1 + field_len;
        Some((ad_type, body))
    }
}
