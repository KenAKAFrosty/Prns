use super::{
    DCACHE_RAM, DRAM, HELTEC_E290, HELTEC_V4, HELTEC_V4_R8, HELTEC_WIRELESS_STICK_LITE_V3, IRAM,
    PSRAM, RECLAIMED_RAM, RTC_FAST_RAM, RTC_SLOW_RAM, T_BEAM_SUPREME, XIAO_ESP32_C6,
};
use crate::profiles::linker::{LinkerAddressProfile, LinkerAddressSpace};
use crate::profiles::FLASH;
use crate::AddressRange;

const ESP32S3_FLASH: [AddressRange; 2] = [
    AddressRange::new(0x3C00_0000, 0x3E00_0000),
    AddressRange::new(0x4200_0000, 0x4400_0000),
];
const ESP32S3_IRAM: [AddressRange; 1] = [AddressRange::new(0x4037_0000, 0x4040_0000)];
const ESP32S3_DRAM: [AddressRange; 1] = [AddressRange::new(0x3FC0_0000, 0x4000_0000)];
const ESP32C6_FLASH: [AddressRange; 1] = [AddressRange::new(0x4200_0000, 0x4240_0000)];
const ESP32C6_RAM: [AddressRange; 1] = [AddressRange::new(0x4080_0000, 0x4090_0000)];

const ESP32S3_SPACES: [LinkerAddressSpace; 8] = [
    LinkerAddressSpace::explicit(FLASH, &ESP32S3_FLASH),
    LinkerAddressSpace::explicit(IRAM, &ESP32S3_IRAM),
    LinkerAddressSpace::explicit(DRAM, &ESP32S3_DRAM),
    LinkerAddressSpace::unmapped(RECLAIMED_RAM),
    LinkerAddressSpace::unmapped(DCACHE_RAM),
    LinkerAddressSpace::memory_geometry(RTC_FAST_RAM),
    LinkerAddressSpace::memory_geometry(RTC_SLOW_RAM),
    LinkerAddressSpace::unmapped(PSRAM),
];
const ESP32S3_NO_PSRAM_SPACES: [LinkerAddressSpace; 7] = [
    ESP32S3_SPACES[0],
    ESP32S3_SPACES[1],
    ESP32S3_SPACES[2],
    ESP32S3_SPACES[3],
    ESP32S3_SPACES[4],
    ESP32S3_SPACES[5],
    ESP32S3_SPACES[6],
];
const ESP32C6_SPACES: [LinkerAddressSpace; 3] = [
    LinkerAddressSpace::explicit(FLASH, &ESP32C6_FLASH),
    LinkerAddressSpace::explicit(IRAM, &ESP32C6_RAM),
    LinkerAddressSpace::explicit(DRAM, &ESP32C6_RAM),
];

const HELTEC_V4_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(HELTEC_V4.id, &ESP32S3_SPACES);
const HELTEC_V4_R8_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(HELTEC_V4_R8.id, &ESP32S3_SPACES);
const HELTEC_E290_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(HELTEC_E290.id, &ESP32S3_SPACES);
const T_BEAM_SUPREME_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(T_BEAM_SUPREME.id, &ESP32S3_SPACES);
const HELTEC_WIRELESS_STICK_LITE_V3_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(HELTEC_WIRELESS_STICK_LITE_V3.id, &ESP32S3_NO_PSRAM_SPACES);
const XIAO_ESP32_C6_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(XIAO_ESP32_C6.id, &ESP32C6_SPACES);

pub(in crate::profiles) const LINKER_ADDRESS_PROFILES: [&LinkerAddressProfile; 6] = [
    &HELTEC_V4_LINKER,
    &HELTEC_V4_R8_LINKER,
    &HELTEC_E290_LINKER,
    &HELTEC_WIRELESS_STICK_LITE_V3_LINKER,
    &T_BEAM_SUPREME_LINKER,
    &XIAO_ESP32_C6_LINKER,
];
