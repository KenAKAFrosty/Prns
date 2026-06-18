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
//! Wire protocol is `multiprocessing.connection`: a 4-byte big-endian length frame, a mutual HMAC
//! challenge/response keyed on the shared `rpc_key`, then one pickled request dict and one pickled
//! reply per connection (a client opens a fresh connection per call). The HMAC digest is *negotiated*
//! exactly as CPython does, so any client version interoperates: a modern client (Python 3.12+) tags
//! its messages with a `{sha256}` prefix; a legacy client (Python ≤ 3.11) sends an unprefixed
//! HMAC-MD5. Honoring both is what lets attachments flow regardless of the client's Python.

use std::string::String;
use std::vec::Vec;

use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;
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
const LEGACY_MD5_DIGEST_LEN: usize = 16;
const LEGACY_MD5_MESSAGE_LEN: usize = 20;

/// The HMAC digests `multiprocessing.connection` negotiates. A modern peer prefixes its message with
/// `{sha256}`; a legacy peer (Python ≤ 3.11) sends a bare HMAC-MD5 with no prefix at all.
#[derive(Clone, Copy)]
enum Digest {
    Md5,
    Sha256,
}

impl Digest {
    fn label(self) -> &'static [u8] {
        match self {
            Digest::Md5 => b"md5",
            Digest::Sha256 => b"sha256",
        }
    }

    #[allow(clippy::expect_used)]
    fn mac(self, key: &[u8], message: &[u8]) -> Vec<u8> {
        match self {
            Digest::Sha256 => hmac_sha256(key, message).to_vec(),
            Digest::Md5 => {
                let mut mac =
                    <Hmac<Md5>>::new_from_slice(key).expect("HMAC accepts a key of any length");
                mac.update(message);
                mac.finalize().into_bytes().to_vec()
            }
        }
    }

    #[allow(clippy::expect_used)]
    fn verify(self, key: &[u8], message: &[u8], tag: &[u8]) -> bool {
        match self {
            Digest::Sha256 => hmac_sha256_verify(key, message, tag).is_ok(),
            Digest::Md5 => {
                let mut mac =
                    <Hmac<Md5>>::new_from_slice(key).expect("HMAC accepts a key of any length");
                mac.update(message);
                mac.verify_slice(tag).is_ok()
            }
        }
    }
}

/// Mirror of CPython's `_get_digest_name_and_payload`: a message of a legacy length carries a bare
/// HMAC-MD5 (digest [`None`], whole message is the payload); a `{digest}`-prefixed message names its
/// own digest. An unrecognized prefix or digest is rejected (`None`), as CPython raises.
fn negotiated_digest(message: &[u8]) -> Option<(Option<Digest>, &[u8])> {
    if message.len() == LEGACY_MD5_DIGEST_LEN || message.len() == LEGACY_MD5_MESSAGE_LEN {
        return Some((None, message));
    }
    let rest = message.strip_prefix(b"{")?;
    let close = rest.iter().position(|&byte| byte == b'}')?;
    let digest = match &rest[..close] {
        b"sha256" => Digest::Sha256,
        b"md5" => Digest::Md5,
        _ => return None,
    };
    Some((Some(digest), &rest[close + 1..]))
}

/// Mirror of CPython's `_create_response`: answer a peer's challenge `message` with a MAC over the
/// whole message. A legacy (unprefixed) challenge gets a bare HMAC-MD5; a `{digest}`-prefixed
/// challenge gets the same `{digest}` prefix back. [`None`] when the challenge digest is unsupported.
fn create_response(key: &[u8; 32], message: &[u8]) -> Option<Vec<u8>> {
    match negotiated_digest(message)? {
        (None, _) => Some(Digest::Md5.mac(key, message)),
        (Some(digest), _) => {
            let mut reply = std::vec![b'{'];
            reply.extend_from_slice(digest.label());
            reply.push(b'}');
            reply.extend_from_slice(&digest.mac(key, message));
            Some(reply)
        }
    }
}

/// Mirror of CPython's `_verify_challenge`: the peer's `response` to *our* `challenge_message` is a
/// MAC over that message, in whatever digest the response declares (an unprefixed response means the
/// legacy HMAC-MD5). Constant-time, length-checked.
fn response_authenticates(key: &[u8; 32], challenge_message: &[u8], response: &[u8]) -> bool {
    match negotiated_digest(response) {
        Some((digest, mac)) => digest
            .unwrap_or(Digest::Md5)
            .verify(key, challenge_message, mac),
        None => false,
    }
}

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

/// Mirror of RNS `Listener.deliver_challenge`: send our `{sha256}`-tagged challenge, accept the
/// client's MAC over it (sha256 if it tags one, else the legacy unprefixed HMAC-MD5), and reply
/// `#WELCOME#`. The MAC covers the digest prefix. Returns whether the client authenticated.
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
    if !response_authenticates(rpc_key, &our_message, &response) {
        let _ = write_frame(stream, FAILURE).await;
        return Ok(false);
    }
    write_frame(stream, WELCOME).await?;
    Ok(true)
}

/// Mirror of RNS `Listener.answer_challenge`: answer the client's challenge in its own negotiated
/// digest (a `{sha256}` reply for a modern client, a bare HMAC-MD5 for a legacy one) and await its
/// `#WELCOME#`. Returns whether it accepted us.
async fn answer_client_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &[u8; 32],
) -> std::io::Result<bool> {
    let client_challenge = read_frame(stream).await?;
    let Some(client_message) = client_challenge.strip_prefix(CHALLENGE) else {
        return Ok(false);
    };
    let Some(reply) = create_response(rpc_key, client_message) else {
        return Ok(false);
    };
    write_frame(stream, &reply).await?;
    Ok(read_frame(stream).await? == WELCOME)
}

/// The wire codec a client's RPC payload speaks. RNS through 1.3.x carried the request and reply as
/// `multiprocessing.connection`'s pickle (`connection.send`/`recv`); RNS 1.3.5+ frames msgpack
/// (`send_bytes(mp.packb(..))` / `mp.unpackb(recv_bytes())`). Both share the same length-prefixed
/// framing and the same auth handshake — only the payload codec differs, so the reply must answer in
/// the dialect the request arrived in or the client mis-decodes it (a pickle `None` reads back as the
/// msgpack integer 78, which a client indexing the result then faults on).
#[derive(Clone, Copy)]
enum RpcDialect {
    Pickle,
    Msgpack,
}

/// Tell the dialects apart by the request's first byte: every RNS RPC request is a small map, so a
/// msgpack request opens with a fixmap tag (`0x81..=0x8f`), while a pickle stream opens with the PROTO
/// opcode `0x80` (or a protocol-0 opcode) — never `0x81..=0x8f`.
fn dialect_of(request: &[u8]) -> RpcDialect {
    match request.first() {
        Some(0x81..=0x8f) => RpcDialect::Msgpack,
        _ => RpcDialect::Pickle,
    }
}

/// The minimal answers, keyed on the method name (a readable substring of the request in either codec)
/// and encoded in the client's own dialect. `interface_stats` resolves to an empty interface map: a
/// NomadNet TextUI calls it at startup and immediately indexes `["interfaces"]`, so a bare `None`
/// crashes it before the UI draws. Phy stats and anything unknown resolve to `None`; the link
/// first-hop timeout to RNS's default; the link count to zero. NOTHING here is a real RPC answer —
/// this only keeps a stock client from faulting.
fn minimal_reply_for(request: &[u8]) -> Vec<u8> {
    let dialect = dialect_of(request);
    if contains(request, b"interface_stats") {
        empty_interface_stats(dialect)
    } else if contains(request, b"first_hop_timeout") {
        reply_int(dialect, DEFAULT_PER_HOP_TIMEOUT_SECS)
    } else if contains(request, b"link_count") {
        reply_int(dialect, 0)
    } else {
        reply_none(dialect)
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// `None` in the client's dialect: pickle protocol-0 `NONE` + `STOP`, or msgpack nil.
fn reply_none(dialect: RpcDialect) -> Vec<u8> {
    match dialect {
        RpcDialect::Pickle => b"N.".to_vec(),
        RpcDialect::Msgpack => std::vec![0xc0],
    }
}

/// A non-negative integer (the only ones the minimal set returns: a 6-second timeout, a zero link
/// count) in the client's dialect: pickle protocol-0 `INT` + `STOP`, or a msgpack positive fixint.
fn reply_int(dialect: RpcDialect, value: i64) -> Vec<u8> {
    match dialect {
        RpcDialect::Pickle => {
            let mut out = std::vec![b'I'];
            out.extend_from_slice(std::format!("{value}").as_bytes());
            out.push(b'\n');
            out.push(b'.');
            out
        }
        RpcDialect::Msgpack if (0..=0x7f).contains(&value) => std::vec![value as u8],
        RpcDialect::Msgpack => std::vec![0xc0],
    }
}

/// The empty interface-stats map `{"interfaces": []}` — the shape a stock client indexes by
/// `["interfaces"]` — in the client's dialect. The shim is siloed from the engine, so it reports no
/// interfaces rather than the node's real ones; the point is only to let the client launch, not to
/// mirror its status screen. Msgpack: fixmap(1) ‖ fixstr("interfaces") ‖ fixarray(0). Pickle:
/// `pickle.dumps({"interfaces": []}, protocol=2)`.
fn empty_interface_stats(dialect: RpcDialect) -> Vec<u8> {
    match dialect {
        RpcDialect::Msgpack => {
            let mut out = std::vec![0x81, 0xaa];
            out.extend_from_slice(b"interfaces");
            out.push(0x90);
            out
        }
        RpcDialect::Pickle => std::vec![
            0x80, 0x02, 0x7d, 0x71, 0x00, 0x58, 0x0a, 0x00, 0x00, 0x00, b'i', b'n', b't', b'e',
            b'r', b'f', b'a', b'c', b'e', b's', 0x71, 0x01, 0x5d, 0x71, 0x02, 0x73, 0x2e,
        ],
    }
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

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
    fn a_msgpack_client_gets_msgpack_replies_in_its_own_dialect() {
        let interface_stats = b"\x81\xa3get\xafinterface_stats";
        assert_eq!(
            minimal_reply_for(interface_stats),
            b"\x81\xaainterfaces\x90",
        );
        let timeout = b"\x81\xa3get\xb1first_hop_timeout";
        assert_eq!(minimal_reply_for(timeout), b"\x06");
        let links = b"\x81\xa3get\xaalink_count";
        assert_eq!(minimal_reply_for(links), b"\x00");
        let rssi = b"\x82\xa3get\xabpacket_rssi";
        assert_eq!(minimal_reply_for(rssi), b"\xc0");
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
    async fn a_modern_sha256_client_completes_the_mutual_auth_and_gets_a_reply() {
        let rpc_key = [0x5au8; 32];
        let (mut client, server) = tokio::io::duplex(8192);
        let server_task = tokio::spawn(async move {
            let _ = serve_connection(server, rpc_key).await;
        });

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

    #[tokio::test]
    async fn a_legacy_md5_client_without_a_digest_prefix_still_authenticates() {
        let rpc_key = [0x5au8; 32];
        let (mut client, server) = tokio::io::duplex(8192);
        let server_task = tokio::spawn(async move {
            let _ = serve_connection(server, rpc_key).await;
        });

        let server_challenge = read_frame_dup(&mut client).await;
        let server_message = server_challenge.strip_prefix(CHALLENGE).unwrap();
        write_frame_dup(&mut client, &Digest::Md5.mac(&rpc_key, server_message)).await;
        assert_eq!(read_frame_dup(&mut client).await, WELCOME);

        let our_message = [0x22u8; LEGACY_MD5_MESSAGE_LEN];
        let mut our_challenge = CHALLENGE.to_vec();
        our_challenge.extend_from_slice(&our_message);
        write_frame_dup(&mut client, &our_challenge).await;
        let server_reply = read_frame_dup(&mut client).await;
        assert_eq!(server_reply, Digest::Md5.mac(&rpc_key, &our_message));
        write_frame_dup(&mut client, WELCOME).await;

        write_frame_dup(&mut client, b"{'get': 'packet_rssi'}").await;
        assert_eq!(read_frame_dup(&mut client).await, b"N.");

        let _ = server_task.await;
    }
}
