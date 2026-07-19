use std::fmt::Write;

use personal_rns::identity::IdentityHash;
use personal_rns::interfaces::rns_management::{RnsAnnounceRateEntry, RnsPathTableEntry};
use personal_rns::routing::{BlackholeExpiry, BlackholedIdentity};
use serde_json::{Map, Value};
use time::{OffsetDateTime, UtcOffset};

pub fn path_table(entries: &[RnsPathTableEntry], json: bool) -> Result<String, serde_json::Error> {
    if json {
        return json_line(entries.iter().map(path_value).collect());
    }
    let mut output = String::new();
    for entry in entries {
        let plural = if entry.hops() == 1 { "" } else { "s" };
        let _ = writeln!(
            output,
            "{} is {} hop{} away via {} on {} expires {}",
            pretty_hex(entry.destination().as_bytes()),
            entry.hops(),
            plural,
            pretty_hex(entry.via().as_bytes()),
            entry.interface(),
            timestamp_str(entry.expires_at_seconds()),
        );
    }
    Ok(output)
}

pub fn rates(
    entries: &[RnsAnnounceRateEntry],
    json: bool,
    now: f64,
) -> Result<String, serde_json::Error> {
    if json {
        return json_line(entries.iter().map(rate_value).collect());
    }
    if entries.is_empty() {
        return Ok(String::from("No information available\n"));
    }
    let mut output = String::new();
    for entry in entries {
        let Some(start) = entry.observed_at_seconds().first().copied() else {
            let _ = writeln!(
                output,
                "Error while processing entry for {}",
                pretty_hex(entry.destination().as_bytes())
            );
            output.push_str("announce history is empty\n");
            continue;
        };
        let span = (now - start).max(3_600.0);
        let hourly = round_three(entry.observed_at_seconds().len() as f64 / (span / 3_600.0));
        let violations = match entry.rate_violations() {
            0 => String::new(),
            1 => String::from(", 1 active rate violation"),
            count => format!(", {count} active rate violations"),
        };
        let blocked = if entry.blocked_until_seconds() > now {
            let synthetic_timestamp = now - (entry.blocked_until_seconds() - now);
            format!(
                ", new announces allowed in {}",
                pretty_date(synthetic_timestamp, now)
            )
        } else {
            String::new()
        };
        let _ = writeln!(
            output,
            "{} last heard {} ago, {} announces/hour in the last {}{}{}",
            pretty_hex(entry.destination().as_bytes()),
            pretty_date(entry.last_allowed_announce_at_seconds(), now),
            number(hourly),
            pretty_date(start, now),
            violations,
            blocked,
        );
    }
    Ok(output)
}

pub fn blackholes(
    entries: &[BlackholedIdentity<String>],
    filter: Option<&str>,
    local_transport: IdentityHash,
    now_seconds: f64,
) -> (String, usize) {
    let mut output = String::new();
    let mut displayed = 0usize;
    for entry in entries {
        let until = match entry.expiry {
            BlackholeExpiry::Indefinite => String::from("indefinitely"),
            BlackholeExpiry::At(at) => format!(
                "for {}",
                pretty_time((at.0 as f64 / 1_000.0 - now_seconds).max(0.0))
            ),
        };
        let reason = entry
            .reason
            .as_deref()
            .filter(|reason| !reason.is_empty())
            .map_or_else(String::new, |reason| format!(" ({})", truncate(reason)));
        let by = if entry.source == local_transport {
            String::new()
        } else {
            format!(" by {}", pretty_hex(entry.source.as_bytes()))
        };
        let line = format!(
            "{} blackholed {until}{reason}{by}",
            pretty_hex(entry.identity.as_bytes())
        );
        if filter.is_some_and(|filter| !line.contains(filter)) {
            continue;
        }
        let _ = writeln!(output, "{line}");
        displayed = displayed.saturating_add(1);
    }
    (output, displayed)
}

pub fn found_path(destination: &[u8], hops: u8, next_hop: &[u8], interface: &str) -> String {
    let plural = if hops == 1 { "" } else { "s" };
    format!(
        "Path found, destination {} is {} hop{} away via {} on {}\n",
        pretty_hex(destination),
        hops,
        plural,
        pretty_hex(next_hop),
        interface,
    )
}

pub fn pretty_hex(bytes: &[u8]) -> String {
    format!("<{}>", hex(bytes))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn path_value(entry: &RnsPathTableEntry) -> Value {
    let mut fields = Map::new();
    fields.insert(
        String::from("hash"),
        hex(entry.destination().as_bytes()).into(),
    );
    fields.insert(
        String::from("timestamp"),
        number_value(entry.learned_at_seconds()),
    );
    fields.insert(String::from("via"), hex(entry.via().as_bytes()).into());
    fields.insert(String::from("hops"), entry.hops().into());
    fields.insert(
        String::from("expires"),
        number_value(entry.expires_at_seconds()),
    );
    fields.insert(String::from("interface"), entry.interface().into());
    Value::Object(fields)
}

fn rate_value(entry: &RnsAnnounceRateEntry) -> Value {
    let mut fields = Map::new();
    fields.insert(
        String::from("hash"),
        hex(entry.destination().as_bytes()).into(),
    );
    fields.insert(
        String::from("last"),
        number_value(entry.last_allowed_announce_at_seconds()),
    );
    fields.insert(
        String::from("rate_violations"),
        entry.rate_violations().into(),
    );
    fields.insert(
        String::from("blocked_until"),
        number_value(entry.blocked_until_seconds()),
    );
    fields.insert(
        String::from("timestamps"),
        Value::Array(
            entry
                .observed_at_seconds()
                .iter()
                .copied()
                .map(number_value)
                .collect(),
        ),
    );
    Value::Object(fields)
}

fn json_line(values: Vec<Value>) -> Result<String, serde_json::Error> {
    serde_json::to_string(&values).map(|mut output| {
        output.push('\n');
        output
    })
}

fn number_value(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn timestamp_str(seconds: f64) -> String {
    let Ok(timestamp) = OffsetDateTime::from_unix_timestamp(seconds as i64) else {
        return number(seconds);
    };
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let local = timestamp.to_offset(offset);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.year(),
        local.month() as u8,
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
    )
}

fn pretty_date(timestamp: f64, now: f64) -> String {
    let difference = now as i64 - timestamp as i64;
    let days = difference.div_euclid(86_400);
    if days < 0 {
        return String::new();
    }
    let seconds = difference.rem_euclid(86_400);
    if days == 0 {
        if seconds < 60 {
            return format!("{seconds} seconds");
        }
        if seconds < 70 {
            return String::from("1 minute");
        }
        if seconds < 7_200 {
            return format!("{} minutes", seconds / 60);
        }
        return format!("{} hours", seconds / 3_600);
    }
    if days == 1 {
        return String::from("1 day");
    }
    if days < 7 {
        return format!("{days} days");
    }
    if days < 31 {
        return format!("{} weeks", days / 7);
    }
    if days < 365 {
        return format!("{} months", days / 30);
    }
    format!("{} years", days / 365)
}

fn pretty_time(value: f64) -> String {
    let mut remaining = value.abs();
    let days = (remaining / 86_400.0).floor() as u64;
    remaining %= 86_400.0;
    let hours = (remaining / 3_600.0).floor() as u64;
    remaining %= 3_600.0;
    let minutes = (remaining / 60.0).floor() as u64;
    remaining %= 60.0;
    let seconds = (remaining * 100.0).round() / 100.0;
    let mut components = Vec::new();
    for (quantity, unit) in [
        (days as f64, "day"),
        (hours as f64, "hour"),
        (minutes as f64, "minute"),
        (seconds, "second"),
    ] {
        if quantity > 0.0 {
            let plural = if quantity == 1.0 { "" } else { "s" };
            components.push(format!("{} {unit}{plural}", number(quantity)));
        }
    }
    match components.as_slice() {
        [] => String::from("0s"),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let last = components.pop().unwrap_or_default();
            format!("{} and {last}", components.join(", "))
        }
    }
}

fn truncate(value: &str) -> String {
    if value.chars().count() <= 64 {
        return value.to_owned();
    }
    value.chars().take(63).chain(std::iter::once('…')).collect()
}

fn round_three(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_relative_dates_keep_stock_boundaries() {
        let now = 1_700_000_000.0;
        assert_eq!(pretty_date(now - 69.0, now), "1 minute");
        assert_eq!(pretty_date(now - 70.0, now), "1 minutes");
        assert_eq!(pretty_date(now - 86_400.0, now), "1 day");
    }

    #[test]
    fn published_reasons_use_the_stock_display_limit() {
        assert_eq!(truncate(&"x".repeat(65)), format!("{}…", "x".repeat(63)));
    }

    #[test]
    fn zero_duration_matches_stock_prettytime() {
        assert_eq!(pretty_time(0.0), "0s");
    }
}
