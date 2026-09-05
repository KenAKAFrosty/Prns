use super::*;
use crate::interfaces::subghz::turbo::{
    TURBO_CHANNEL_COUNT, TURBO_OCCUPANCY_LIMIT_US, US915_TURBO_CHANNELS,
};
use crate::interfaces::subghz::MonotonicMicros;

fn model() -> Us915HoppingModel {
    Us915HoppingModel::new(
        MeasuredTwentyDbBandwidth::new(500_000).unwrap(),
        ChannelOccupancyLimit::new(TURBO_OCCUPANCY_LIMIT_US).unwrap(),
    )
    .unwrap()
}

#[test]
fn wide_us915_model_requires_at_least_twenty_five_channels() {
    let model = model();
    assert_eq!(model.minimum_channel_count(), 25);
    assert_eq!(model.observation_window_us(), 10_000_000);
    assert_eq!(model.minimum_carrier_separation_hz(), 500_000);
    assert!(matches!(
        Us915HopSet::<24>::evenly_spaced(model),
        Err(HopSetError::InsufficientChannels {
            provided: 24,
            required: 25,
        })
    ));
}

#[test]
fn turbo_channels_form_a_valid_us915_hop_set() {
    let hop_set = Us915HopSet::new(model(), US915_TURBO_CHANNELS).unwrap();
    assert_eq!(hop_set.channels(), &US915_TURBO_CHANNELS);
    assert_eq!(TURBO_CHANNEL_COUNT, 51);
}

#[test]
fn offline_oracle_rejects_the_first_excess_microsecond() {
    let hop_set = Us915HopSet::new(model(), US915_TURBO_CHANNELS).unwrap();
    let transmissions = [
        HopTransmission::new(0, MonotonicMicros::new(0), 300_000).unwrap(),
        HopTransmission::new(0, MonotonicMicros::new(5_000_000), 90_001).unwrap(),
    ];
    assert!(matches!(
        audit_frequency_occupancy(&hop_set, &transmissions),
        Err(FrequencyOccupancyError::ChannelOccupancyExceeded {
            occupied_us: 390_001,
            maximum_us: 390_000,
            ..
        })
    ));
}

#[test]
fn ten_second_quarantine_leaves_no_prior_transmission_in_the_window() {
    let hop_set = Us915HopSet::new(model(), US915_TURBO_CHANNELS).unwrap();
    let transmissions = [
        HopTransmission::new(0, MonotonicMicros::new(0), 390_000).unwrap(),
        HopTransmission::new(0, MonotonicMicros::new(10_400_000), 390_000).unwrap(),
    ];
    assert!(audit_frequency_occupancy(&hop_set, &transmissions).is_ok());
}

#[test]
fn power_class_changes_at_fifty_channels_and_applies_antenna_budget() {
    let inputs = Us915PowerInputs::new(
        ConductedPowerDbm::new(30).unwrap(),
        AntennaGainDeciDb::new(60),
        ConductedPowerDbm::new(30).unwrap(),
    );
    let low = Us915PowerBudget::for_hop_count(49, inputs).unwrap();
    let high = Us915PowerBudget::for_hop_count(50, inputs).unwrap();
    assert_eq!(low.class(), Us915PowerClass::QuarterWatt);
    assert_eq!(low.class().maximum_conducted_milliwatts(), 250);
    assert_eq!(low.selected().dbm(), 23);
    assert_eq!(high.class(), Us915PowerClass::OneWatt);
    assert_eq!(high.class().maximum_conducted_milliwatts(), 1_000);
    assert_eq!(high.selected().dbm(), 30);

    let high_gain = Us915PowerInputs::new(
        ConductedPowerDbm::new(30).unwrap(),
        AntennaGainDeciDb::new(75),
        ConductedPowerDbm::new(30).unwrap(),
    );
    assert_eq!(
        Us915PowerBudget::for_hop_count(51, high_gain)
            .unwrap()
            .selected()
            .dbm(),
        28
    );
}

#[test]
fn model_assesses_supplied_airtime_without_owning_a_modulation() {
    assert_eq!(
        model().assess_airtime(390_000),
        FrameDwell::WithinLimit {
            airtime_us: 390_000,
            remaining_us: 0,
        }
    );
    assert_eq!(
        model().assess_airtime(390_001),
        FrameDwell::ExceedsLimit {
            airtime_us: 390_001,
            excess_us: 1,
        }
    );
}
