//! A minimal `Prns::new(Recipe { .. }).run()` callsite over TCP: one `Single` destination, one
//! `/hello` route, an announce ticker on the cloned handle. The reference shape for the new entry
//! point. Demo code, so it `expect`s on setup rather than threading errors.
#![allow(clippy::expect_used)]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::interfaces::tcp::impls::tokio::TcpServerInterface;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::request_router::{Decline, RequestContext, RequestRoute, RoutePolicy};
use personal_rns::runtime::{Prns, Recipe, StartingDestination};
use personal_rns::{interfaces, routes};

struct Hello;

impl RequestRoute<()> for Hello {
    const PATH: &'static str = "/hello";
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;
    async fn handle(mut context: RequestContext<'_, ()>) -> Result<(), Decline> {
        context.respond(b"world")
    }
}

#[tokio::main]
async fn main() {
    let tcp = TcpServerInterface::bind(InterfaceId::new([0xA0; 16]), "127.0.0.1:4040", 1_000_000)
        .await
        .expect("bind the TCP listener");

    let me = StartingDestination::Single {
        app_name: "scratch",
        aspects: &["demo"],
        identity: Zeroizing::new([0x11; IDENTITY_SECRET_KEY_LEN]),
        app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    };
    let address = me.address();

    let prns = Prns::new(Recipe {
        transport: None,
        destinations: [me],
        state: (),
        routes: routes![Hello],
        on_event: |_event, _state| {},
        interfaces: interfaces![tcp],
    });

    let announcer = prns.handle();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            let _ = announcer.issue(EngineCommand::AnnounceNow(AnnounceNow {
                destination: address,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }));
        }
    });

    prns.run().await;
}
