//! `git clone` over Reticulum — the dogfood, made an example.
//!
//! Two Prns nodes stand up over one loopback link. One serves a throwaway git
//! repository through stock `git-upload-pack`; the other clones it. Every byte of
//! the git pack protocol rides a Prns **ByteStream** — Buffer over Channel over
//! Link over RNS — and git is none the wiser: its built-in `ext::` transport
//! bridges straight onto the stream, so no custom remote helper is needed.
//!
//! Run it:
//! ```text
//! cargo run
//! ```
//! It needs `git` and `nc` on `PATH`, builds its own demo repo and clone target in
//! a temp directory, and removes them when it finishes.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

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

/// Loopback TCP port the two nodes meet on — a stand-in for a real radio or LAN.
const PORT: u16 = 4252;
/// The served destination: `git/serve`, the address the clone side dials.
const APP: &str = "git";
const ASPECTS: &[&str] = &["serve"];

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    // Prns::run is !Send by design; a LocalSet is the host's seam for it.
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, run());
}

async fn run() {
    let demo = DemoRepo::create().expect("set up a throwaway git repo");
    let dest = responder_destination();

    // The serving node: listens on loopback and serves the git/serve destination.
    let listener = Arc::new(TcpListener::bind(("127.0.0.1", PORT)).await.expect("bind"));
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
            identity: responder_secret(),
            announce_app_data: b"git-over-rns",
            proof: ProofStrategy::ProveAll,
            ratchet: RatchetPolicy::NoRatchets,
        }],
        std::vec![serve_serial],
        serve_link_tx,
    );
    let serve_handle = serve.handle();
    tokio::task::spawn_local(serve.run());

    // The cloning node: dials the server.
    let clone_serial = SerialInterface::new(
        move || async move { TcpStream::connect(("127.0.0.1", PORT)).await },
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
    tokio::task::spawn_local(clone.run());

    // The server announces so the cloner learns the path.
    {
        let handle = serve_handle.clone();
        tokio::task::spawn_local(async move {
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

    // The cloner brings the link up; the server learns the same link id from its
    // own event stream. Both ends derive the identical id from the handshake.
    let clone_link = establish(&clone_handle, dest).await;
    let serve_link = tokio::time::timeout(Duration::from_secs(10), serve_link_rx.recv())
        .await
        .expect("server sees the link within the window")
        .expect("server reactor alive");
    println!("LINK UP: clone={clone_link:?} serve={serve_link:?}");
    assert_eq!(clone_link, serve_link, "both ends derive the same link id");

    // One bidirectional byte stream per side: each writes on tx, reads on rx, and
    // the two are mirror images (clone tx=0/rx=1, serve tx=1/rx=0).
    let (mut clone_r, mut clone_w) = clone_handle.byte_stream(clone_link, sid(1), sid(0)).await;
    let (mut serve_r, mut serve_w) = serve_handle.byte_stream(serve_link, sid(0), sid(1)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Server side: git-upload-pack's stdio bridged onto the serving byte stream.
    let mut upload_pack = tokio::process::Command::new("git-upload-pack")
        .arg(&demo.repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn git-upload-pack");
    let mut pack_in = upload_pack.stdin.take().expect("upload-pack stdin");
    let mut pack_out = upload_pack.stdout.take().expect("upload-pack stdout");
    tokio::task::spawn_local(async move {
        let _ = tokio::io::copy(&mut serve_r, &mut pack_in).await;
        let _ = pack_in.shutdown().await;
    });
    tokio::task::spawn_local(async move {
        let _ = tokio::io::copy(&mut pack_out, &mut serve_w).await;
        let _ = serve_w.shutdown().await;
    });

    // Client side: a tiny local TCP endpoint that git's `ext::nc` transport dials,
    // bridged onto the cloning byte stream.
    let bridge = TcpListener::bind(("127.0.0.1", 0u16)).await.expect("bridge bind");
    let bridge_port = bridge.local_addr().expect("bridge addr").port();
    tokio::task::spawn_local(async move {
        let (socket, _) = bridge.accept().await.expect("git's nc connects");
        let (mut socket_r, mut socket_w) = socket.into_split();
        let upstream = tokio::task::spawn_local(async move {
            let _ = tokio::io::copy(&mut socket_r, &mut clone_w).await;
            let _ = clone_w.shutdown().await;
        });
        let downstream = tokio::task::spawn_local(async move {
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
        .arg(&demo.clone_into)
        .status()
        .await
        .expect("run git clone");
    let _ = upload_pack.kill().await;

    if !status.success() {
        println!("FAILED: git clone exited with {status:?}");
        demo.cleanup();
        std::process::exit(2);
    }

    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(&demo.clone_into)
        .arg("log")
        .arg("--oneline")
        .output()
        .expect("git log on the clone");
    println!("OK: git clone over ByteStream pulled the repo. Cloned history:");
    print!("{}", String::from_utf8_lossy(&log.stdout));
    demo.cleanup();
}

/// A throwaway git repository and the directory to clone it into, both under a
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

fn responder_secret() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([0x11u8; IDENTITY_SECRET_KEY_LEN])
}

fn responder_destination() -> DestinationHash {
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(&responder_secret());
    let name = expand_name(APP, ASPECTS).expect("expand git/serve name");
    derive_destination_hash(&signer.identity_hash(), &name)
}

fn sid(id: u16) -> StreamId {
    StreamId::new(id).expect("stream id within the 14-bit range")
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
    panic!("link never established");
}

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
                eprintln!("[{label}] link established");
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
