use std::collections::HashMap;

use futures_util::StreamExt;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use prns_interfaces_tokio::wifi_direct::wpa::proxies::{
    P2PDeviceProxy, SupplicantProxy, P2P_DEVICE_INTERFACE, SUPPLICANT_SERVICE,
};

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() {
                byte as char
            } else {
                '.'
            }
        })
        .collect()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    env_logger::init();
    let ifname = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("wlp0s20f3"));
    let connection = zbus::Connection::system().await.expect("system bus");
    let supplicant = SupplicantProxy::new(&connection).await.expect("supplicant");
    let path = supplicant.get_interface(&ifname).await.expect("interface");
    let p2p = P2PDeviceProxy::builder(&connection)
        .path(path)
        .expect("path")
        .build()
        .await
        .expect("p2p proxy");

    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(P2P_DEVICE_INTERFACE)
        .expect("interface rule")
        .sender(SUPPLICANT_SERVICE)
        .expect("sender rule")
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &connection, None)
        .await
        .expect("signal stream");

    let mut config = HashMap::new();
    config.insert("DeviceName", Value::from("Prns-probe"));
    p2p.set_p2p_device_config(config)
        .await
        .expect("device config");
    let mut listen = HashMap::new();
    listen.insert("period", Value::from(500i32));
    listen.insert("interval", Value::from(1500i32));
    let _ = p2p.extended_listen(listen).await;

    let mut service = HashMap::new();
    service.insert("service_type", Value::from("bonjour"));
    service.insert(
        "query",
        Value::from(vec![
            0x05u8, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
        ]),
    );
    service.insert(
        "response",
        Value::from(vec![0x04u8, 0x50, 0x72, 0x6e, 0x73, 0xc0, 0x27]),
    );
    p2p.add_service(service).await.expect("add service");
    let _ = p2p.service_discovery_external(0).await;
    println!("SD_PROBE advertised _prns._tcp bonjour service (wpa auto-answers)");

    let ptr_query = vec![
        0x0du8, 0x00, 0x01, 0x01, 0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
    ];
    let mut args = HashMap::new();
    args.insert("tlv", Value::from(ptr_query));
    let reference = p2p
        .service_discovery_request(args)
        .await
        .expect("sd request");
    println!("SD_PROBE registered _prns._tcp PTR query ref={reference}");

    p2p.find(HashMap::new()).await.expect("find");
    println!("SD_PROBE find running on {ifname}; listening for responses");

    while let Some(message) = stream.next().await {
        let Ok(message) = message else { continue };
        let header = message.header();
        let Some(member) = header.member() else {
            continue;
        };
        match member.as_str() {
            "DeviceFound" => {
                if let Ok((path,)) = message.body().deserialize::<(OwnedObjectPath,)>() {
                    println!("SD_PROBE DeviceFound {path}");
                }
            }
            "GONegotiationRequest" => {
                println!("SD_PROBE *** GONegotiationRequest — the phone recognized us and is forming ***");
                if let Ok((path, _pw, _intent)) =
                    message.body().deserialize::<(OwnedObjectPath, u16, u8)>()
                {
                    println!("SD_PROBE   from {path}");
                }
            }
            "ProvisionDiscoveryPBCRequest" => {
                println!("SD_PROBE ProvisionDiscoveryPBCRequest (peer initiating pairing)");
            }
            "ServiceDiscoveryRequest" => {
                let Ok((request,)) = message
                    .body()
                    .deserialize::<(HashMap<String, OwnedValue>,)>()
                else {
                    continue;
                };
                println!(
                    "SD_PROBE ServiceDiscoveryRequest keys={:?}",
                    request.keys().collect::<Vec<_>>()
                );
                if let Some(tlvs) = request
                    .get("tlvs")
                    .and_then(|value| value.try_clone().ok())
                    .and_then(|value| Vec::<u8>::try_from(value).ok())
                {
                    println!("SD_PROBE req.tlvs.len={}", tlvs.len());
                    println!("SD_PROBE req.tlvs.hex   = {}", hex(&tlvs));
                    println!("SD_PROBE req.tlvs.ascii = {}", ascii(&tlvs));
                }
            }
            "ServiceDiscoveryResponse" => {
                let Ok((response,)) = message
                    .body()
                    .deserialize::<(HashMap<String, OwnedValue>,)>()
                else {
                    continue;
                };
                let peer = response
                    .get("peer_object")
                    .and_then(|value| value.try_clone().ok())
                    .and_then(|value| OwnedObjectPath::try_from(value).ok());
                println!("SD_PROBE ServiceDiscoveryResponse peer={peer:?}");
                if let Some(tlvs) = response
                    .get("tlvs")
                    .and_then(|value| value.try_clone().ok())
                    .and_then(|value| Vec::<u8>::try_from(value).ok())
                {
                    println!("SD_PROBE tlvs.len={}", tlvs.len());
                    println!("SD_PROBE tlvs.hex   = {}", hex(&tlvs));
                    println!("SD_PROBE tlvs.ascii = {}", ascii(&tlvs));
                }
            }
            _ => {}
        }
    }
}
