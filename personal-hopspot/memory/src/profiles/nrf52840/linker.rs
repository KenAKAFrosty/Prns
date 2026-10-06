use super::{
    MESH_POCKET_10000, MESH_POCKET_5000, MESH_TOWER_V2, MUZI_BASE_DUO, RAK10724, RAK4631,
    SENSECAP_SOLAR_NODE, T096, T1000_E, T114, T_ECHO_S140_V6, T_ECHO_S140_V7, WIO_TRACKER_L1,
};
use crate::profiles::linker::{LinkerAddressProfile, LinkerAddressSpace};
use crate::profiles::{FLASH, RAM};

const NRF52840_SPACES: [LinkerAddressSpace; 2] = [
    LinkerAddressSpace::firmware_owned(FLASH),
    LinkerAddressSpace::memory_geometry(RAM),
];

const T_ECHO_S140_V6_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(T_ECHO_S140_V6.id, &NRF52840_SPACES);
const T_ECHO_S140_V7_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(T_ECHO_S140_V7.id, &NRF52840_SPACES);
const T096_LINKER: LinkerAddressProfile = LinkerAddressProfile::new(T096.id, &NRF52840_SPACES);
const T114_LINKER: LinkerAddressProfile = LinkerAddressProfile::new(T114.id, &NRF52840_SPACES);
const MESH_POCKET_5000_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(MESH_POCKET_5000.id, &NRF52840_SPACES);
const MESH_POCKET_10000_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(MESH_POCKET_10000.id, &NRF52840_SPACES);
const T1000_E_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(T1000_E.id, &NRF52840_SPACES);
const MESH_TOWER_V2_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(MESH_TOWER_V2.id, &NRF52840_SPACES);
const MUZI_BASE_DUO_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(MUZI_BASE_DUO.id, &NRF52840_SPACES);
const RAK4631_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(RAK4631.id, &NRF52840_SPACES);
const WIO_TRACKER_L1_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(WIO_TRACKER_L1.id, &NRF52840_SPACES);

const SENSECAP_SOLAR_NODE_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(SENSECAP_SOLAR_NODE.id, &NRF52840_SPACES);

const RAK10724_LINKER: LinkerAddressProfile =
    LinkerAddressProfile::new(RAK10724.id, &NRF52840_SPACES);

pub(in crate::profiles) const LINKER_ADDRESS_PROFILES: [&LinkerAddressProfile; 13] = [
    &T_ECHO_S140_V6_LINKER,
    &T_ECHO_S140_V7_LINKER,
    &T096_LINKER,
    &T114_LINKER,
    &MESH_POCKET_5000_LINKER,
    &MESH_POCKET_10000_LINKER,
    &T1000_E_LINKER,
    &SENSECAP_SOLAR_NODE_LINKER,
    &MESH_TOWER_V2_LINKER,
    &MUZI_BASE_DUO_LINKER,
    &RAK4631_LINKER,
    &RAK10724_LINKER,
    &WIO_TRACKER_L1_LINKER,
];
