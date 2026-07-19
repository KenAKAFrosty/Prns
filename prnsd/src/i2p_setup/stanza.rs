use std::fmt;

use personal_rns::i2p::{DuplicateI2pPeer, I2pPeerAddress, I2pPeers};

use super::SetupReachability;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct I2pInterfaceStanza {
    peers: I2pPeers,
    reachability: SetupReachability,
}

impl I2pInterfaceStanza {
    pub(super) fn new(
        peers: impl IntoIterator<Item = I2pPeerAddress>,
        reachability: SetupReachability,
    ) -> Result<Self, DuplicateI2pPeer> {
        Ok(Self {
            peers: I2pPeers::new(peers)?,
            reachability,
        })
    }

    pub(super) fn is_idle(&self) -> bool {
        self.peers.is_empty() && self.reachability == SetupReachability::OutboundOnly
    }

    pub(super) const fn is_connectable(&self) -> bool {
        matches!(self.reachability, SetupReachability::Connectable)
    }
}

impl fmt::Display for I2pInterfaceStanza {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "  [[I2P]]")?;
        writeln!(formatter, "    type = I2PInterface")?;
        writeln!(formatter, "    enabled = Yes")?;
        writeln!(
            formatter,
            "    connectable = {}",
            if self.is_connectable() { "Yes" } else { "No" }
        )?;
        let mut peers = self.peers.iter().peekable();
        if peers.peek().is_some() {
            formatter.write_str("    peers = ")?;
            while let Some(peer) = peers.next() {
                formatter.write_str(peer.as_str())?;
                if peers.peek().is_some() {
                    formatter.write_str(", ")?;
                }
            }
            formatter.write_str("\n")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitted_stanzas_are_valid_idle_outbound_and_connectable_shapes() {
        let idle = I2pInterfaceStanza::new(Vec::new(), SetupReachability::OutboundOnly).unwrap();
        assert!(idle.is_idle());
        assert_eq!(
            idle.to_string(),
            "  [[I2P]]\n    type = I2PInterface\n    enabled = Yes\n    connectable = No\n"
        );

        let configured = I2pInterfaceStanza::new(
            [
                I2pPeerAddress::new("one.i2p").unwrap(),
                I2pPeerAddress::new("two.i2p").unwrap(),
            ],
            SetupReachability::Connectable,
        )
        .unwrap();
        assert_eq!(
            configured.to_string(),
            "  [[I2P]]\n    type = I2PInterface\n    enabled = Yes\n    connectable = Yes\n    peers = one.i2p, two.i2p\n"
        );
    }

    #[test]
    fn duplicate_peers_fail_before_a_stanza_exists() {
        assert!(I2pInterfaceStanza::new(
            [
                I2pPeerAddress::new("same.i2p").unwrap(),
                I2pPeerAddress::new("same.i2p").unwrap(),
            ],
            SetupReachability::OutboundOnly,
        )
        .is_err());
    }
}
