//! `git clone` over Reticulum — the dogfood, made an example.
//!
//! Every byte of git's pack protocol rides a Prns **ByteStream** (Buffer over
//! Channel over Link over RNS). git is none the wiser: its built-in `ext::`
//! transport bridges straight onto the stream, so there's no custom remote helper.
//!
//! The thing a clone *addresses* is a **Reticulum destination** — the server's
//! `git/serve` destination hash — never an IP. An IP appears only as the interface
//! wire (`--listen` / `--connect`): how two RNS instances physically find each
//! other. Swap loopback for a LAN address and the exact same code spans two
//! machines.
//!
//! ```text
//! cargo run                                  # self-contained loopback smoke
//! cargo run -- serve --repo PATH             # serve a repo; prints its destination
//! cargo run -- clone <dest-hex> INTO --connect HOST:PORT
//! ```
//! Needs `git` and `nc` on `PATH`.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::spawn_local;

use personal_rns::crypto::ratchets::RatchetPolicy;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, Settlement,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::serial::impls::tokio::SerialInterface;
use personal_rns::routing::announce::{derive_destination_hash, expand_name};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    Diagnostic, PreConfiguredDestination, Prns, PrnsEvent, PrnsHandle, PrnsRecipe, StreamId,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;

/// The served destination's name: `git/serve`. Its hash is what a clone addresses.
const APP: &str = "git";
const ASPECTS: &[&str] = &["serve"];
const DEFAULT_PORT: u16 = 4252;

const USAGE: &str = "\
git-over-rns — a real `git clone` carried over a Reticulum ByteStream.

USAGE:
    git-over-rns                                    self-contained loopback smoke
    git-over-rns serve [--repo PATH] [--listen ADDR]
    git-over-rns clone <DESTINATION-HEX> <INTO> --connect ADDR

`serve` prints its destination hash; hand that hash to `clone`. The address flags
are only the interface wire (a stand-in for a radio); the clone target is always
the destination hash.";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    // Prns::run is !Send by design; a LocalSet is the host's seam for it.
    let local = tokio::task::LocalSet::new();
    match args.get(1).map(String::as_str) {
        None | Some("demo") => local.block_on(&runtime, demo()),
        Some("serve") => local.block_on(&runtime, serve(ServeOpts::parse(&args[2..]))),
        Some("clone") => local.block_on(&runtime, clone(CloneOpts::parse(&args[2..]))),
        Some("--help" | "-h") => println!("{USAGE}"),
        Some(other) => {
            eprintln!("unknown subcommand: {other}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// serve: announce a git/serve destination and serve git-upload-pack per link.
// ---------------------------------------------------------------------------

struct ServeOpts {
    repo: PathBuf,
    listen: String,
}

impl ServeOpts {
    fn parse(args: &[String]) -> Self {
        let repo = flag(args, "--repo").map_or_else(|| PathBuf::from("."), PathBuf::from);
        let listen = flag(args, "--listen").unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_PORT}"));
        Self { repo, listen }
    }
}

async fn serve(opts: ServeOpts) {
    let dest = served_destination();
    let hex = hex16(dest.as_bytes());
    let port = opts.listen.rsplit_once(':').map_or(DEFAULT_PORT.to_string(), |(_, p)| p.to_string());
    eprintln!("serving {} as git/serve", opts.repo.display());
    eprintln!("destination: {hex}");
    eprintln!("clone it from another machine with:");
    eprintln!("    git-over-rns clone {hex} <into> --connect <this-host>:{port}\n");

    let listener = Arc::new(TcpListener::bind(opts.listen.as_str()).await.expect("bind --listen"));
    let serial = SerialInterface::new(
        move || {
            let listener = listener.clone();
            async move { listener.accept().await.map(|(stream, _)| stream) }
        },
        Duration::from_millis(500),
        b"serve-serial",
    );

    let (link_tx, mut link_rx) = mpsc::unbounded_channel::<LinkId>();
    let node = node(
        "serve",
        std::vec![PreConfiguredDestination::Single {
            app_name: APP,
            aspects: ASPECTS,
            identity: served_secret(),
            announce_app_data: b"git-over-rns",
            proof: ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::NoRatchets,
        }],
        std::vec![serial],
        link_tx,
    );
    let handle = node.handle();
    spawn_local(node.run());
    spawn_announce(handle.clone(), dest);

    // Every link a clone brings up gets its own git-upload-pack, bridged onto the
    // link's byte stream. The serve side reads stream 0 (the clone's tx) and writes
    // stream 1 (the clone's rx).
    while let Some(link) = link_rx.recv().await {
        eprintln!("link up — serving git-upload-pack over {link:?}");
        spawn_local(serve_one(handle.clone(), link, opts.repo.clone()));
    }
}

async fn serve_one(handle: PrnsHandle, link: LinkId, repo: PathBuf) {
    let (mut stream_r, mut stream_w) = handle.byte_stream(link, sid(0), sid(1)).await;
    let mut upload_pack = match tokio::process::Command::new("git-upload-pack")
        .arg(&repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("could not spawn git-upload-pack: {error}");
            return;
        }
    };
    let mut pack_in = upload_pack.stdin.take().expect("upload-pack stdin");
    let mut pack_out = upload_pack.stdout.take().expect("upload-pack stdout");
    let to_pack = spawn_local(async move {
        let _ = tokio::io::copy(&mut stream_r, &mut pack_in).await;
        let _ = pack_in.shutdown().await;
    });
    let from_pack = spawn_local(async move {
        let _ = tokio::io::copy(&mut pack_out, &mut stream_w).await;
        let _ = stream_w.shutdown().await;
    });
    let _ = to_pack.await;
    let _ = from_pack.await;
    let _ = upload_pack.wait().await;
}

// ---------------------------------------------------------------------------
// clone: address a destination hash, learn its path, clone over the stream.
// ---------------------------------------------------------------------------

struct CloneOpts {
    dest: DestinationHash,
    into: PathBuf,
    connect: String,
}

impl CloneOpts {
    fn parse(args: &[String]) -> Self {
        let mut positional = std::vec::Vec::new();
        let mut connect = None;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--connect" => {
                    connect = args.get(i + 1).cloned();
                    i += 2;
                }
                flagged if flagged.starts_with("--") => {
                    eprintln!("unknown flag: {flagged}\n\n{USAGE}");
                    std::process::exit(2);
                }
                _ => {
                    positional.push(args[i].clone());
                    i += 1;
                }
            }
        }
        let dest = positional
            .first()
            .and_then(|hex| parse_dest(hex))
            .unwrap_or_else(|| fail("clone needs a 32-hex-char destination as its first argument"));
        let into = positional
            .get(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| fail("clone needs a target directory as its second argument"));
        let connect = connect.unwrap_or_else(|| fail("clone needs --connect <host:port>"));
        Self {
            dest,
            into,
            connect,
        }
    }
}

async fn clone(opts: CloneOpts) {
    let connect = opts.connect.clone();
    let serial = SerialInterface::new(
        move || {
            let addr = connect.clone();
            async move { TcpStream::connect(addr).await }
        },
        Duration::from_millis(500),
        b"clone-serial",
    );
    let node = node(
        "clone",
        std::vec::Vec::<PreConfiguredDestination>::new(),
        std::vec![serial],
        mpsc::unbounded_channel::<LinkId>().0,
    );
    let handle = node.handle();
    spawn_local(node.run());

    println!(
        "interface up to {}; learning the path to {} from its announce...",
        opts.connect,
        hex16(opts.dest.as_bytes())
    );
    let link = establish(&handle, opts.dest).await;
    println!("link up — {link:?}");

    // The clone side writes stream 0 / reads stream 1; the serve side mirrors.
    let (mut stream_r, mut stream_w) = handle.byte_stream(link, sid(1), sid(0)).await;

    // git's `ext::nc` transport dials this tiny local endpoint, which is bridged
    // onto the byte stream. git never learns there's a Reticulum link underneath.
    let bridge = TcpListener::bind(("127.0.0.1", 0u16)).await.expect("bridge bind");
    let bridge_port = bridge.local_addr().expect("bridge addr").port();
    spawn_local(async move {
        let (socket, _) = bridge.accept().await.expect("git's nc connects");
        let (mut socket_r, mut socket_w) = socket.into_split();
        let upstream = spawn_local(async move {
            let _ = tokio::io::copy(&mut socket_r, &mut stream_w).await;
            let _ = stream_w.shutdown().await;
        });
        let downstream = spawn_local(async move {
            let _ = tokio::io::copy(&mut stream_r, &mut socket_w).await;
            let _ = socket_w.shutdown().await;
        });
        let _ = upstream.await;
        let _ = downstream.await;
    });

    println!("running `git clone` over the ByteStream...");
    let status = tokio::process::Command::new("git")
        .arg("-c")
        .arg("protocol.ext.allow=always")
        .arg("-c")
        .arg("protocol.version=0")
        .arg("clone")
        .arg(format!("ext::nc 127.0.0.1 {bridge_port}"))
        .arg(&opts.into)
        .status()
        .await
        .expect("run git clone");
    handle.close_link(link);

    if status.success() {
        println!("OK: cloned into {}", opts.into.display());
    } else {
        eprintln!("FAILED: git clone exited with {status:?}");
        std::process::exit(2);
    }
}

// ---------------------------------------------------------------------------
// demo: both halves in one process over loopback — the self-contained smoke.
// ---------------------------------------------------------------------------

async fn demo() {
    let repo = DemoRepo::create().expect("set up a throwaway git repo");
    let dest = served_destination();

    let listener = Arc::new(
        TcpListener::bind(("127.0.0.1", DEFAULT_PORT))
            .await
            .expect("bind loopback"),
    );
    let serve_serial = SerialInterface::new(
        move || {
            let listener = listener.clone();
            async move { listener.accept().await.map(|(stream, _)| stream) }
        },
        Duration::from_millis(500),
        b"serve-serial",
    );
    let (serve_link_tx, mut serve_link_rx) = mpsc::unbounded_channel::<LinkId>();
    let serve = node(
        "serve",
        std::vec![PreConfiguredDestination::Single {
            app_name: APP,
            aspects: ASPECTS,
            identity: served_secret(),
            announce_app_data: b"git-over-rns",
            proof: ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::NoRatchets,
        }],
        std::vec![serve_serial],
        serve_link_tx,
    );
    let serve_handle = serve.handle();
    spawn_local(serve.run());

    let clone_serial = SerialInterface::new(
        move || async move { TcpStream::connect(("127.0.0.1", DEFAULT_PORT)).await },
        Duration::from_millis(500),
        b"clone-serial",
    );
    let clone = node(
        "clone",
        std::vec::Vec::<PreConfiguredDestination>::new(),
        std::vec![clone_serial],
        mpsc::unbounded_channel::<LinkId>().0,
    );
    let clone_handle = clone.handle();
    spawn_local(clone.run());
    spawn_announce(serve_handle.clone(), dest);

    let clone_link = establish(&clone_handle, dest).await;
    let serve_link = tokio::time::timeout(Duration::from_secs(10), serve_link_rx.recv())
        .await
        .expect("server sees the link within the window")
        .expect("server reactor alive");
    println!("LINK UP: clone={clone_link:?} serve={serve_link:?}");
    assert_eq!(clone_link, serve_link, "both ends derive the same link id");

    let (mut clone_r, mut clone_w) = clone_handle.byte_stream(clone_link, sid(1), sid(0)).await;
    let (mut serve_r, mut serve_w) = serve_handle.byte_stream(serve_link, sid(0), sid(1)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut upload_pack = tokio::process::Command::new("git-upload-pack")
        .arg(&repo.repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn git-upload-pack");
    let mut pack_in = upload_pack.stdin.take().expect("upload-pack stdin");
    let mut pack_out = upload_pack.stdout.take().expect("upload-pack stdout");
    spawn_local(async move {
        let _ = tokio::io::copy(&mut serve_r, &mut pack_in).await;
        let _ = pack_in.shutdown().await;
    });
    spawn_local(async move {
        let _ = tokio::io::copy(&mut pack_out, &mut serve_w).await;
        let _ = serve_w.shutdown().await;
    });

    let bridge = TcpListener::bind(("127.0.0.1", 0u16)).await.expect("bridge bind");
    let bridge_port = bridge.local_addr().expect("bridge addr").port();
    spawn_local(async move {
        let (socket, _) = bridge.accept().await.expect("git's nc connects");
        let (mut socket_r, mut socket_w) = socket.into_split();
        let upstream = spawn_local(async move {
            let _ = tokio::io::copy(&mut socket_r, &mut clone_w).await;
            let _ = clone_w.shutdown().await;
        });
        let downstream = spawn_local(async move {
            let _ = tokio::io::copy(&mut clone_r, &mut socket_w).await;
            let _ = socket_w.shutdown().await;
        });
        let _ = upstream.await;
        let _ = downstream.await;
    });

    println!("BRIDGE UP on 127.0.0.1:{bridge_port}; running real `git clone` over ByteStream...");
    let status = tokio::process::Command::new("git")
        .arg("-c")
        .arg("protocol.ext.allow=always")
        .arg("-c")
        .arg("protocol.version=0")
        .arg("clone")
        .arg(format!("ext::nc 127.0.0.1 {bridge_port}"))
        .arg(&repo.clone_into)
        .status()
        .await
        .expect("run git clone");
    let _ = upload_pack.kill().await;

    if !status.success() {
        eprintln!("FAILED: git clone exited with {status:?}");
        repo.cleanup();
        std::process::exit(2);
    }

    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo.clone_into)
        .arg("log")
        .arg("--oneline")
        .output()
        .expect("git log on the clone");
    println!("OK: git clone over ByteStream pulled the repo. Cloned history:");
    print!("{}", String::from_utf8_lossy(&log.stdout));
    repo.cleanup();
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

/// Build a node from a recipe: a current set of served destinations, the interfaces
/// it speaks over, and a sink the event stream forwards every established link id to.
fn node<D, I>(
    label: &'static str,
    destinations: D,
    interfaces: std::vec::Vec<I>,
    link_tx: mpsc::UnboundedSender<LinkId>,
) -> Prns<(), (), impl FnMut(PrnsEvent<'_>, &()), GrowableHeap>
where
    D: IntoIterator<Item = PreConfiguredDestination<'static>>,
    I: personal_rns::reactor::interface_seam::Interface + Send + 'static,
{
    Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: destinations,
        app_state: (),
        storage: GrowableHeap,
        routes: (),
        interfaces,
        on_event: move |event: PrnsEvent<'_>, _: &()| match event {
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(established)) => {
                let _ = link_tx.send(established.link_id);
            }
            PrnsEvent::Diagnostic(Diagnostic::CommandSettled {
                settlement: Settlement::EstablishLink(Ok(established)),
                ..
            }) => {
                let _ = link_tx.send(established.link_id);
            }
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { hops, .. }) => {
                eprintln!("[{label}] announce heard (hops={hops})");
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => {
                eprintln!("[{label}] link closed");
            }
            _ => {}
        },
    })
}

/// Re-announce the served destination once a second so a fresh peer learns the path.
fn spawn_announce(handle: PrnsHandle, dest: DestinationHash) {
    spawn_local(async move {
        loop {
            handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
                destination: dest,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }));
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

/// Bring the link up, retrying until the announce has propagated and the proof
/// validates. `establish_link` resolves the link id straight out of the settlement.
async fn establish(handle: &PrnsHandle, dest: DestinationHash) -> LinkId {
    for attempt in 0..40 {
        match handle.establish_link(dest).await {
            Ok(link) => return link,
            Err(reason) => {
                eprintln!("[clone] establish attempt {attempt} not ready yet ({reason:?})");
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    }
    fail("link never established")
}

/// The demo identity. A fixed secret keeps the destination hash stable and printable
/// for an example; a real deployment would load a persistent identity from a vault so
/// the hash survives restarts without being a known constant.
fn served_secret() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([0x11u8; IDENTITY_SECRET_KEY_LEN])
}

fn served_destination() -> DestinationHash {
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&served_secret());
    let name = expand_name(APP, ASPECTS).expect("expand git/serve name");
    derive_destination_hash(&signer.identity_hash(), &name)
}

fn sid(id: u16) -> StreamId {
    StreamId::new(id).expect("stream id within the 14-bit range")
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1).cloned())
}

fn parse_dest(hex: &str) -> Option<DestinationHash> {
    let hex = hex.trim();
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(DestinationHash::new(bytes))
}

fn hex16(bytes: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn fail(message: &str) -> ! {
    eprintln!("{message}\n\n{USAGE}");
    std::process::exit(2);
}

/// A throwaway git repository and a directory to clone it into, both under a
/// per-process temp directory that [`cleanup`](DemoRepo::cleanup) removes.
struct DemoRepo {
    base: PathBuf,
    repo: PathBuf,
    clone_into: PathBuf,
}

impl DemoRepo {
    fn create() -> std::io::Result<Self> {
        let base = std::env::temp_dir().join(format!("git-over-rns-{}", std::process::id()));
        let repo = base.join("demo");
        let clone_into = base.join("clone");
        std::fs::create_dir_all(&repo)?;

        let repo_path = repo.to_str().expect("utf-8 temp path");
        git(&["-C", repo_path, "-c", "init.defaultBranch=main", "init", "-q"])?;
        std::fs::write(
            repo.join("readme.txt"),
            "This repository was cloned over a Reticulum ByteStream.\n",
        )?;
        git(&["-C", repo_path, "add", "readme.txt"])?;
        git(&[
            "-C",
            repo_path,
            "-c",
            "user.email=demo@reticulum.rs",
            "-c",
            "user.name=Prns Demo",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "first commit, carried by ByteStream over RNS",
        ])?;
        Ok(Self {
            base,
            repo,
            clone_into,
        })
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn git(args: &[&str]) -> std::io::Result<()> {
    let status = std::process::Command::new("git").args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("git {args:?} failed: {status}")))
    }
}
