#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::interfaces::lora::{
    CodingRate, LoraBandwidth, Modulation, PreambleSymbols, RadioProfile, SpreadingFactor,
};
use prns_core::interfaces::subghz::{
    supported_modes, Frequency, RegulatoryRegion, ResolvedSubGMode, SubGConfiguration, SubGMode,
    SubGRegion, TxPower,
};

fuzz_target!(|bytes: &[u8]| {
    let mut input = [0u8; 12];
    let copied = bytes.len().min(input.len());
    input[..copied].copy_from_slice(&bytes[..copied]);

    let region_selector = input[0] as usize % (RegulatoryRegion::ALL.len() + 1);
    let region = if region_selector == RegulatoryRegion::ALL.len() {
        SubGRegion::Custom
    } else {
        SubGRegion::Regulated(RegulatoryRegion::ALL[region_selector])
    };
    let spreading_factor = match input[1] % 8 {
        0 => SpreadingFactor::Sf5,
        1 => SpreadingFactor::Sf6,
        2 => SpreadingFactor::Sf7,
        3 => SpreadingFactor::Sf8,
        4 => SpreadingFactor::Sf9,
        5 => SpreadingFactor::Sf10,
        6 => SpreadingFactor::Sf11,
        _ => SpreadingFactor::Sf12,
    };
    let bandwidth = match input[2] % 3 {
        0 => LoraBandwidth::Bw125kHz,
        1 => LoraBandwidth::Bw250kHz,
        _ => LoraBandwidth::Bw500kHz,
    };
    let coding_rate = match input[3] % 4 {
        0 => CodingRate::Cr45,
        1 => CodingRate::Cr46,
        2 => CodingRate::Cr47,
        _ => CodingRate::Cr48,
    };
    let frequency = Frequency::new(u32::from_le_bytes(input[4..8].try_into().unwrap()));
    let tx_power = TxPower::new(i8::from_le_bytes([input[8]]));
    let preamble = PreambleSymbols::new(u16::from_le_bytes([input[9], input[10]]));

    if let Ok(profile) = RadioProfile::new(
        region,
        frequency,
        Modulation::Lora {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        tx_power,
        preamble,
    ) {
        assert_eq!(profile.validate(), Ok(()));
        let configuration = SubGConfiguration::manual_lora(profile);
        assert_eq!(configuration.region(), region);
        assert_eq!(configuration.mode(), SubGMode::ManualLoRa);
        let ResolvedSubGMode::LoRa(resolved) = configuration.resolve();
        assert_eq!(resolved, profile);
    }

    assert_eq!(
        SubGConfiguration::auto_lora_for(region).is_ok(),
        supported_modes(region).supports(SubGMode::AutoLoRa)
    );
});
