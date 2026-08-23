mod announce_app_data;
mod buffer_stream;
mod channel;
mod common;
mod ifac;
mod large_request;
mod link_closure;
mod link_packet;
mod plain_group;
mod ratchet;
mod resource_rejection;
mod tcp;
mod tunnel_recovery;
mod udp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    AnnounceAppData,
    BufferStream,
    Channel,
    IfacServer,
    LargeRequest,
    LinkClosure,
    LinkPacket,
    PlainGroup,
    Ratchet,
    ResourceRejectionClient,
    ResourceRejectionServer,
    TcpClient,
    TcpServer,
    TunnelRecoveryClient,
    TunnelRecoveryServer,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UnknownScenario(String);

impl Scenario {
    fn parse(value: &str) -> Result<Self, UnknownScenario> {
        match value {
            "announce-app-data" => Ok(Self::AnnounceAppData),
            "buffer-stream" => Ok(Self::BufferStream),
            "channel" => Ok(Self::Channel),
            "ifac-server" => Ok(Self::IfacServer),
            "large-request" => Ok(Self::LargeRequest),
            "link-closure" => Ok(Self::LinkClosure),
            "link-packet" => Ok(Self::LinkPacket),
            "plain-group" => Ok(Self::PlainGroup),
            "ratchet" => Ok(Self::Ratchet),
            "resource-rejection-client" => Ok(Self::ResourceRejectionClient),
            "resource-rejection-server" => Ok(Self::ResourceRejectionServer),
            "tcp-client" => Ok(Self::TcpClient),
            "tcp-server" => Ok(Self::TcpServer),
            "tunnel-recovery-client" => Ok(Self::TunnelRecoveryClient),
            "tunnel-recovery-server" => Ok(Self::TunnelRecoveryServer),
            "udp" => Ok(Self::Udp),
            unknown => Err(UnknownScenario(unknown.to_owned())),
        }
    }
}

fn report<E: core::fmt::Debug>(result: Result<(), E>) {
    if let Err(error) = result {
        eprintln!("FAILED {error:?}");
        std::process::exit(1);
    }
}

#[tokio::main]
async fn main() {
    let Some(argument) = std::env::args().nth(1) else {
        eprintln!("FAILED missing interop scenario");
        std::process::exit(1);
    };
    let scenario = match Scenario::parse(&argument) {
        Ok(scenario) => scenario,
        Err(error) => {
            eprintln!("FAILED {error:?}");
            std::process::exit(1);
        }
    };
    match scenario {
        Scenario::AnnounceAppData => report(announce_app_data::run().await),
        Scenario::BufferStream => report(buffer_stream::run().await),
        Scenario::Channel => report(channel::run().await),
        Scenario::IfacServer => report(ifac::run_server().await),
        Scenario::LargeRequest => report(large_request::run().await),
        Scenario::LinkClosure => report(link_closure::run().await),
        Scenario::LinkPacket => report(link_packet::run().await),
        Scenario::PlainGroup => report(plain_group::run().await),
        Scenario::Ratchet => report(ratchet::run().await),
        Scenario::ResourceRejectionClient => report(resource_rejection::run_client().await),
        Scenario::ResourceRejectionServer => report(resource_rejection::run_server().await),
        Scenario::TcpClient => report(tcp::run_client().await),
        Scenario::TcpServer => report(tcp::run_server().await),
        Scenario::TunnelRecoveryClient => report(tunnel_recovery::run_client().await),
        Scenario::TunnelRecoveryServer => report(tunnel_recovery::run_server().await),
        Scenario::Udp => report(udp::run().await),
    }
}
