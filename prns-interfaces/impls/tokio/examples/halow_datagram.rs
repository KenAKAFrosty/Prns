//! Bounded link smoke: normal Ethernet data only, on an already configured device.

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use prns_core::interfaces::wifi_halow::{InstanceTag, PeerMac};
    use prns_core::interfaces::MacAddress;
    use prns_interfaces_tokio::wifi_halow::{Destination, EtherType, HaLowSocket};
    use std::io;
    use std::time::Duration;

    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 3 && arguments.len() != 4 {
        return Err(
            "usage: halow_datagram <interface> receive|broadcast|unicast [peer-mac]".into(),
        );
    }
    let invalid = || io::Error::from(io::ErrorKind::InvalidInput);
    let instance = InstanceTag::new(b"halow-smoke").map_err(|_| invalid())?;
    let mode = arguments[2].as_str();
    let peer = if mode == "unicast" && arguments.len() == 4 {
        let mut mac = [0; 6];
        let pieces: Vec<_> = arguments[3].split(':').collect();
        if pieces.len() != 6 {
            return Err(invalid().into());
        }
        for (byte, piece) in mac.iter_mut().zip(pieces) {
            *byte = u8::from_str_radix(piece, 16)?;
        }
        Some(PeerMac::new(MacAddress::new(mac)).map_err(|_| invalid())?)
    } else if matches!(mode, "receive" | "broadcast") && arguments.len() == 3 {
        None
    } else {
        return Err(invalid().into());
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    runtime.block_on(async {
        // Local experimental EtherType; the marker below isolates these lab payloads.
        let socket = HaLowSocket::bind(&arguments[1], EtherType::new(0x88b6)?)?;
        const MARKER: &[u8] = b"PRNS-HALOW-SMOKE-1:";
        if mode == "receive" {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            let mut buffer = [0; 1500];
            let mut count = 0;
            loop {
                let packet =
                    match tokio::time::timeout_at(deadline, socket.receive(&mut buffer)).await {
                        Ok(result) => result?,
                        Err(_) => break,
                    };
                let payload = &buffer[..packet.length];
                if payload.len() != MARKER.len() + 1 || !payload.starts_with(MARKER) {
                    continue;
                }
                count += 1;
                println!(
                    "received sequence={} source={:02x?} interface_id={:02x?}",
                    payload[MARKER.len()],
                    packet.source.address().octets(),
                    instance.peer_id(packet.source).as_bytes()
                );
            }
            println!("received_total={count}");
            if count == 0 {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
        } else {
            for sequence in 0..5_u8 {
                let mut payload = MARKER.to_vec();
                payload.push(sequence);
                let destination = match peer {
                    Some(peer) => Destination::Peer(peer),
                    None => Destination::Broadcast,
                };
                tokio::time::timeout(Duration::from_secs(2), socket.send(destination, &payload))
                    .await
                    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            println!("accepted_total=5");
        }
        Ok::<_, io::Error>(())
    })?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("halow_datagram requires Linux and an already configured Ethernet mesh device");
    std::process::exit(1);
}
