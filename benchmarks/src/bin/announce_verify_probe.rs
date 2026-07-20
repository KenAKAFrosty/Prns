use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use personal_rns::interfaces::pipe;
use personal_rns::interfaces::{InterfaceDescriptor, InterfaceId, InterfaceKind, ReportsStatus};
use personal_rns::reactor::interface_seam::{Interface, InterfaceSeam};
use personal_rns::routes;
use personal_rns::runtime::{
    PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
};
use personal_rns::storage::GrowableHeap;

// A real announce wire packet (the engine's own test vector): valid header, keys,
// id, and Ed25519 signature, so the verify does the full work.
const RNS_1_3_5_ANNOUNCE: &str =
    "010016f8a6d3f7d7c5b6f106d293804d73140002281f6d21232cbba9d12e516183197f08e\
59b7afba27e99e4fe39f01b0d4d2583a5920220253970a16861e82e52e955a05ee39e2b6d2\
0a2331f515512f667009618ccc8f5ebce0600845468d9b829006a172e839fc07deb9b065b91\
7b2891e6d143e6bfc3b80cbdca33f1f85a9ef68835693cb252ba60f558f84436c91761e6f97\
4d0daa069e56495df1870f85d6e6b5af2640868656c6c6f2d706572736f6e616c";

fn bytes_from_hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}

struct AnnounceFlood {
    id: InterfaceId,
    packet: Vec<u8>,
    delivered: Arc<AtomicU64>,
    deadline: Instant,
}

impl Interface for AnnounceFlood {
    const HW_MTU: usize = pipe::PIPE_HW_MTU;
    const KIND: InterfaceKind = InterfaceKind::Pipe;

    fn descriptor(&self) -> InterfaceDescriptor {
        pipe::descriptor(self.id, pipe::configured_policy(Default::default()))
    }

    fn channel_tag(&self) -> &[u8] {
        b"announce-verify-probe"
    }

    async fn run<Seam: InterfaceSeam>(self, mut seam: Seam) {
        while Instant::now() < self.deadline {
            seam.next_inbound(&self.packet).await;
            self.delivered.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl ReportsStatus for AnnounceFlood {
    fn status_view(&self) -> Option<personal_rns::interfaces::StatusView> {
        None
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let duration_ms: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);
    let duration = Duration::from_millis(duration_ms);

    let delivered = Arc::new(AtomicU64::new(0));
    let flood = AnnounceFlood {
        id: InterfaceId::new([0xAF; 8]),
        packet: bytes_from_hex(RNS_1_3_5_ANNOUNCE),
        delivered: delivered.clone(),
        deadline: Instant::now() + duration,
    };

    let node: PrnsNode<(), (), _, GrowableHeap> = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        on_event: |_event: PrnsEvent<'_>, _state: &()| {},
        interfaces: |node: &PrnsNodeHandle| {
            node.add_interface(flood);
        },
    });

    let start = Instant::now();
    tokio::select! {
        _ = node.run() => {}
        () = tokio::time::sleep(duration + Duration::from_millis(250)) => {}
    }
    let elapsed = start.elapsed().as_secs_f64().max(f64::EPSILON);
    let n = delivered.load(Ordering::Relaxed);
    println!(
        "RESULT verified={n} announce_verify_per_sec={:.0} elapsed_ms={:.0}",
        n as f64 / elapsed,
        elapsed * 1000.0,
    );
}
