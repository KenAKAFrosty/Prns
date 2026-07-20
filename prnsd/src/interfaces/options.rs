use prns_config::editing::{InterfaceSetting, InterfaceSettingKey, InterfaceSettingValue};
use prns_config::InterfaceKind;

use super::error::InterfacesError;
use super::InterfaceOptions;

impl InterfaceOptions {
    pub(super) fn settings(
        self,
        kind: InterfaceKind,
    ) -> Result<Vec<InterfaceSetting>, InterfacesError> {
        let mut values = Vec::new();
        push(&mut values, "interface_mode", text(self.mode))?;
        push(&mut values, "outgoing", boolean(self.outgoing))?;
        push(&mut values, "bitrate", unsigned(self.bitrate))?;
        push(&mut values, "announce_cap", decimal(self.announce_cap))?;
        push(
            &mut values,
            "announce_rate_target",
            unsigned(self.announce_rate_target),
        )?;
        push(
            &mut values,
            "announce_rate_grace",
            unsigned(self.announce_rate_grace),
        )?;
        push(
            &mut values,
            "announce_rate_penalty",
            unsigned(self.announce_rate_penalty),
        )?;
        push(&mut values, "network_name", text(self.network_name))?;
        push(&mut values, "pass_phrase", text(self.pass_phrase))?;
        push(&mut values, "ifac_size", unsigned(self.ifac_size))?;
        push(&mut values, "discoverable", boolean(self.discoverable))?;
        push(
            &mut values,
            "announce_interval",
            signed(self.announce_interval),
        )?;
        push(
            &mut values,
            "discovery_stamp_value",
            unsigned(self.discovery_stamp_value.map(u64::from)),
        )?;
        push(&mut values, "discovery_name", text(self.discovery_name))?;
        push(
            &mut values,
            "discovery_encrypt",
            boolean(self.discovery_encrypt),
        )?;
        push(&mut values, "reachable_on", text(self.reachable_on))?;
        push(&mut values, "publish_ifac", boolean(self.publish_ifac))?;
        push(&mut values, "latitude", decimal(self.latitude))?;
        push(&mut values, "longitude", decimal(self.longitude))?;
        push(&mut values, "height", decimal(self.height))?;
        push(
            &mut values,
            "discovery_frequency",
            unsigned(self.discovery_frequency),
        )?;
        push(
            &mut values,
            "discovery_bandwidth",
            unsigned(self.discovery_bandwidth.map(u64::from)),
        )?;
        push(
            &mut values,
            "discovery_modulation",
            text(self.discovery_modulation),
        )?;
        push(
            &mut values,
            "ingress_control",
            boolean(self.ingress_control),
        )?;
        push(&mut values, "egress_control", boolean(self.egress_control))?;
        push(
            &mut values,
            "ic_max_held_announces",
            signed(self.ic_max_held_announces),
        )?;
        push(&mut values, "ic_burst_hold", decimal(self.ic_burst_hold))?;
        push(
            &mut values,
            "ic_burst_freq_new",
            decimal(self.ic_burst_freq_new),
        )?;
        push(&mut values, "ic_burst_freq", decimal(self.ic_burst_freq))?;
        push(
            &mut values,
            "ic_pr_burst_freq_new",
            decimal(self.ic_pr_burst_freq_new),
        )?;
        push(
            &mut values,
            "ic_pr_burst_freq",
            decimal(self.ic_pr_burst_freq),
        )?;
        push(&mut values, "ec_pr_freq", decimal(self.ec_pr_freq))?;
        push(&mut values, "ic_new_time", decimal(self.ic_new_time))?;
        push(
            &mut values,
            "ic_burst_penalty",
            decimal(self.ic_burst_penalty),
        )?;
        push(
            &mut values,
            "ic_held_release_interval",
            decimal(self.ic_held_release_interval),
        )?;
        push(&mut values, "bootstrap_only", boolean(self.bootstrap_only))?;
        push(&mut values, "recursive_prs", boolean(self.recursive_prs))?;
        push(
            &mut values,
            "announces_from_internal",
            boolean(self.announces_from_internal),
        )?;
        push(
            &mut values,
            "ignore_config_warnings",
            boolean(self.ignore_config_warnings),
        )?;
        push(&mut values, "group_id", text(self.group_id))?;
        push(&mut values, "discovery_scope", text(self.discovery_scope))?;
        push(
            &mut values,
            "discovery_port",
            unsigned(self.discovery_port.map(u64::from)),
        )?;
        push(
            &mut values,
            "data_port",
            unsigned(self.data_port.map(u64::from)),
        )?;
        push(&mut values, "devices", list(self.devices))?;
        push(&mut values, "ignored_devices", list(self.ignored_devices))?;
        push(
            &mut values,
            "multicast_address_type",
            text(self.multicast_address_type),
        )?;
        push(&mut values, "target_host", text(self.target_host))?;
        push(
            &mut values,
            "target_port",
            unsigned(self.target_port.map(u64::from)),
        )?;
        push(&mut values, "target", text(self.target))?;
        push(&mut values, "kiss_framing", boolean(self.kiss_framing))?;
        push(&mut values, "i2p_tunneled", boolean(self.i2p_tunneled))?;
        push(&mut values, "listen_ip", text(self.listen_ip))?;
        push(
            &mut values,
            "listen_port",
            unsigned(self.listen_port.map(u64::from)),
        )?;
        push(&mut values, "forward_ip", text(self.forward_ip))?;
        push(
            &mut values,
            "forward_port",
            unsigned(self.forward_port.map(u64::from)),
        )?;
        push(&mut values, "device", text(self.device))?;
        push(&mut values, "port", port(kind, self.port)?)?;
        push(&mut values, "prefer_ipv6", boolean(self.prefer_ipv6))?;
        push(&mut values, "speed", unsigned(self.speed.map(u64::from)))?;
        push(
            &mut values,
            "databits",
            unsigned(self.databits.map(u64::from)),
        )?;
        push(&mut values, "parity", text(self.parity))?;
        push(
            &mut values,
            "stopbits",
            unsigned(self.stopbits.map(u64::from)),
        )?;
        push(&mut values, "flow_control", boolean(self.flow_control))?;
        push(
            &mut values,
            "preamble",
            unsigned(self.preamble.map(u64::from)),
        )?;
        push(&mut values, "txtail", unsigned(self.txtail.map(u64::from)))?;
        push(
            &mut values,
            "persistence",
            unsigned(self.persistence.map(u64::from)),
        )?;
        push(
            &mut values,
            "slottime",
            unsigned(self.slottime.map(u64::from)),
        )?;
        push(&mut values, "id_callsign", text(self.id_callsign))?;
        push(&mut values, "id_interval", unsigned(self.id_interval))?;
        push(&mut values, "callsign", text(self.callsign))?;
        push(&mut values, "ssid", unsigned(self.ssid.map(u64::from)))?;
        push(&mut values, "frequency", unsigned(self.frequency))?;
        push(
            &mut values,
            "bandwidth",
            unsigned(self.bandwidth.map(u64::from)),
        )?;
        push(
            &mut values,
            "spreadingfactor",
            unsigned(self.spreading_factor.map(u64::from)),
        )?;
        push(
            &mut values,
            "codingrate",
            unsigned(self.coding_rate.map(u64::from)),
        )?;
        push(
            &mut values,
            "txpower",
            self.txpower
                .map(i64::from)
                .map(InterfaceSettingValue::Signed),
        )?;
        push(
            &mut values,
            "airtime_limit_short",
            decimal(self.airtime_limit_short),
        )?;
        push(
            &mut values,
            "airtime_limit_long",
            decimal(self.airtime_limit_long),
        )?;
        push(&mut values, "command", text(self.command))?;
        push(&mut values, "respawn_delay", decimal(self.respawn_delay))?;
        push(&mut values, "peers", list(self.peers))?;
        push(&mut values, "connectable", boolean(self.connectable))?;
        push(
            &mut values,
            "connect_timeout",
            unsigned(self.connect_timeout),
        )?;
        push(
            &mut values,
            "max_reconnect_tries",
            unsigned(self.max_reconnect_tries.map(u64::from)),
        )?;
        push(
            &mut values,
            "fixed_mtu",
            unsigned(self.fixed_mtu.map(u64::from)),
        )?;
        push(&mut values, "remote", text(self.remote))?;
        push(&mut values, "listen_on", text(self.listen_on))?;
        if let Some(setting) = values
            .iter()
            .find(|setting| !kind.accepts_setting(setting.key().as_str()))
        {
            return Err(InterfacesError::InapplicableSetting {
                key: setting.key().as_str(),
                kind,
            });
        }
        Ok(values)
    }
}

fn text(value: Option<String>) -> Option<InterfaceSettingValue> {
    value.map(InterfaceSettingValue::Text)
}

fn boolean(value: Option<bool>) -> Option<InterfaceSettingValue> {
    value.map(InterfaceSettingValue::Bool)
}

fn unsigned(value: Option<u64>) -> Option<InterfaceSettingValue> {
    value.map(InterfaceSettingValue::Unsigned)
}

fn signed(value: Option<i64>) -> Option<InterfaceSettingValue> {
    value.map(InterfaceSettingValue::Signed)
}

fn decimal(value: Option<f64>) -> Option<InterfaceSettingValue> {
    value.map(InterfaceSettingValue::Decimal)
}

fn list(value: Option<Vec<String>>) -> Option<InterfaceSettingValue> {
    value.map(InterfaceSettingValue::List)
}

fn port(
    kind: InterfaceKind,
    value: Option<String>,
) -> Result<Option<InterfaceSettingValue>, InterfacesError> {
    let numeric = matches!(
        kind,
        InterfaceKind::TcpServer
            | InterfaceKind::Udp
            | InterfaceKind::Backbone
            | InterfaceKind::BackboneClient
            | InterfaceKind::PrnsWebSocketServer
    );
    match value {
        Some(value) if numeric => value
            .parse::<u16>()
            .map(u64::from)
            .map(InterfaceSettingValue::Unsigned)
            .map(Some)
            .map_err(|_| InterfacesError::InvalidPort(kind)),
        Some(value) => Ok(Some(InterfaceSettingValue::Text(value))),
        None => Ok(None),
    }
}

fn push(
    settings: &mut Vec<InterfaceSetting>,
    key: &'static str,
    value: Option<InterfaceSettingValue>,
) -> Result<(), InterfacesError> {
    if let Some(value) = value {
        let key = InterfaceSettingKey::parse(key).ok_or(InterfacesError::UnknownSettingKey(key))?;
        settings.push(InterfaceSetting::new(key, value));
    }
    Ok(())
}
