use socket2::{Domain, Protocol, Socket, Type};
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use tokio::net::{TcpListener, TcpStream};

use super::{is_local_peer, LocalPrefix};

const TCP_RENDEZVOUS_LISTEN_BACKLOG: i32 = 128;

pub(super) fn canonical_rendezvous_peer(peer: SocketAddr) -> SocketAddr {
    if let SocketAddr::V6(address) = peer {
        if let Some(ipv4) = address.ip().to_ipv4_mapped() {
            return SocketAddr::new(IpAddr::V4(ipv4), address.port());
        }
    }
    peer
}

pub(super) fn rendezvous_peer_is_local(peer: SocketAddr, local_prefixes: &[LocalPrefix]) -> bool {
    let peer = canonical_rendezvous_peer(peer);
    if let SocketAddr::V6(peer) = peer {
        if peer.ip().is_unicast_link_local() {
            return peer.scope_id() != 0
                && local_prefixes
                    .iter()
                    .any(|prefix| prefix.index == peer.scope_id());
        }
    }
    is_local_peer(peer.ip(), local_prefixes)
}

pub(super) enum RendezvousListeners {
    Supplied(TcpListener),
    Both {
        ipv4: TcpListener,
        ipv6: TcpListener,
    },
}

impl RendezvousListeners {
    pub(super) async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        match self {
            Self::Supplied(listener) => listener.accept().await,
            Self::Both { ipv4, ipv6 } => tokio::select! {
                accepted = ipv4.accept() => accepted,
                accepted = ipv6.accept() => accepted,
            },
        }
    }
}

fn bind_rendezvous_socket(address: SocketAddr) -> io::Result<TcpListener> {
    let domain = match address {
        SocketAddr::V4(_) => Domain::IPV4,
        SocketAddr::V6(_) => Domain::IPV6,
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&address.into())?;
    socket.listen(TCP_RENDEZVOUS_LISTEN_BACKLOG)?;
    TcpListener::from_std(socket.into())
}

pub(super) fn bind_rendezvous(port: u16) -> io::Result<RendezvousListeners> {
    // Claim IPv4 first to retain the existing central/satellite ownership rule.
    // Separate IPv6 ownership avoids OS-dependent dual-stack bind sharing.
    let ipv4 = bind_rendezvous_socket(SocketAddr::from(([0, 0, 0, 0], port)))?;
    let port = ipv4.local_addr()?.port();
    let ipv6 = bind_rendezvous_socket(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port))?;
    Ok(RendezvousListeners::Both { ipv4, ipv6 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV6;
    use std::time::Duration;

    #[tokio::test]
    async fn rendezvous_accepts_ipv4_and_ipv6_without_losing_port_ownership() {
        let listener = bind_rendezvous(0).unwrap();
        let RendezvousListeners::Both { ref ipv4, .. } = listener else {
            panic!("expected both families")
        };
        let port = ipv4.local_addr().unwrap().port();
        for host in ["127.0.0.1", "::1"] {
            let address = SocketAddr::new(host.parse().unwrap(), port);
            let client = TcpStream::connect(address).await.unwrap();
            let (_, peer) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            assert!(rendezvous_peer_is_local(peer, &[]));
            assert_eq!(canonical_rendezvous_peer(peer).ip(), address.ip());
            drop(client);
        }
        assert_eq!(
            bind_rendezvous(port).err().unwrap().kind(),
            io::ErrorKind::AddrInUse
        );
    }

    #[test]
    fn rendezvous_link_local_admission_preserves_interface_scope() {
        let prefixes = [LocalPrefix {
            addr: "fe80::1".parse().unwrap(),
            netmask: "ffff:ffff:ffff:ffff::".parse().unwrap(),
            index: 7,
        }];
        for (scope, accepted) in [(0, false), (3, false), (7, true)] {
            let peer = SocketAddrV6::new("fe80::2".parse().unwrap(), 42699, 0, scope);
            assert_eq!(rendezvous_peer_is_local(peer.into(), &prefixes), accepted);
        }
        let prefixes = [LocalPrefix {
            addr: "192.168.12.1".parse().unwrap(),
            netmask: "255.255.255.0".parse().unwrap(),
            index: 7,
        }];
        assert!(rendezvous_peer_is_local(
            "[::ffff:192.168.12.224]:5000".parse().unwrap(),
            &prefixes
        ));
        assert!(!rendezvous_peer_is_local(
            "[::ffff:192.168.99.224]:5000".parse().unwrap(),
            &prefixes
        ));
    }

    #[tokio::test]
    async fn an_ipv6_conflict_releases_the_ipv4_claim() {
        let ipv6 =
            bind_rendezvous_socket(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)).unwrap();
        let port = ipv6.local_addr().unwrap().port();
        assert_eq!(
            bind_rendezvous(port).err().unwrap().kind(),
            io::ErrorKind::AddrInUse
        );
        let ipv4 = bind_rendezvous_socket(SocketAddr::from(([0, 0, 0, 0], port))).unwrap();
        drop(ipv6);
        assert_eq!(
            bind_rendezvous(port).err().unwrap().kind(),
            io::ErrorKind::AddrInUse
        );
        drop(ipv4);
        assert!(bind_rendezvous(port).is_ok());
    }
}
