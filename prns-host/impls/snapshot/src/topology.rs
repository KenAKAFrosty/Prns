use super::*;
use personal_rns::interfaces::{InterfaceKind as RuntimeKind, InterfaceOriginKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostConnectionRole {
    Independent,
    Supervisor,
    Member,
}

/// Raw physical inventory plus explicit control/parent associations. Counters
/// are per row; a consumer must not sum them with the logical HostSnapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConnectionSnapshot {
    pub runtime_interface: prns_host::InterfaceId,
    pub logical_interface: prns_host::InterfaceId,
    pub attachment: Option<prns_host::InterfaceId>,
    pub parent: Option<prns_host::InterfaceId>,
    pub role: HostConnectionRole,
    pub kind: Option<InterfaceKind>,
    pub runtime_kind: Option<RuntimeKind>,
    pub origin: InterfaceOriginKind,
    pub attachment_epoch: u64,
    pub name: Option<String>,
    pub health: InterfaceHealth,
    pub failure_detail: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: Option<u64>,
    pub tx_bps: Option<u64>,
    /// Last observed wire peer. Configured DNS targets remain profile metadata.
    pub endpoint: Option<String>,
    pub radio: personal_rns::interfaces::RadioIndication,
    pub details: personal_rns::interfaces::PeerDetails,
}

pub fn detailed_interface_topology(
    raw: &[InterfaceInventoryEntry],
    attachments: impl IntoIterator<Item = HostInterfaceAttachment>,
) -> Result<Vec<HostConnectionSnapshot>, HostSnapshotAssemblyError> {
    let roots = raw
        .iter()
        .filter_map(|entry| match entry.snapshot.membership {
            Membership::FleetMember { supervisor_id } => Some((entry.snapshot.id, supervisor_id)),
            Membership::Independent => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut controls = BTreeMap::new();
    let mut kinds = BTreeMap::new();
    for attachment in attachments {
        let root = roots
            .get(&attachment.interface)
            .copied()
            .unwrap_or(attachment.interface);
        if controls
            .insert(root, attachment.host_interface)
            .is_some_and(|old| old != attachment.host_interface)
        {
            return Err(HostSnapshotAssemblyError::ConflictingLogicalInterface { interface: root });
        }
        if kinds
            .insert(attachment.host_interface, attachment.kind)
            .is_some_and(|old| old != attachment.kind)
        {
            return Err(HostSnapshotAssemblyError::ConflictingHostInterfaceKind {
                interface: attachment.host_interface,
            });
        }
    }
    let mut result = raw
        .iter()
        .map(|entry| {
            let state = &entry.snapshot;
            let parent = roots.get(&state.id).copied();
            let root = parent.unwrap_or(state.id);
            let control = controls.get(&root).copied();
            let runtime_kind = state.id.kind();
            let role = if parent.is_some() {
                HostConnectionRole::Member
            } else if roots.values().any(|id| *id == state.id)
                || matches!(
                    runtime_kind,
                    Some(
                        RuntimeKind::AutoWifi
                            | RuntimeKind::BluetoothAuto
                            | RuntimeKind::TcpServer
                            | RuntimeKind::BackboneServer
                            | RuntimeKind::WebSocketServer
                            | RuntimeKind::UsbAutoHost
                            | RuntimeKind::WifiDirect
                            | RuntimeKind::WifiAware
                            | RuntimeKind::I2p
                            | RuntimeKind::Weave
                            | RuntimeKind::WifiHaLow
                    )
                )
            {
                HostConnectionRole::Supervisor
            } else {
                HostConnectionRole::Independent
            };
            HostConnectionSnapshot {
                runtime_interface: host_interface(state.id),
                logical_interface: host_interface(root),
                attachment: control,
                parent: parent.map(host_interface),
                role,
                kind: control.and_then(|id| kinds.get(&id).copied()),
                runtime_kind,
                origin: entry.origin,
                attachment_epoch: entry.attachment_epoch,
                name: entry.name.clone(),
                health: host_interface_health(state.connection),
                failure_detail: state.failure_reason.map(str::to_owned),
                rx_bytes: state.rx_bytes,
                tx_bytes: state.tx_bytes,
                rx_bps: state.transfer_rates.map(|rates| u64::from(rates.rx_bps)),
                tx_bps: state.transfer_rates.map(|rates| u64::from(rates.tx_bps)),
                endpoint: entry.endpoint.map(|address| address.to_string()),
                radio: state.radio,
                details: state.details,
            }
        })
        .collect::<Vec<_>>();
    result.sort_by_key(|entry| entry.runtime_interface);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::{
        InterfaceGravity, InterfaceId, InterfaceMode, InterfaceSnapshot, PeerDetails,
        RadioIndication,
    };
    use personal_rns::node_introspection::FrameAccountingCoverage;

    fn entry(
        kind: RuntimeKind,
        tag: &[u8],
        membership: Membership,
        bytes: u64,
    ) -> InterfaceInventoryEntry {
        InterfaceInventoryEntry {
            name: None,
            endpoint: None,
            origin: InterfaceOriginKind::Configured,
            attachment_epoch: 7,
            frame_accounting: FrameAccountingCoverage::Unavailable,
            ifac: None,
            snapshot: InterfaceSnapshot {
                id: InterfaceId::from_channel_tag(kind, tag),
                mode: InterfaceMode::Full,
                gravity: InterfaceGravity::ZERO,
                connection: ConnectionState::Connected,
                failure_reason: None,
                rx_bytes: bytes,
                tx_bytes: 0,
                transfer_rates: None,
                destinations: 0,
                links: 0,
                transported_links: 0,
                membership,
                radio: RadioIndication::for_kind(Some(kind)),
                details: PeerDetails::NotApplicable,
            },
        }
    }

    #[test]
    fn physical_bluetooth_peer_details_survive_projection() {
        let mut peer = entry(
            RuntimeKind::BluetoothPeer,
            b"peer",
            Membership::Independent,
            3,
        );
        peer.snapshot.details = PeerDetails::BleCoc;
        let projected = detailed_interface_topology(&[peer.clone()], []).unwrap();
        assert_eq!(projected[0].radio, peer.snapshot.radio);
        assert_eq!(projected[0].details, PeerDetails::BleCoc);
        assert_eq!(projected[0].runtime_kind, Some(RuntimeKind::BluetoothPeer));
    }

    #[test]
    fn detailed_rows_retain_explicit_parentage_and_do_not_fold_traffic() {
        let parent = entry(RuntimeKind::AutoWifi, b"lan", Membership::Independent, 12);
        let child = entry(
            RuntimeKind::WifiPeer,
            b"peer",
            Membership::FleetMember {
                supervisor_id: parent.snapshot.id,
            },
            45,
        );
        let raw = vec![parent.clone(), child.clone()];
        let control = prns_host::InterfaceId::new([0x61; 8]);
        let detail = detailed_interface_topology(
            &raw,
            [HostInterfaceAttachment::with_host_interface(
                parent.snapshot.id,
                control,
                InterfaceKind::AutoLan,
            )],
        )
        .expect("topology");
        let member = detail
            .iter()
            .find(|row| row.role == HostConnectionRole::Member)
            .expect("member");
        assert_eq!(member.parent, Some(host_interface(parent.snapshot.id)));
        assert_eq!(member.logical_interface, host_interface(parent.snapshot.id));
        assert_eq!(member.attachment, Some(control));
        assert_eq!(member.rx_bytes, 45);
        assert_eq!(member.attachment_epoch, 7);
        assert_eq!(member.runtime_kind, Some(RuntimeKind::WifiPeer));
        assert_eq!(
            detail
                .iter()
                .find(|row| row.role == HostConnectionRole::Supervisor)
                .expect("parent")
                .rx_bytes,
            12
        );
        assert_eq!(raw[1].snapshot.membership, child.snapshot.membership);
        assert!(member.endpoint.is_none());
    }

    #[test]
    fn orphan_member_is_preserved_without_inventing_an_attachment() {
        let parent = InterfaceId::from_channel_tag(RuntimeKind::AutoWifi, b"missing");
        let child = entry(
            RuntimeKind::WifiPeer,
            b"peer",
            Membership::FleetMember {
                supervisor_id: parent,
            },
            1,
        );
        let detail = detailed_interface_topology(&[child], []).expect("topology");
        assert_eq!(detail[0].parent, Some(host_interface(parent)));
        assert!(detail[0].attachment.is_none());
    }

    #[test]
    fn actual_loopback_endpoint_survives_projection_for_local_peer_filtering() {
        let mut local = entry(
            RuntimeKind::TcpClient,
            b"local-rendezvous",
            Membership::Independent,
            8,
        );
        local.endpoint = Some("127.0.0.1:42699".parse().expect("socket address"));
        let detail = detailed_interface_topology(&[local], []).expect("topology");
        assert_eq!(detail[0].endpoint.as_deref(), Some("127.0.0.1:42699"));
        assert_eq!(detail[0].role, HostConnectionRole::Independent);
    }
}
