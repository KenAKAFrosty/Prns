//! Compatibility shim for real RNS shared-instance clients — NOT Prns's RPC.
//!
//! Stock RNS apps (Sideband, NomadNet, MeshChat) that attach to a shared instance speak a control
//! channel separate from the data bus: an RPC over Python's `multiprocessing.connection`. LXMF turns
//! on `link.track_phy_stats` on its delivery link, which makes a client fetch per-packet RSSI/SNR/Q
//! over this RPC for *every* link packet (RNS `Link.__update_phy_stats` → `Reticulum.get_packet_rssi`).
//! That call is unguarded, so with no listener it raises `ConnectionRefused`, the exception unwinds
//! through `Link.receive`, and a resource transfer (an LXMF attachment) to that client never
//! completes — the image "just hangs". This shim answers ONLY the minimal set those clients need for
//! attachments to flow: phy stats resolve to `None`, the link first-hop timeout to RNS's default, and
//! everything else to `None`.
//!
//! THIS IS NOT THE RPC MECHANISM FOR THE PRNS ENGINE. It must not grow toward the real RNS RPC surface
//! (path tables, interface stats, drop-path, blackholing, rate tables, ...). It exists purely so an
//! unmodified RNS client does not fault on a control channel we otherwise do not speak; anything past
//! the minimal set is intentionally answered `None`.
//!
//! Wire protocol is RNS 1.3.1's `multiprocessing.connection`: a 4-byte big-endian length frame, a
//! mutual HMAC-SHA256 challenge/response keyed on the shared `rpc_key`, then one pickled request dict
//! and one pickled reply per connection (a client opens a fresh connection per call).

use std::string::String;
use std::vec::Vec;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
#[cfg(target_os = "linux")]
use tokio::net::UnixListener;

use crate::crypto::{hmac_sha256, hmac_sha256_verify};

const CHALLENGE: &[u8] = b"#CHALLENGE#";
const WELCOME: &[u8] = b"#WELCOME#";
const FAILURE: &[u8] = b"#FAILURE#";
const DIGEST_PREFIX: &[u8] = b"{sha256}";
const CHALLENGE_NONCE_LEN: usize = 40;
const MAX_FRAME_LEN: usize = 4096;
const DEFAULT_PER_HOP_TIMEOUT_SECS: i64 = 6;

/// Answers the RNS shared-instance control RPC for stock clients, with the minimal replies that keep
/// attachment delivery from faulting. Stand one up beside a [`LocalServer`](super::tokio::LocalServer)
/// and drive it with [`run`](Self::run).
pub struct SharedInstanceRpcCompat {
    rpc_key: [u8; 32],
    bind: RpcBind,
}

enum RpcBind {
    Tcp(String),
    #[cfg(target_os = "linux")]
    Abstract(String),
}

impl SharedInstanceRpcCompat {
    /// Answer on a loopback TCP port — RNS's `instance_control_port` (default 37428's sibling 37429),
    /// or whatever a client configured. `rpc_key` MUST equal the clients' key: RNS's `full_hash` of the
    /// shared transport identity's private key, or a value both sides set as `rpc_key` in config.
    #[must_use]
    pub fn tcp(rpc_key: [u8; 32], port: u16) -> Self {
        Self {
            rpc_key,
            bind: RpcBind::Tcp(std::format!("127.0.0.1:{port}")),
        }
    }

    /// Answer on the abstract AF_UNIX socket `\0rns/{socket_path}/rpc` that a default-config RNS client
    /// uses on Linux. Linux only.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn abstract_unix(rpc_key: [u8; 32], socket_path: impl Into<String>) -> Self {
        Self {
            rpc_key,
            bind: RpcBind::Abstract(socket_path.into()),
        }
    }

    /// Accept control connections forever, serving each on its own task. Returns only if the listener
    /// cannot be bound.
    pub async fn run(self) {
        match self.bind {
            RpcBind::Tcp(addr) => {
                let Ok(listener) = TcpListener::bind(addr.as_str()).await else {
                    return;
                };
                loop {
                    if let Ok((stream, _)) = listener.accept().await {
                        let key = self.rpc_key;
                        tokio::spawn(async move {
                            let _ = serve_connection(stream, key).await;
                        });
                    }
                }
            }
            #[cfg(target_os = "linux")]
            RpcBind::Abstract(socket_path) => {
                let Some(listener) = bind_abstract_rpc(&socket_path) else {
                    return;
                };
                loop {
                    if let Ok((stream, _)) = listener.accept().await {
                        let key = self.rpc_key;
                        tokio::spawn(async move {
                            let _ = serve_connection(stream, key).await;
                        });
                    }
                }
            }
        }
    }
}

/// Bind `\0rns/{socket_path}/rpc` in the Linux abstract namespace (leading null implied), mirroring how
/// the data bus binds `\0rns/{socket_path}`.
#[cfg(target_os = "linux")]
fn bind_abstract_rpc(socket_path: &str) -> Option<UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let name = std::format!("rns/{socket_path}/rpc");
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
    let listener = std::os::unix::net::UnixListener::bind_addr(&addr).ok()?;
    listener.set_nonblocking(true).ok()?;
    UnixListener::from_std(listener).ok()
}

async fn serve_connection<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    rpc_key: [u8; 32],
) -> std::io::Result<()> {
    if !deliver_our_challenge(&mut stream, &rpc_key).await? {
        return Ok(());
    }
    if !answer_client_challenge(&mut stream, &rpc_key).await? {
        return Ok(());
    }
    let request = read_frame(&mut stream).await?;
    write_frame(&mut stream, &minimal_reply_for(&request)).await
}

/// Mirror of RNS `Listener.deliver_challenge`: send our challenge, then accept the client's response
/// `{sha256}` ++ HMAC-SHA256(`rpc_key`, our_message) and reply `#WELCOME#`. The MAC covers the digest
/// prefix. Returns whether the client authenticated.
async fn deliver_our_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &[u8; 32],
) -> std::io::Result<bool> {
    let mut nonce = [0u8; CHALLENGE_NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| std::io::Error::other("rpc challenge entropy"))?;
    let mut our_message = DIGEST_PREFIX.to_vec();
    our_message.extend_from_slice(&nonce);
    let mut challenge = CHALLENGE.to_vec();
    challenge.extend_from_slice(&our_message);
    write_frame(stream, &challenge).await?;

    let response = read_frame(stream).await?;
    let Some(mac) = response.strip_prefix(DIGEST_PREFIX) else {
        let _ = write_frame(stream, FAILURE).await;
        return Ok(false);
    };
    if hmac_sha256_verify(rpc_key, &our_message, mac).is_err() {
        let _ = write_frame(stream, FAILURE).await;
        return Ok(false);
    }
    write_frame(stream, WELCOME).await?;
    Ok(true)
}

/// Mirror of RNS `Listener.answer_challenge`: reply to the client's challenge with `{sha256}` ++
/// HMAC-SHA256(`rpc_key`, client_message) and await its `#WELCOME#`. Returns whether it accepted us.
async fn answer_client_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &[u8; 32],
) -> std::io::Result<bool> {
    let client_challenge = read_frame(stream).await?;
    let Some(client_message) = client_challenge.strip_prefix(CHALLENGE) else {
        return Ok(false);
    };
    let mut reply = DIGEST_PREFIX.to_vec();
    reply.extend_from_slice(&hmac_sha256(rpc_key, client_message));
    write_frame(stream, &reply).await?;
    Ok(read_frame(stream).await? == WELCOME)
}

/// The minimal answers, keyed on the method name carried in the pickled request. Phy stats and
/// anything unknown resolve to `None`; the link first-hop timeout to RNS's default. NOTHING here is a
/// real RPC answer — this only keeps a stock client from faulting.
fn minimal_reply_for(request: &[u8]) -> Vec<u8> {
    if contains(request, b"first_hop_timeout") {
        pickle_int(DEFAULT_PER_HOP_TIMEOUT_SECS)
    } else if contains(request, b"link_count") {
        pickle_int(0)
    } else {
        pickle_none()
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Protocol-0 `NONE` + `STOP` — `pickle.loads` reads it as `None` on any client.
fn pickle_none() -> Vec<u8> {
    b"N.".to_vec()
}

/// Protocol-0 `INT` + `STOP`.
fn pickle_int(value: i64) -> Vec<u8> {
    let mut out = std::vec![b'I'];
    out.extend_from_slice(std::format!("{value}").as_bytes());
    out.push(b'\n');
    out.push(b'.');
    out
}

async fn write_frame<S: AsyncWrite + Unpin>(stream: &mut S, payload: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let signed = i32::from_be_bytes(header);
    let len = if signed == -1 {
        let mut wide = [0u8; 8];
        stream.read_exact(&mut wide).await?;
        usize::try_from(u64::from_be_bytes(wide))
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
    } else {
        usize::try_from(signed)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
    };
    if len > MAX_FRAME_LEN {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    let mut body = std::vec![0u8; len];
    stream.read_exact(&mut body).await?;
    Ok(body)
}

/// Adopt the host's RNS transport identity as the shared-instance `rpc_key`, so a default-config
/// client derives the same key with no manual step. A client keeps its own config dir, but on a
/// desktop that dir *is* `~/.reticulum`, where RNS persists the node's transport identity as the raw
/// private key at `{storage_dir}/transport_identity`; its `rpc_key` is `full_hash(get_private_key())`,
/// the SHA-256 of those bytes. A present identity is honored untouched; an absent one means this is the
/// first instance on the host, so it is seeded from `seed_if_absent` (owning the identity as a shared
/// instance does).
#[must_use]
pub fn rpc_key_from_rns_identity(storage_dir: &std::path::Path, seed_if_absent: &[u8]) -> [u8; 32] {
    let path = storage_dir.join("transport_identity");
    let private = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = std::fs::create_dir_all(storage_dir);
            let _ = std::fs::write(&path, seed_if_absent);
            seed_if_absent.to_vec()
        }
    };
    crate::crypto::sha256(&private)
}

/// RNS's storage directory: `$RETICULUM_CONFIG_DIR/storage`, else `~/.reticulum/storage` — the layout
/// a stock client uses by default.
#[must_use]
pub fn reticulum_storage_dir() -> std::path::PathBuf {
    std::env::var_os("RETICULUM_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".reticulum")
        })
        .join("storage")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_minimal_set_answers_phy_stats_none_and_timeout_default() {
        let rssi = b"\x80\x04\x95...{'get': 'packet_rssi'}";
        assert_eq!(minimal_reply_for(rssi), b"N.");
        let timeout = b"{'get': 'first_hop_timeout'}";
        assert_eq!(minimal_reply_for(timeout), b"I6\n.");
        let links = b"{'get': 'link_count'}";
        assert_eq!(minimal_reply_for(links), b"I0\n.");
        let unknown = b"{'get': 'path_table'}";
        assert_eq!(minimal_reply_for(unknown), b"N.");
    }

    #[test]
    fn it_seeds_a_missing_rns_identity_then_honors_the_seeded_one() {
        let dir =
            std::env::temp_dir().join(std::format!("prns-rpc-compat-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let seed = [0x33u8; 64];
        let seeded = rpc_key_from_rns_identity(&dir, &seed);
        assert_eq!(seeded, crate::crypto::sha256(&seed));

        let honored = rpc_key_from_rns_identity(&dir, &[0x99u8; 64]);
        assert_eq!(honored, seeded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_client_that_completes_the_mutual_auth_gets_a_reply() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let rpc_key = [0x5au8; 32];
        let (mut client, server) = tokio::io::duplex(8192);
        let server_task = tokio::spawn(async move {
            let _ = serve_connection(server, rpc_key).await;
        });

        async fn read_frame_dup(c: &mut tokio::io::DuplexStream) -> Vec<u8> {
            let mut header = [0u8; 4];
            c.read_exact(&mut header).await.unwrap();
            let len = i32::from_be_bytes(header) as usize;
            let mut body = std::vec![0u8; len];
            c.read_exact(&mut body).await.unwrap();
            body
        }
        async fn write_frame_dup(c: &mut tokio::io::DuplexStream, payload: &[u8]) {
            c.write_all(&(payload.len() as u32).to_be_bytes())
                .await
                .unwrap();
            c.write_all(payload).await.unwrap();
            c.flush().await.unwrap();
        }

        let server_challenge = read_frame_dup(&mut client).await;
        let server_message = server_challenge.strip_prefix(CHALLENGE).unwrap();
        let mut response = DIGEST_PREFIX.to_vec();
        response.extend_from_slice(&hmac_sha256(&rpc_key, server_message));
        write_frame_dup(&mut client, &response).await;
        assert_eq!(read_frame_dup(&mut client).await, WELCOME);

        let mut our_msg = DIGEST_PREFIX.to_vec();
        our_msg.extend_from_slice(&[0x11u8; CHALLENGE_NONCE_LEN]);
        let mut our_challenge = CHALLENGE.to_vec();
        our_challenge.extend_from_slice(&our_msg);
        write_frame_dup(&mut client, &our_challenge).await;
        let server_reply = read_frame_dup(&mut client).await;
        let server_mac = server_reply.strip_prefix(DIGEST_PREFIX).unwrap();
        assert!(hmac_sha256_verify(&rpc_key, &our_msg, server_mac).is_ok());
        write_frame_dup(&mut client, WELCOME).await;

        write_frame_dup(&mut client, b"{'get': 'packet_rssi'}").await;
        assert_eq!(read_frame_dup(&mut client).await, b"N.");

        let _ = server_task.await;
    }
}
