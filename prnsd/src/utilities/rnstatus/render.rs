use std::cmp::Ordering;
use std::fmt::Write;

use personal_rns::interfaces::rns_management::{
    RnsInterfaceStatsReport, RnsInterfaceStatusReport, RnsOptionalField,
};

use super::{RnstatusArgs, RnstatusSort};

pub fn human(
    report: &RnsInterfaceStatsReport,
    link_count: Option<u64>,
    args: &RnstatusArgs,
    now: f64,
) -> String {
    let mut output = String::new();
    let mut interfaces = report.interfaces.iter().collect::<Vec<_>>();
    sort_interfaces(&mut interfaces, args.sort, args.reverse);
    for status in interfaces {
        if should_show(status, args) {
            render_interface(&mut output, status, args, now);
        }
    }
    render_footer(&mut output, report, link_count, args);
    output.push('\n');
    output
}

fn sort_interfaces(
    interfaces: &mut [&RnsInterfaceStatusReport],
    sorting: Option<RnstatusSort>,
    reverse: bool,
) {
    let Some(sorting) = sorting else {
        return;
    };
    interfaces.sort_by(|left, right| {
        let ordering = numeric_sort_value(left, sorting)
            .partial_cmp(&numeric_sort_value(right, sorting))
            .unwrap_or(Ordering::Equal);
        if reverse {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn numeric_sort_value(status: &RnsInterfaceStatusReport, sorting: RnstatusSort) -> f64 {
    match sorting {
        RnstatusSort::Rate => optional_number(&status.bitrate_bps),
        RnstatusSort::Traffic => status.receive_bytes.saturating_add(status.transmit_bytes) as f64,
        RnstatusSort::Rx => status.receive_bytes as f64,
        RnstatusSort::Tx => status.transmit_bytes as f64,
        RnstatusSort::Rxs => status.receive_speed_bps,
        RnstatusSort::Txs => status.transmit_speed_bps,
        RnstatusSort::Announces => {
            optional_number(&status.incoming_announce_frequency)
                + optional_number(&status.outgoing_announce_frequency)
        }
        RnstatusSort::Arx => optional_number(&status.incoming_announce_frequency),
        RnstatusSort::Atx => optional_number(&status.outgoing_announce_frequency),
        RnstatusSort::Prx => optional_number(&status.incoming_path_request_frequency),
        RnstatusSort::Ptx => optional_number(&status.outgoing_path_request_frequency),
        RnstatusSort::Held => optional_u64(&status.held_announces) as f64,
        RnstatusSort::Gravity => optional_i64(&status.gravity) as f64,
    }
}

fn should_show(status: &RnsInterfaceStatusReport, args: &RnstatusArgs) -> bool {
    if outbound_i2p_parent(status) {
        return false;
    }
    if !args.show_all && hidden_member(&status.name) {
        return false;
    }
    selection_matches(
        &status.name,
        args.filter.as_deref(),
        args.burst,
        burst_active(status),
    )
}

fn selection_matches(
    name: &str,
    filter: Option<&str>,
    burst_only: bool,
    active_burst: bool,
) -> bool {
    let name_matches =
        filter.is_some_and(|filter| name.to_lowercase().contains(&filter.to_lowercase()));
    if burst_only {
        active_burst || name_matches
    } else {
        filter.is_none() || name_matches
    }
}

fn hidden_member(name: &str) -> bool {
    name.starts_with("LocalInterface[")
        || name.starts_with("TCPInterface[Client")
        || name.starts_with("BackboneInterface[Client on")
        || name.starts_with("AutoInterfacePeer[")
        || name.starts_with("WeaveInterfacePeer[")
        || name.starts_with("I2PInterfacePeer[Connected peer")
}

fn outbound_i2p_parent(status: &RnsInterfaceStatusReport) -> bool {
    status.name.starts_with("I2PInterface[")
        && matches!(status.i2p_connectable, RnsOptionalField::Value(false))
}

fn burst_active(status: &RnsInterfaceStatusReport) -> bool {
    matches!(status.burst_active, RnsOptionalField::Value(true))
        || matches!(
            status.path_request_burst_active,
            RnsOptionalField::Value(true)
        )
}

fn render_interface(
    output: &mut String,
    status: &RnsInterfaceStatusReport,
    args: &RnstatusArgs,
    now: f64,
) {
    let _ = writeln!(output, "\n {}", status.name);
    write_optional(
        output,
        "    Source    : Auto-connect via <",
        ">",
        &status.autoconnect_source,
    );
    write_optional(output, "    Network   : ", "", &status.ifac_network_name);
    write_optional(output, "    Group     : ", "", &status.group_id);
    let _ = writeln!(
        output,
        "    Status    : {}",
        if status.online { "Up" } else { "Down" }
    );
    if let Some(clients) = status.clients.value().copied() {
        render_clients(output, status, clients);
    }
    if !mode_is_suppressed(&status.name) {
        let _ = writeln!(output, "    Mode      : {}", status.mode.display_name());
    }
    render_gravity(output, &status.gravity);
    if let Some(bitrate) = status.bitrate_bps.value() {
        let _ = writeln!(output, "    Rate      : {}", speed_str(*bitrate));
    }
    render_radio_state(output, status, now);
    render_device_state(output, status);
    render_medium_state(output, status);
    render_access(output, status);
    render_announce_state(output, status, args, now);
    render_traffic(output, status);
    render_fleet_peers(output, status);
}

fn render_fleet_peers(output: &mut String, status: &RnsInterfaceStatusReport) {
    if status.fleet_peers.is_empty() {
        return;
    }
    let count = status.fleet_peers.len();
    let _ = writeln!(output, "    Peers     : {count} connected");
    for peer in &status.fleet_peers {
        let state = if peer.online { "Up" } else { "Down" };
        let mut line = format!("      {}  {state}", peer.name);
        if let Some(rssi) = peer.rssi.value() {
            line.push_str(&format!("  rssi {rssi}"));
        }
        let _ = writeln!(output, "{line}");
        let tx = format!(
            "↑{}  {}",
            pretty_size(peer.transmit_bytes as f64, "B"),
            pretty_speed(peer.transmit_speed_bps)
        );
        let rx = format!(
            "↓{}  {}",
            pretty_size(peer.receive_bytes as f64, "B"),
            pretty_speed(peer.receive_speed_bps)
        );
        let _ = writeln!(output, "        Traffic   : {tx}\n                    {rx}");
    }
}

fn render_clients(output: &mut String, status: &RnsInterfaceStatusReport, clients: u64) {
    if status.name.starts_with("Shared Instance[") {
        let programs = clients.saturating_sub(1);
        let suffix = if programs == 1 { "program" } else { "programs" };
        let _ = writeln!(output, "    Serving   : {programs} {suffix}");
    } else if status.name.starts_with("I2PInterface[") {
        if matches!(status.i2p_connectable, RnsOptionalField::Value(true)) {
            let suffix = if clients == 1 {
                "connected I2P endpoint"
            } else {
                "connected I2P endpoints"
            };
            let _ = writeln!(output, "    Peers     : {clients} {suffix}");
        }
    } else {
        let _ = writeln!(output, "    Clients   : {clients}");
    }
}

fn mode_is_suppressed(name: &str) -> bool {
    name.starts_with("Shared Instance[")
        || name.starts_with("TCPInterface[Client")
        || name.starts_with("LocalInterface[")
}

fn optional_i64(field: &RnsOptionalField<i64>) -> i64 {
    field.value().copied().unwrap_or(0)
}

fn render_gravity(output: &mut String, field: &RnsOptionalField<i64>) {
    if let Some(gravity) = field.value().filter(|gravity| **gravity != 0) {
        let _ = writeln!(output, "    Gravity   : {gravity}");
    }
}

fn render_radio_state(output: &mut String, status: &RnsInterfaceStatusReport, now: f64) {
    if status.noise_floor_dbm.is_present() {
        match status.noise_floor_dbm.value() {
            Some(noise) => {
                let _ = write!(output, "    Noise Fl. : {} dBm", number(*noise));
                match status.interference_dbm.value() {
                    Some(interference) => {
                        let _ = write!(output, "\n    Intrfrnc. : {} dBm", number(*interference));
                    }
                    None if status.interference_dbm.is_present() => {
                        if let (Some(last_at), Some(last_dbm)) = (
                            status.interference_last_at.value(),
                            status.interference_last_dbm.value(),
                        ) {
                            let _ = write!(
                                output,
                                "\n    Intrfrnc. : {} dBm {} ago",
                                number(*last_dbm),
                                pretty_time((now - last_at).max(0.0), true)
                            );
                        } else {
                            let _ = write!(output, ", no interference");
                        }
                    }
                    None => {}
                }
                output.push('\n');
            }
            None => output.push_str("    Noise Fl. : Unknown\n"),
        }
    }
}

fn render_device_state(output: &mut String, status: &RnsInterfaceStatusReport) {
    render_optional_number(output, "    CPU load  : ", " %", &status.cpu_load_percent);
    render_optional_number(
        output,
        "    CPU temp  : ",
        "°C",
        &status.cpu_temperature_celsius,
    );
    render_optional_number(
        output,
        "    Mem usage : ",
        " %",
        &status.memory_load_percent,
    );
    if let Some(percent) = status.battery_percent.value() {
        let state = status
            .battery_state
            .value()
            .map_or("unknown", String::as_str);
        let _ = writeln!(output, "    Battery   : {}% ({state})", *percent as i64);
    }
}

fn render_medium_state(output: &mut String, status: &RnsInterfaceStatusReport) {
    if let (Some(short), Some(long)) = (
        status.airtime_short_percent.value(),
        status.airtime_long_percent.value(),
    ) {
        let _ = writeln!(
            output,
            "    Airtime   : {}% (15s), {}% (1h)",
            number(*short),
            number(*long)
        );
    }
    if let (Some(short), Some(long)) = (
        status.channel_load_short_percent.value(),
        status.channel_load_long_percent.value(),
    ) {
        let _ = writeln!(
            output,
            "    Ch. Load  : {}% (15s), {}% (1h)",
            number(*short),
            number(*long)
        );
    }
    render_optional_or_unknown(output, "    Switch ID : ", &status.switch_id);
    render_optional_or_unknown(output, "    Endpoint  : ", &status.endpoint_id);
    render_optional_or_unknown(output, "    Via       : ", &status.via_switch_id);
    if let Some(peers) = status.peers.value() {
        let _ = writeln!(output, "    Peers     : {peers} reachable");
    }
    write_optional(output, "    I2P       : ", "", &status.i2p_tunnel_state);
}

fn render_access(output: &mut String, status: &RnsInterfaceStatusReport) {
    if let (Some(signature), Some(size)) = (
        status.ifac_signature.value(),
        status.ifac_size_bytes.value(),
    ) {
        let tail = signature.len().saturating_sub(5);
        let _ = writeln!(
            output,
            "    Access    : {}-bit IFAC by <…{}>",
            size.saturating_mul(8),
            hex(&signature[tail..])
        );
    }
    write_optional(output, "    I2P B32   : ", "", &status.i2p_b32);
}

fn render_announce_state(
    output: &mut String,
    status: &RnsInterfaceStatusReport,
    args: &RnstatusArgs,
    now: f64,
) {
    if args.announce_stats {
        if let Some(queued) = status
            .announce_queue
            .value()
            .copied()
            .filter(|count| *count > 0)
        {
            let suffix = if queued == 1 { "announce" } else { "announces" };
            let _ = writeln!(output, "    Queued    : {queued} {suffix}");
        }
        if let Some(held) = status
            .held_announces
            .value()
            .copied()
            .filter(|count| *count > 0)
        {
            let suffix = if held == 1 { "announce" } else { "announces" };
            let _ = writeln!(output, "    Held      : {held} {suffix}");
        }
    }
    render_path_request_rates(output, status, args, now);
    render_announce_rates(output, status, args, now);
}

fn render_path_request_rates(
    output: &mut String,
    status: &RnsInterfaceStatusReport,
    args: &RnstatusArgs,
    now: f64,
) {
    if !args.path_request_stats {
        return;
    }
    let (Some(incoming), Some(outgoing)) = (
        status.incoming_path_request_frequency.value(),
        status.outgoing_path_request_frequency.value(),
    ) else {
        return;
    };
    let clients = effective_clients(status);
    let displayed_outgoing = subtract_own_shared_rate(status, *outgoing, clients);
    let per_client = per_client_rate(*outgoing, clients, status.clients.value().is_none());
    let burst = active_for(
        &status.path_request_burst_active,
        &status.path_request_burst_activated_at,
        now,
    );
    let _ = writeln!(
        output,
        "    Path Rqs. : {}↑  {per_client}",
        pretty_frequency(displayed_outgoing)
    );
    let _ = writeln!(
        output,
        "                {}↓  {burst}",
        pretty_frequency(*incoming)
    );
}

fn render_announce_rates(
    output: &mut String,
    status: &RnsInterfaceStatusReport,
    args: &RnstatusArgs,
    now: f64,
) {
    if !args.announce_stats {
        return;
    }
    let (Some(incoming), Some(outgoing)) = (
        status.incoming_announce_frequency.value(),
        status.outgoing_announce_frequency.value(),
    ) else {
        return;
    };
    let clients = effective_clients(status);
    let displayed_outgoing = subtract_own_shared_rate(status, *outgoing, clients);
    let per_client = per_client_rate(*outgoing, clients, status.clients.value().is_none());
    let policy = announce_policy(status);
    let burst = active_for(&status.burst_active, &status.burst_activated_at, now);
    let _ = writeln!(
        output,
        "    Announces : {}↑  {per_client}",
        pretty_frequency(displayed_outgoing)
    );
    let _ = writeln!(
        output,
        "                {}↓ {policy}{burst}",
        pretty_frequency(*incoming)
    );
}

fn effective_clients(status: &RnsInterfaceStatusReport) -> Option<u64> {
    status
        .clients
        .value()
        .copied()
        .or_else(|| status.peers.value().copied().filter(|peers| *peers > 0))
}

fn subtract_own_shared_rate(
    status: &RnsInterfaceStatusReport,
    outgoing: f64,
    clients: Option<u64>,
) -> f64 {
    if status.name.starts_with("Shared Instance[") {
        if let Some(clients) = clients.filter(|clients| *clients > 0) {
            return outgoing - outgoing / clients as f64;
        }
    }
    outgoing
}

fn per_client_rate(outgoing: f64, clients: Option<u64>, peers: bool) -> String {
    clients
        .filter(|clients| *clients > 0)
        .map_or_else(String::new, |clients| {
            format!(
                "{}/{kind}",
                pretty_frequency(outgoing / clients as f64),
                kind = if peers { "p" } else { "c" }
            )
        })
}

fn announce_policy(status: &RnsInterfaceStatusReport) -> String {
    let target = status.announce_rate_target_seconds.value();
    let penalty = status.announce_rate_penalty_seconds.value();
    let grace = status.announce_rate_grace.value();
    match (target, penalty, grace) {
        (Some(target), Some(penalty), Some(grace)) if *target != 0.0 && *grace != 0.0 => format!(
            "(t:{}/p:{}/g:{})",
            pretty_time(*target, false),
            pretty_time(*penalty, false),
            number(*grace)
        ),
        (Some(target), Some(penalty), _) if *target != 0.0 => format!(
            "(t:{}/p:{})",
            pretty_time(*target, false),
            pretty_time(*penalty, false)
        ),
        (Some(target), _, _) if *target != 0.0 => {
            format!("(t:{})", pretty_time(*target, false))
        }
        _ => String::new(),
    }
}

fn active_for(
    active: &RnsOptionalField<bool>,
    activated_at: &RnsOptionalField<f64>,
    now: f64,
) -> String {
    if !matches!(active, RnsOptionalField::Value(true)) {
        return String::new();
    }
    activated_at.value().map_or_else(
        || String::from("burst active"),
        |activated| {
            format!(
                "burst for {}",
                pretty_time((now - activated).max(0.0), false)
            )
        },
    )
}

fn render_traffic(output: &mut String, status: &RnsInterfaceStatusReport) {
    let tx = format!(
        "↑{}  {}",
        pretty_size(status.transmit_bytes as f64, "B"),
        pretty_speed(status.transmit_speed_bps)
    );
    let rx = format!(
        "↓{}  {}",
        pretty_size(status.receive_bytes as f64, "B"),
        pretty_speed(status.receive_speed_bps)
    );
    let _ = writeln!(output, "    Traffic   : {tx}\n                {rx}");
}

fn render_footer(
    output: &mut String,
    report: &RnsInterfaceStatsReport,
    link_count: Option<u64>,
    args: &RnstatusArgs,
) {
    if args.totals {
        let tx = format!(
            "↑{}  {}",
            pretty_size(report.transmit_bytes as f64, "B"),
            pretty_speed(report.transmit_speed_bps)
        );
        let rx = format!(
            "↓{}  {}",
            pretty_size(report.receive_bytes as f64, "B"),
            pretty_speed(report.receive_speed_bps)
        );
        let _ = writeln!(output, "\n Totals       : {tx}\n                {rx}");
    }
    let link_text = link_count.map(link_count_text).unwrap_or_default();
    if let Some(identity) = report.transport_identity.value() {
        let _ = writeln!(
            output,
            "\n Transport Instance <{}> running",
            hex(identity.as_bytes())
        );
        if let Some(version) = report.software_version.value() {
            let _ = writeln!(output, " Software           {version}");
        }
        if let Some(identity) = report.network_identity.value() {
            let _ = writeln!(output, " Network Identity   <{}>", hex(identity.as_bytes()));
        }
        if let Some(destination) = report.probe_responder.value() {
            let _ = writeln!(
                output,
                " Probe destination <{}> (rnstransport.probe)",
                hex(destination.as_bytes())
            );
        }
        if let Some(uptime) = report.transport_uptime_seconds.value() {
            let _ = writeln!(
                output,
                " Uptime is {}{link_text}",
                pretty_time(*uptime, false)
            );
        } else if !link_text.is_empty() {
            let _ = writeln!(output, "{link_text}");
        }
    } else if !link_text.is_empty() {
        let _ = writeln!(output, "\n{link_text}");
    }
}

fn link_count_text(link_count: u64) -> String {
    let suffix = if link_count == 1 { "entry" } else { "entries" };
    format!(", {link_count} {suffix} in link table")
}

fn render_optional_number(
    output: &mut String,
    prefix: &str,
    suffix: &str,
    value: &RnsOptionalField<f64>,
) {
    if !value.is_present() {
        return;
    }
    match value.value() {
        Some(value) => {
            let _ = writeln!(output, "{prefix}{}{suffix}", number(*value));
        }
        None => {
            let _ = writeln!(output, "{prefix}Unknown");
        }
    }
}

fn render_optional_or_unknown(output: &mut String, prefix: &str, value: &RnsOptionalField<String>) {
    if !value.is_present() {
        return;
    }
    let _ = writeln!(
        output,
        "{prefix}{}",
        value.value().map_or("Unknown", String::as_str)
    );
}

fn write_optional(
    output: &mut String,
    prefix: &str,
    suffix: &str,
    value: &RnsOptionalField<String>,
) {
    if let Some(value) = value.value() {
        let _ = writeln!(output, "{prefix}{value}{suffix}");
    }
}

fn optional_number(value: &RnsOptionalField<f64>) -> f64 {
    value.value().copied().unwrap_or(0.0)
}

fn optional_u64(value: &RnsOptionalField<u64>) -> u64 {
    value.value().copied().unwrap_or(0)
}

fn pretty_speed(value: f64) -> String {
    pretty_size(value, "b") + "ps"
}

fn speed_str(mut value: f64) -> String {
    for unit in ["", "k", "M", "G", "T", "P", "E", "Z"] {
        if value.abs() < 1_000.0 {
            return format!("{value:3.2} {unit}bps");
        }
        value /= 1_000.0;
    }
    format!("{value:.2} Ybps")
}

fn pretty_size(mut value: f64, suffix: &str) -> String {
    for unit in ["", "K", "M", "G", "T", "P", "E", "Z"] {
        if value.abs() < 1_000.0 {
            return if unit.is_empty() {
                format!("{value:.0} {suffix}")
            } else {
                format!("{value:.2} {unit}{suffix}")
            };
        }
        value /= 1_000.0;
    }
    format!("{value:.2}Y{suffix}")
}

fn pretty_frequency(mut value: f64) -> String {
    if value == 0.0 {
        return String::from("0 Hz");
    }
    for unit in ["", "K", "M", "G", "T", "P", "E", "Z"] {
        if value.abs() < 1_000.0 {
            return format!("{} {unit}Hz", round_one(value));
        }
        value /= 1_000.0;
    }
    format!("{value:.2}YHz")
}

fn round_one(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    number(rounded)
}

pub(super) fn pretty_time(value: f64, compact: bool) -> String {
    let mut remaining = value.abs();
    let days = (remaining / 86_400.0).floor() as u64;
    remaining %= 86_400.0;
    let hours = (remaining / 3_600.0).floor() as u64;
    remaining %= 3_600.0;
    let minutes = (remaining / 60.0).floor() as u64;
    remaining %= 60.0;
    let seconds = if compact {
        remaining.floor()
    } else {
        (remaining * 100.0).round() / 100.0
    };
    let mut components = Vec::new();
    for (value, suffix) in [
        (days as f64, "d"),
        (hours as f64, "h"),
        (minutes as f64, "m"),
        (seconds, "s"),
    ] {
        if value > 0.0 && (!compact || components.len() < 2) {
            components.push(format!("{}{suffix}", number(value)));
        }
    }
    let joined = match components.as_slice() {
        [] => String::from("0s"),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let last = components.pop().unwrap_or_default();
            format!("{} and {last}", components.join(", "))
        }
    };
    if value < 0.0 {
        format!("-{joined}")
    } else {
        joined
    }
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_style_units_use_decimal_thresholds() {
        assert_eq!(pretty_size(999.0, "B"), "999 B");
        assert_eq!(pretty_size(1_000.0, "B"), "1.00 KB");
        assert_eq!(pretty_speed(8_000.0), "8.00 Kbps");
        assert_eq!(speed_str(1_000_000_000.0), "1.00 Gbps");
    }

    #[test]
    fn compact_time_stops_after_two_nonzero_components() {
        assert_eq!(pretty_time(90_061.0, true), "1d and 1h");
        assert_eq!(pretty_time(61.5, false), "1m and 1.5s");
    }

    #[test]
    fn burst_filter_requires_activity_unless_an_explicit_name_matches() {
        assert!(!selection_matches("LAN", None, true, false));
        assert!(selection_matches("LAN", None, true, true));
        assert!(selection_matches("LAN", Some("lan"), true, false));
        assert!(!selection_matches("LAN", Some("radio"), true, false));
        assert!(selection_matches("LAN", None, false, false));
    }

    #[test]
    fn signed_nonzero_gravity_is_visible_and_zero_or_absent_is_quiet() {
        let mut output = String::new();
        render_gravity(&mut output, &RnsOptionalField::Value(-7));
        render_gravity(&mut output, &RnsOptionalField::Value(0));
        render_gravity(&mut output, &RnsOptionalField::Absent);

        assert_eq!(output, "    Gravity   : -7\n");
        assert_eq!(optional_i64(&RnsOptionalField::Value(-7)), -7);
        assert_eq!(optional_i64(&RnsOptionalField::Absent), 0);
    }

    #[test]
    fn nested_fleet_peers_render_under_the_supervisor() {
        let status = RnsInterfaceStatusReport {
            name: String::from("Bluetooth Auto"),
            short_name: RnsOptionalField::Absent,
            interface_type: RnsOptionalField::Absent,
            interface_hash: RnsOptionalField::Absent,
            parent_name: RnsOptionalField::Absent,
            parent_hash: RnsOptionalField::Absent,
            online: true,
            mode: personal_rns::interfaces::rns_management::RnsInterfaceMode::Full,
            gravity: RnsOptionalField::Absent,
            clients: RnsOptionalField::Absent,
            receive_bytes: 100,
            transmit_bytes: 50,
            receive_speed_bps: 0.0,
            transmit_speed_bps: 0.0,
            bitrate_bps: RnsOptionalField::Absent,
            peers: RnsOptionalField::Absent,
            ifac_signature: RnsOptionalField::Absent,
            ifac_size_bytes: RnsOptionalField::Absent,
            ifac_network_name: RnsOptionalField::Absent,
            group_id: RnsOptionalField::Value(String::from("reticulum")),
            autoconnect_source: RnsOptionalField::Absent,
            announce_queue: RnsOptionalField::Absent,
            held_announces: RnsOptionalField::Absent,
            incoming_announce_frequency: RnsOptionalField::Absent,
            outgoing_announce_frequency: RnsOptionalField::Absent,
            incoming_path_request_frequency: RnsOptionalField::Absent,
            outgoing_path_request_frequency: RnsOptionalField::Absent,
            announce_rate_target_seconds: RnsOptionalField::Absent,
            announce_rate_penalty_seconds: RnsOptionalField::Absent,
            announce_rate_grace: RnsOptionalField::Absent,
            burst_active: RnsOptionalField::Absent,
            burst_activated_at: RnsOptionalField::Absent,
            path_request_burst_active: RnsOptionalField::Absent,
            path_request_burst_activated_at: RnsOptionalField::Absent,
            i2p_connectable: RnsOptionalField::Absent,
            i2p_b32: RnsOptionalField::Absent,
            i2p_tunnel_state: RnsOptionalField::Absent,
            airtime_short_percent: RnsOptionalField::Absent,
            airtime_long_percent: RnsOptionalField::Absent,
            channel_load_short_percent: RnsOptionalField::Absent,
            channel_load_long_percent: RnsOptionalField::Absent,
            noise_floor_dbm: RnsOptionalField::Absent,
            interference_dbm: RnsOptionalField::Absent,
            interference_last_at: RnsOptionalField::Absent,
            interference_last_dbm: RnsOptionalField::Absent,
            cpu_load_percent: RnsOptionalField::Absent,
            cpu_temperature_celsius: RnsOptionalField::Absent,
            memory_load_percent: RnsOptionalField::Absent,
            battery_percent: RnsOptionalField::Absent,
            battery_state: RnsOptionalField::Absent,
            switch_id: RnsOptionalField::Absent,
            endpoint_id: RnsOptionalField::Absent,
            via_switch_id: RnsOptionalField::Absent,
            blocked_ip_list: RnsOptionalField::Absent,
            rssi: RnsOptionalField::Absent,
            fleet_peers: vec![
                personal_rns::interfaces::rns_management::RnsFleetPeerReport {
                    name: String::from("ab12… @ AA:BB:CC:DD:EE:FF"),
                    online: true,
                    receive_bytes: 40,
                    transmit_bytes: 20,
                    receive_speed_bps: 0.0,
                    transmit_speed_bps: 0.0,
                    rssi: RnsOptionalField::Value(-61),
                },
            ],
        };
        let mut output = String::new();
        write_optional(&mut output, "    Group     : ", "", &status.group_id);
        render_fleet_peers(&mut output, &status);
        assert!(output.contains("Group     : reticulum"));
        assert!(output.contains("Peers     : 1 connected"));
        assert!(output.contains("ab12… @ AA:BB:CC:DD:EE:FF  Up  rssi -61"));
        assert!(output.contains("Traffic"));
    }
}
