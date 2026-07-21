use embassy_time::{Duration, Timer};
use embedded_storage_async::nor_flash::{NorFlash, ReadNorFlash};
use nrf_softdevice::{random_bytes, Flash, FlashError, Softdevice};

use personal_rns::interfaces::bluetooth_auto::{
    decode_persisted_ble_identity, encode_persisted_ble_identity, BleIdentity,
    PersistedBleIdentityError, PERSISTED_BLE_IDENTITY_LEN,
};

const BLE_IDENTITY_FLASH_OFFSET: u32 = 0xEC000;
const ENTROPY_ATTEMPTS: usize = 200;

#[repr(align(4))]
struct AlignedIdentityRecord([u8; PERSISTED_BLE_IDENTITY_LEN]);

pub(super) async fn load_or_create(
    sd: &'static Softdevice,
) -> Result<BleIdentity, NrfBleIdentityError> {
    let mut flash = Flash::take(sd);
    if let Some(identity) = read_identity(&mut flash).await? {
        return Ok(identity);
    }
    let mut bytes = [0u8; 16];
    let mut generated = false;
    for _ in 0..ENTROPY_ATTEMPTS {
        if random_bytes(sd, &mut bytes).is_ok() {
            generated = true;
            break;
        }
        Timer::after(Duration::from_millis(5)).await;
    }
    if !generated {
        return Err(NrfBleIdentityError::EntropyUnavailable);
    }
    let identity = BleIdentity::new(bytes);
    let record = AlignedIdentityRecord(encode_persisted_ble_identity(identity));
    flash
        .write(BLE_IDENTITY_FLASH_OFFSET + 8, &record.0[8..])
        .await
        .map_err(NrfBleIdentityError::Flash)?;
    flash
        .write(BLE_IDENTITY_FLASH_OFFSET, &record.0[..8])
        .await
        .map_err(NrfBleIdentityError::Flash)?;
    match read_identity(&mut flash).await? {
        Some(persisted) if persisted == identity => Ok(identity),
        Some(_) | None => Err(NrfBleIdentityError::Verification),
    }
}

async fn read_identity(flash: &mut Flash) -> Result<Option<BleIdentity>, NrfBleIdentityError> {
    let mut record = AlignedIdentityRecord([0u8; PERSISTED_BLE_IDENTITY_LEN]);
    flash
        .read(BLE_IDENTITY_FLASH_OFFSET, &mut record.0)
        .await
        .map_err(NrfBleIdentityError::Flash)?;
    decode_persisted_ble_identity(&record.0).map_err(NrfBleIdentityError::Stored)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NrfBleIdentityError {
    Flash(FlashError),
    EntropyUnavailable,
    Stored(PersistedBleIdentityError),
    Verification,
}

impl core::fmt::Display for NrfBleIdentityError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Flash(error) => write!(formatter, "BLE identity flash: {error:?}"),
            Self::EntropyUnavailable => formatter.write_str("BLE identity entropy unavailable"),
            Self::Stored(error) => error.fmt(formatter),
            Self::Verification => formatter.write_str("BLE identity flash verification failed"),
        }
    }
}
