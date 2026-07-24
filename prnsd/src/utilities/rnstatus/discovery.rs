use std::fmt::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use personal_rns::interface_discovery::{
    discovered_interface_configuration, AdvertisementDetails, DiscoveredInterfaceStatus,
    DiscoveryArchive, DiscoveryRecord, DISCOVERED_INTERFACES_FILE,
};
use personal_rns::units::InstantMillis;
use serde_json::{Map, Value};

use super::render::pretty_time;
use super::RnstatusArgs;

pub fn render(config_dir: &Path, args: &RnstatusArgs) -> Result<String, String> {
    let loaded = DiscoveryArchive::load(config_dir.join(DISCOVERED_INTERFACES_FILE))
        .map_err(|error| format!("could not load discovered interfaces: {error}"))?;
    let now = unix_time_millis();
    let records = loaded
        .catalog
        .ranked_records(now)
        .into_iter()
        .filter(|record| record.status(now) != DiscoveredInterfaceStatus::Expired)
        .filter(|record| {
            args.filter.as_ref().is_none_or(|filter| {
                record
                    .interface()
                    .name
                    .to_lowercase()
                    .contains(&filter.to_lowercase())
            })
        })
        .collect::<Vec<_>>();
    if args.json {
        let values = records
            .into_iter()
            .map(|record| discovery_value(record, now))
            .collect::<Vec<_>>();
        return serde_json::to_string(&values)
            .map(|mut output| {
                output.push('\n');
                output
            })
            .map_err(|error| format!("could not encode discovered interfaces: {error}"));
    }
    if args.discovery_details {
        Ok(render_details(&records, now))
    } else {
        Ok(render_table(&records, now))
    }
}

fn render_table(records: &[&DiscoveryRecord], now: InstantMillis) -> String {
    let mut output = String::new();
    output.push('\n');
    let _ = writeln!(
        output,
        "{:<25} {:<12} {:<12} {:<12} {:<8} {:<15}",
        "Name", "Type", "Status", "Last Heard", "Value", "Location"
    );
    let _ = writeln!(output, "{}", "-".repeat(89));
    for record in records {
        let interface = record.interface();
        let name = truncate_name(&interface.name);
        let interface_type = interface
            .advertisement
            .interface_type
            .rns_name()
            .trim_end_matches("Interface");
        let status = status_table(record.status(now));
        let last_heard = short_age(now.0.saturating_sub(record.last_heard().0));
        let location = location_short(
            interface.advertisement.location.latitude,
            interface.advertisement.location.longitude,
        );
        let _ = writeln!(
            output,
            "{name:<25} {interface_type:<12} {status:<12} {last_heard:<12} {:<8} {location:<15}",
            interface.stamp_value.get()
        );
    }
    output
}

fn render_details(records: &[&DiscoveryRecord], now: InstantMillis) -> String {
    let mut output = String::new();
    output.push('\n');
    for (index, record) in records.iter().enumerate() {
        if index != 0 {
            output.push_str("\n================================\n\n");
        }
        let interface = record.interface();
        let advertisement = &interface.advertisement;
        let transport_id = advertisement.transport.transport_id();
        if transport_id.as_bytes() != interface.provenance.announced_by.as_bytes() {
            let _ = writeln!(
                output,
                "Network   ID : {}",
                hex(interface.provenance.announced_by.as_bytes())
            );
        }
        let _ = writeln!(output, "Transport ID : {}", hex(transport_id.as_bytes()));
        let _ = writeln!(output, "Name         : {}", interface.name);
        let _ = writeln!(
            output,
            "Type         : {}",
            advertisement.interface_type.rns_name()
        );
        let _ = writeln!(
            output,
            "Status       : {}",
            status_title(record.status(now))
        );
        let _ = writeln!(
            output,
            "Transport    : {}",
            if advertisement.transport.is_enabled() {
                "Enabled"
            } else {
                "Disabled"
            }
        );
        let hops = interface.provenance.hops.0;
        let _ = writeln!(
            output,
            "Distance     : {hops} hop{}",
            if hops == 1 { "" } else { "s" }
        );
        let _ = writeln!(
            output,
            "Discovered   : {} ago",
            pretty_time(
                now.0.saturating_sub(record.first_heard().0) as f64 / 1_000.0,
                true
            )
        );
        let _ = writeln!(
            output,
            "Last Heard   : {} ago",
            pretty_time(
                now.0.saturating_sub(record.last_heard().0) as f64 / 1_000.0,
                true
            )
        );
        let _ = writeln!(
            output,
            "Location     : {}",
            location_detailed(
                advertisement.location.latitude,
                advertisement.location.longitude,
                advertisement.location.height,
            )
        );
        render_advertisement_details(&mut output, &advertisement.details);
        let _ = writeln!(output, "Stamp Value  : {}", interface.stamp_value.get());
        if let Some(configuration) = discovered_interface_configuration(interface) {
            output.push_str("\nConfiguration Entry:\n");
            for line in configuration.lines() {
                let _ = writeln!(output, "  {line}");
            }
        }
    }
    output
}

fn render_advertisement_details(output: &mut String, details: &AdvertisementDetails) {
    match details {
        AdvertisementDetails::None => {}
        AdvertisementDetails::Reachable { host, port } => {
            let _ = writeln!(output, "Address      : {host}");
            let _ = writeln!(output, "Port         : {port}");
        }
        AdvertisementDetails::I2p { address } => {
            let _ = writeln!(output, "Address      : {address}");
        }
        AdvertisementDetails::RNode {
            frequency_hz,
            bandwidth_hz,
            spreading_factor,
            coding_rate,
        } => {
            let _ = writeln!(
                output,
                "Frequency    : {}",
                format_si_frequency(*frequency_hz)
            );
            let _ = writeln!(
                output,
                "Bandwidth    : {}",
                format_si_frequency(u64::from(*bandwidth_hz))
            );
            let _ = writeln!(output, "Spreading SF : SF{spreading_factor}");
            let _ = writeln!(output, "Coding Rate  : 4/{coding_rate}");
        }
        AdvertisementDetails::Weave {
            frequency_hz,
            bandwidth_hz,
            channel: _,
            modulation,
        }
        | AdvertisementDetails::Kiss {
            frequency_hz,
            bandwidth_hz,
            modulation,
        } => {
            let _ = writeln!(
                output,
                "Frequency    : {}",
                format_si_frequency(*frequency_hz)
            );
            let _ = writeln!(
                output,
                "Bandwidth    : {}",
                format_si_frequency(u64::from(*bandwidth_hz))
            );
            let _ = writeln!(output, "Modulation   : {modulation}");
        }
    }
}

fn discovery_value(record: &DiscoveryRecord, now: InstantMillis) -> Value {
    let interface = record.interface();
    let advertisement = &interface.advertisement;
    let mut fields = Map::new();
    fields.insert(String::from("name"), interface.name.clone().into());
    fields.insert(
        String::from("type"),
        advertisement.interface_type.rns_name().into(),
    );
    fields.insert(
        String::from("status"),
        status_name(record.status(now)).into(),
    );
    fields.insert(
        String::from("transport"),
        advertisement.transport.is_enabled().into(),
    );
    fields.insert(
        String::from("transport_id"),
        hex(advertisement.transport.transport_id().as_bytes()).into(),
    );
    fields.insert(
        String::from("network_id"),
        hex(interface.provenance.announced_by.as_bytes()).into(),
    );
    fields.insert(String::from("hops"), interface.provenance.hops.0.into());
    fields.insert(
        String::from("discovered"),
        number(record.first_heard().0 as f64 / 1_000.0),
    );
    fields.insert(
        String::from("last_heard"),
        number(record.last_heard().0 as f64 / 1_000.0),
    );
    fields.insert(
        String::from("heard_count"),
        record.observation_count().get().saturating_sub(1).into(),
    );
    fields.insert(String::from("value"), interface.stamp_value.get().into());
    fields.insert(
        String::from("latitude"),
        optional_number(advertisement.location.latitude),
    );
    fields.insert(
        String::from("longitude"),
        optional_number(advertisement.location.longitude),
    );
    fields.insert(
        String::from("height"),
        optional_number(advertisement.location.height),
    );
    insert_discovery_details(&mut fields, &advertisement.details);
    if let Some(ifac) = &advertisement.published_ifac {
        if let Some(name) = &ifac.network_name {
            fields.insert(String::from("ifac_netname"), name.clone().into());
        }
        if let Some(passphrase) = &ifac.passphrase {
            fields.insert(String::from("ifac_netkey"), passphrase.clone().into());
        }
    }
    if let Some(configuration) = discovered_interface_configuration(interface) {
        fields.insert(String::from("config_entry"), configuration.into());
    }
    Value::Object(fields)
}

fn insert_discovery_details(fields: &mut Map<String, Value>, details: &AdvertisementDetails) {
    match details {
        AdvertisementDetails::None => {}
        AdvertisementDetails::Reachable { host, port } => {
            fields.insert(String::from("reachable_on"), host.clone().into());
            fields.insert(String::from("port"), (*port).into());
        }
        AdvertisementDetails::I2p { address } => {
            fields.insert(String::from("reachable_on"), address.clone().into());
        }
        AdvertisementDetails::RNode {
            frequency_hz,
            bandwidth_hz,
            spreading_factor,
            coding_rate,
        } => {
            fields.insert(String::from("frequency"), (*frequency_hz).into());
            fields.insert(String::from("bandwidth"), (*bandwidth_hz).into());
            fields.insert(String::from("sf"), (*spreading_factor).into());
            fields.insert(String::from("cr"), (*coding_rate).into());
        }
        AdvertisementDetails::Weave {
            frequency_hz,
            bandwidth_hz,
            channel,
            modulation,
        } => {
            fields.insert(String::from("frequency"), (*frequency_hz).into());
            fields.insert(String::from("bandwidth"), (*bandwidth_hz).into());
            fields.insert(String::from("channel"), (*channel).into());
            fields.insert(String::from("modulation"), modulation.clone().into());
        }
        AdvertisementDetails::Kiss {
            frequency_hz,
            bandwidth_hz,
            modulation,
        } => {
            fields.insert(String::from("frequency"), (*frequency_hz).into());
            fields.insert(String::from("bandwidth"), (*bandwidth_hz).into());
            fields.insert(String::from("modulation"), modulation.clone().into());
        }
    }
}

fn truncate_name(name: &str) -> String {
    if name.chars().count() <= 25 {
        return String::from(name);
    }
    name.chars().take(24).chain(std::iter::once('…')).collect()
}

fn short_age(age_ms: u64) -> String {
    let seconds = age_ms / 1_000;
    if seconds < 60 {
        String::from("Just now")
    } else if seconds < 3_600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3_600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

fn location_short(latitude: Option<f64>, longitude: Option<f64>) -> String {
    match (latitude, longitude) {
        (Some(latitude), Some(longitude)) => {
            format!("{:.4}, {:.4}", round_four(latitude), round_four(longitude))
        }
        _ => String::from("N/A"),
    }
}

fn location_detailed(latitude: Option<f64>, longitude: Option<f64>, height: Option<f64>) -> String {
    match (latitude, longitude) {
        (Some(latitude), Some(longitude)) => {
            let mut location = format!("{:.4}, {:.4}", round_four(latitude), round_four(longitude));
            if let Some(height) = height {
                let _ = write!(location, " · altitude {height} m");
            }
            location
        }
        _ => String::from("Unknown"),
    }
}

fn format_si_frequency(hertz: u64) -> String {
    if hertz >= 1_000_000 {
        format_scaled_quantity(hertz, 1_000_000, "MHz")
    } else if hertz >= 1_000 {
        format_scaled_quantity(hertz, 1_000, "kHz")
    } else {
        format!("{hertz} Hz")
    }
}

fn format_scaled_quantity(value: u64, scale: u64, unit: &str) -> String {
    let whole = value / scale;
    let remainder = value % scale;
    if remainder == 0 {
        return format!("{whole} {unit}");
    }
    let width = scale.ilog10() as usize;
    let fractional = format!("{remainder:0width$}")
        .trim_end_matches('0')
        .to_string();
    format!("{whole}.{fractional} {unit}")
}

fn round_four(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn status_name(status: DiscoveredInterfaceStatus) -> &'static str {
    match status {
        DiscoveredInterfaceStatus::Available => "available",
        DiscoveredInterfaceStatus::Unknown => "unknown",
        DiscoveredInterfaceStatus::Stale => "stale",
        DiscoveredInterfaceStatus::Expired => "expired",
    }
}

fn status_title(status: DiscoveredInterfaceStatus) -> &'static str {
    match status {
        DiscoveredInterfaceStatus::Available => "Available",
        DiscoveredInterfaceStatus::Unknown => "Unknown",
        DiscoveredInterfaceStatus::Stale => "Stale",
        DiscoveredInterfaceStatus::Expired => "Expired",
    }
}

fn status_table(status: DiscoveredInterfaceStatus) -> &'static str {
    match status {
        DiscoveredInterfaceStatus::Available => "✓ Available",
        DiscoveredInterfaceStatus::Unknown => "? Unknown",
        DiscoveredInterfaceStatus::Stale => "× Stale",
        DiscoveredInterfaceStatus::Expired => "× Expired",
    }
}

fn optional_number(value: Option<f64>) -> Value {
    value.map_or(Value::Null, number)
}

fn number(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn unix_time_millis() -> InstantMillis {
    InstantMillis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                duration.as_millis().min(u128::from(u64::MAX)) as u64
            }),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::{format_si_frequency, location_detailed, render_advertisement_details};
    use personal_rns::interface_discovery::AdvertisementDetails;

    #[test]
    fn radio_details_use_practitioner_units_and_lora_notation() {
        let mut output = String::new();
        render_advertisement_details(
            &mut output,
            &AdvertisementDetails::RNode {
                frequency_hz: 868_100_000,
                bandwidth_hz: 125_000,
                spreading_factor: 8,
                coding_rate: 5,
            },
        );
        assert!(output.contains("868.1 MHz"));
        assert!(output.contains("125 kHz"));
        assert!(output.contains("SF8"));
        assert!(output.contains("4/5"));
        assert_eq!(format_si_frequency(915), "915 Hz");
    }

    #[test]
    fn location_height_is_presented_as_altitude_in_metres() {
        assert_eq!(
            location_detailed(Some(41.0), Some(-87.0), Some(180.0)),
            "41.0000, -87.0000 · altitude 180 m"
        );
    }
}
