//! A minimal `Prns::new(PrnsRecipe { .. }).run()` callsite over TCP: one `Single` destination, one
//! `/hello` route, an announce ticker on the cloned handle. The reference shape for the new entry
//! point. Demo code, so it `expect`s on setup rather than threading errors.
#![allow(clippy::expect_used)]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::reactor::interfaces::tcp::impls::tokio::TcpServerInterface;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::request_router::{Decline, RequestContext, RequestRoute, RoutePolicy};
use personal_rns::runtime::{PreConfiguredDestination, Prns, PrnsRecipe};
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
    let tcp1 = TcpServerInterface::bind("127.0.0.1:4040", 1_000_000)
        .await
        .expect("should bind the first TCP listener");

    let tcp2 = TcpServerInterface::bind("127.0.0.1:7070", 1_000_000)
        .await
        .expect("should bind the second TCP listener");

    let me = PreConfiguredDestination::Single {
        app_name: "scratch",
        aspects: &["demo"],
        identity: Zeroizing::new([0x11; IDENTITY_SECRET_KEY_LEN]),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    };
    let me_destination = me
        .destination_hash()
        .expect("the scratch destination name is valid");

    let prns = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [me],
        state: (),
        routes: routes![Hello],
        interfaces: interfaces![tcp1, tcp2],
        on_event: |_event, _state| {},
    });

    let announcer = prns.handle();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            let _ = announcer.issue(EngineCommand::AnnounceNow(AnnounceNow {
                destination: me_destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }));
        }
    });

    prns.run().await;
}
