//! Wired client ingress policy for a headless router.
use personal_rns::interfaces::{tcp, EffectiveInterfacePolicy, InterfaceMode};

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum TcpMode {
    #[default]
    PointToPoint,
    Gateway,
}

impl TcpMode {
    pub fn policy(self) -> EffectiveInterfacePolicy {
        let mut policy = tcp::policy_for_bitrate(tcp::TCP_BITRATE_ESTIMATE);
        policy.mode = match self {
            Self::PointToPoint => InterfaceMode::PointToPoint,
            Self::Gateway => InterfaceMode::Gateway,
        };
        policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_enables_recursive_discovery_without_changing_other_tcp_budgets() {
        let peer = TcpMode::default().policy();
        assert!(!peer.mode.recursively_forwards_unknown_paths());
        let gateway = TcpMode::Gateway.policy();
        assert!(gateway.mode.recursively_forwards_unknown_paths());
        let mut expected = peer;
        expected.mode = InterfaceMode::Gateway;
        assert_eq!(gateway, expected);
    }
}
