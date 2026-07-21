use super::identity::BleAddress;

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
pub(super) const EXPERIMENTAL_ROLE_VERSION: u8 = 0x03;
pub(super) const EXPERIMENTAL_ROLE_PERIPHERAL_ONLY: u8 = 0x01;

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

pub fn encode_advertisement(
    out: &mut [u8],
    role_capabilities: BleRoleCapabilities,
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
        || *data.first()? < EXPERIMENTAL_ROLE_VERSION
    {
        return None;
    }
    if data.get(1)? & EXPERIMENTAL_ROLE_PERIPHERAL_ONLY == 0 {
        Some(BleRoleCapabilities::DualRole)
    } else {
        Some(BleRoleCapabilities::PeripheralOnly)
    }
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
