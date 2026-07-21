use prns_config::{I2pPeersPlan, I2pReachabilityPlan};

use crate::i2p::{
    I2pInterface, I2pInterfaceConfig, I2pInterfaceName, I2pPeerAddress, I2pPeers, I2pReachability,
    I2pRetryPolicy, TokioSamBridge,
};

use super::{AttachmentResult, InterfaceConstruction, PlanFailure, PlanRuntimeContext};

pub(super) fn stand_up(
    construction: InterfaceConstruction<'_>,
    peers: &I2pPeersPlan,
    reachability: I2pReachabilityPlan,
    context: &PlanRuntimeContext,
) -> AttachmentResult {
    let config = runtime_config(construction.interface, peers, reachability, context)?;
    let i2p = I2pInterface::new(TokioSamBridge::default(), config);
    let attached = construction.attach(i2p);
    Ok(attached.id())
}

fn runtime_config(
    interface: &prns_config::PlannedInterface,
    planned_peers: &I2pPeersPlan,
    planned_reachability: I2pReachabilityPlan,
    context: &PlanRuntimeContext,
) -> Result<I2pInterfaceConfig, PlanFailure> {
    let name = I2pInterfaceName::new(interface.name.clone()).map_err(PlanFailure::from)?;
    let peers = planned_peers
        .iter()
        .map(|peer| I2pPeerAddress::new(peer.as_str()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(PlanFailure::from)?;
    let peers = I2pPeers::new(peers).map_err(PlanFailure::from)?;
    let reachability = match planned_reachability {
        I2pReachabilityPlan::OutboundOnly => I2pReachability::OutboundOnly,
        I2pReachabilityPlan::Connectable => {
            let storage = context
                .i2p_storage
                .as_ref()
                .ok_or(PlanFailure::MissingI2pStorage)?;
            I2pReachability::Connectable {
                key_path: storage.destination_key_path(&name),
            }
        }
    };
    Ok(I2pInterfaceConfig {
        name,
        peers,
        reachability,
        policy: interface.policy,
        retry: I2pRetryPolicy::STOCK,
    })
}

#[cfg(test)]
mod tests {
    use prns_config::PlannedMedium;
    use prns_core::identity::IdentityHash;

    use crate::i2p::{I2pPeerAddress, I2pReachability, I2pRetryPolicy};

    use super::runtime_config;
    use crate::from_plan::{PlanFailure, PlanRuntimeContext};

    #[test]
    fn planned_peers_cross_the_runtime_boundary_without_reinterpretation() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\npeers = example.i2p, QUJDRA==\n",
        )
        .expect("valid I2P configuration")
        .value;
        let interface = &plan.interfaces[0];
        let PlannedMedium::I2p {
            peers,
            reachability,
        } = &interface.medium
        else {
            panic!("I2P medium expected")
        };

        let config = runtime_config(
            interface,
            peers,
            *reachability,
            &PlanRuntimeContext::default(),
        )
        .expect("the typed plan converts to runtime types");

        assert_eq!(
            config
                .peers
                .iter()
                .map(I2pPeerAddress::as_str)
                .collect::<Vec<_>>(),
            vec!["example.i2p", "QUJDRA=="]
        );
        assert_eq!(config.reachability, I2pReachability::OutboundOnly);
        assert_eq!(config.policy, interface.policy);
        assert_eq!(config.retry, I2pRetryPolicy::STOCK);
    }

    #[test]
    fn connectable_requires_host_runtime_context() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\nconnectable = Yes\n",
        )
        .expect("valid I2P configuration")
        .value;
        let interface = &plan.interfaces[0];
        let PlannedMedium::I2p {
            peers,
            reachability,
        } = &interface.medium
        else {
            panic!("I2P medium expected")
        };

        let error = runtime_config(
            interface,
            peers,
            *reachability,
            &PlanRuntimeContext::default(),
        )
        .expect_err("connectable I2P needs persistent host context");

        assert!(matches!(error, PlanFailure::MissingI2pStorage));
    }

    #[test]
    fn connectable_uses_the_host_supplied_stock_key_scope() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\nconnectable = Yes\n",
        )
        .expect("valid I2P configuration")
        .value;
        let interface = &plan.interfaces[0];
        let PlannedMedium::I2p {
            peers,
            reachability,
        } = &interface.medium
        else {
            panic!("I2P medium expected")
        };
        let context = PlanRuntimeContext::with_rns_i2p_storage(
            "/var/lib/reticulum/storage",
            IdentityHash::new([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ]),
        );

        let config = runtime_config(interface, peers, *reachability, &context)
            .expect("the daemon context completes connectable I2P");
        let I2pReachability::Connectable { key_path } = config.reachability else {
            panic!("connectable runtime expected")
        };

        assert_eq!(
            key_path.as_path(),
            std::path::Path::new(
                "/var/lib/reticulum/storage/i2p/4c621c0110154bbe086a0395dbeb07878a1613258d5e0346c96ddef1a5aeae2d.i2p"
            )
        );
    }

    #[test]
    fn config_peer_validation_matches_runtime_peer_types() {
        for peer in [
            "example.i2p",
            "52chars.b32.i2p",
            "QUJDRA==",
            "EXAMPLE.I2P",
            "abc",
            "A=AA",
            "not a peer",
            "",
        ] {
            let config = format!(
                "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\npeers = {peer}\n"
            );
            assert_eq!(
                prns_config::parse_and_plan(&config).is_ok(),
                I2pPeerAddress::new(peer).is_ok(),
                "config and runtime must agree for {peer:?}"
            );
        }
    }
}
