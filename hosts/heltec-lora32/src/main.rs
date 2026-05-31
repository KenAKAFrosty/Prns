//! Heltec WiFi LoRa 32 (ESP32-S3 + SX1262 + OLED) host.
//!
//! - **Stage A/B** (done): the Personal Reticulum engine runs on the S3, with
//!   live state on the OLED.
//! - **RNSAutoInterface** (this file): the RNS-compatible UDP LAN interface,
//!   brought up via `esp-radio` (async-first → an embassy executor under
//!   `#[esp_rtos::main]`; engine tick + OLED live in an async loop).
//!   M1 WiFi association, M2 embassy-net IP stack (SLAAC link-local), M3
//!   multicast group join, M4 the RNS-exact discovery handshake (we beacon
//!   `sha256(group_id ++ our link-local)` and peer with any node whose beacon
//!   authenticates against its source address), and now **M5: the data plane** —
//!   the engine's self-announce is fanned out as unicast to every discovered
//!   peer on the data port, and inbound RNS packets are fed back into the
//!   engine. The self-announce targets an `lxmf.delivery` destination with an
//!   LXMF display-name payload, so LXMF apps (Sideband / Columba) list the
//!   board as a messageable peer. Wire format + the engine-facing
//!   [`rns_auto::RnsAutoInterface`] live in [`rns_auto`].
//!
//! Board: Heltec WiFi LoRa 32 V3 (ESP32-S3). OLED `SDA=17 SCL=18 RST=21`,
//! `Vext=GPIO36` (active-low). WiFi creds come from build-time env
//! `WIFI_SSID` / `WIFI_PASSWORD` so they never enter source; optional
//! `WIFI_BSSID` pins the STA to one AP (mesh units don't bridge the
//! link-local multicast RNS discovery rides on).

#![no_std]
#![no_main]

extern crate alloc;

mod rns_auto;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::{Rng, TrngSource};
use esp_hal::time::{Instant, Rate};
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

use embassy_executor::Spawner;
use embassy_futures::select::{select4, Either4};
use embassy_net::udp::{PacketMetadata, UdpMetadata, UdpSocket};
use embassy_net::{Config as NetConfig, IpAddress, Runner, StackResources};
use embassy_time::{Duration, Ticker};
use static_cell::StaticCell;

use core::fmt::Write as _;
use embedded_graphics::mono_font::ascii::FONT_6X10;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};
use heapless::{String as HString, Vec as HVec};
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{self, Config as WifiConfig, Interface as WifiStaInterface, PowerSaveMode};

use personal_rns::engine::{
    ingest, tick, DefaultEngineState, InstantMillis, ReannounceSchedule, SelfAnnounceConfig,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::Interface;

esp_app_desc!();

/// WiFi credentials, baked in at build time (never committed to source):
/// `WIFI_SSID="…" WIFI_PASSWORD="…" cargo build --release`.
const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");
/// Optional BSSID to pin the STA to (e.g. `WIFI_BSSID=24:2d:6c:11:aa:48`). On a
/// multi-unit mesh, link-local multicast doesn't bridge between units, so RNS
/// AutoInterface discovery only works when this node shares a physical AP with
/// its peers. Unset = associate to the strongest BSSID (may roam between units).
const WIFI_BSSID: Option<&str> = option_env!("WIFI_BSSID");

fn now_millis() -> InstantMillis {
    InstantMillis(Instant::now().duration_since_epoch().as_millis())
}

/// Eight bytes of CSPRNG-grade entropy from the hardware TRNG (true-random
/// while the radio is up and a [`TrngSource`] is installed). Drives the
/// engine's announce-id minting and rebroadcast jitter each step.
fn entropy_u64() -> u64 {
    let mut bytes = [0u8; 8];
    Rng::new().read(&mut bytes);
    u64::from_le_bytes(bytes)
}

/// Busy-wait a few ms during setup (before the async loop runs).
fn block_ms(ms: u64) {
    let target = Instant::now().duration_since_epoch().as_millis() + ms;
    while Instant::now().duration_since_epoch().as_millis() < target {}
}

/// Parse a colon-separated MAC like "24:2d:6c:11:aa:48" into 6 bytes.
fn parse_bssid(s: &str) -> Option<[u8; 6]> {
    let mut out = [0u8; 6];
    let mut n = 0;
    for part in s.split(':') {
        if n >= 6 {
            return None;
        }
        out[n] = u8::from_str_radix(part, 16).ok()?;
        n += 1;
    }
    (n == 6).then_some(out)
}

/// Extract the IPv6 source address from a received datagram's metadata.
fn ipv6_src(meta: &UdpMetadata) -> Option<core::net::Ipv6Addr> {
    match meta.endpoint.addr {
        IpAddress::Ipv6(addr) => Some(addr),
        // proto-ipv4 is off, so no other variant can occur; stay robust.
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// Authenticate an inbound discovery datagram against its source address and
/// update the peer table, logging a newly-discovered peer and which channel
/// (`mcast` beacon vs `ucast` reverse-peering) found it.
fn note_peer(
    bytes: &[u8],
    src: core::net::Ipv6Addr,
    our_link_local: &core::net::Ipv6Addr,
    peers: &mut rns_auto::PeerTable<8>,
    now_ms: u64,
    auth_failures: &mut u32,
    via: &str,
) {
    match rns_auto::classify_beacon(bytes, &src, our_link_local) {
        rns_auto::BeaconVerdict::Peer => match peers.observe(src, now_ms) {
            rns_auto::PeerObservation::NewlyDiscovered => {
                println!("HELTEC_S3 PEER+ {src} via {via} (peers={})", peers.len());
            }
            rns_auto::PeerObservation::TableFull => {
                println!("HELTEC_S3 PEER table full, dropped {src}");
            }
            rns_auto::PeerObservation::Refreshed => {}
        },
        rns_auto::BeaconVerdict::AuthenticationFailed => {
            *auth_failures = auth_failures.wrapping_add(1);
        }
        rns_auto::BeaconVerdict::SelfEcho | rns_auto::BeaconVerdict::TooShort => {}
    }
}

/// The embassy-net background task: polls the WiFi device and runs the IP stack.
/// Must own the device + resources for 'static, hence the `StaticCell` below.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiStaInterface<'static>>) -> ! {
    runner.run().await
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // esp-radio needs a heap and a preemptive scheduler, started before the radio.
    esp_alloc::heap_allocator!(size: 72 * 1024);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // Install the hardware TRNG as the system entropy source before the radio
    // comes up: esp-radio draws WPA entropy from it, and the engine mints
    // announce ids + rebroadcast jitter from it via `entropy_u64`. Held alive
    // for the whole program (true-random only while the radio is running).
    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    println!("HELTEC_S3: boot — Personal Reticulum on ESP32-S3, WiFi bring-up (RNSAutoInterface M5)");

    // --- Engine: announcing node, pinned fixture identity. ---
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);

    // We announce an `lxmf.delivery` destination so LXMF apps (Sideband /
    // Columba) surface us as a messageable peer — a bare `personal.node`
    // announce is a valid RNS announce but matches none of the aspects an LXMF
    // app's announce stream filters for. The app_data is the exact shape LXMF
    // 0.9.9 emits: `msgpack([display_name_bytes, stamp_cost])` — the name
    // bin8-encoded (LXMF packs it as bytes, not a string) and a nil stamp cost.
    const DISPLAY_NAME: &str = "Personal Node (S3)";
    let mut lxmf_app_data: HVec<u8, 64> = HVec::new();
    let _ = lxmf_app_data.push(0x92); // msgpack: 2-element array
    let _ = lxmf_app_data.push(0xc4); // msgpack: bin8
    let _ = lxmf_app_data.push(DISPLAY_NAME.len() as u8);
    let _ = lxmf_app_data.extend_from_slice(DISPLAY_NAME.as_bytes());
    let _ = lxmf_app_data.push(0xc0); // msgpack: nil (no stamp cost)

    let mut state: DefaultEngineState = DefaultEngineState::announcing(
        &secret_key,
        SelfAnnounceConfig {
            app_name: "lxmf",
            aspects: &["delivery"],
            app_data: lxmf_app_data.as_slice(),
            // Fast re-announce so a listening node reliably catches us during
            // bring-up — the first announce can fire before any peer is
            // discovered. Production cadence is the 6 h `default()`.
            schedule: ReannounceSchedule::every(15_000),
        },
    )
    .expect("static self-announce config is valid");
    drop(secret_key);
    let mut dest_hex: HString<16> = HString::new();
    if let Some(dest) = state.self_announced_destination() {
        for byte in dest.as_bytes().iter().take(4) {
            let _ = write!(dest_hex, "{byte:02x}");
        }
    }

    // --- OLED (Heltec V3 pinout). ---
    let mut vext = Output::new(peripherals.GPIO36, Level::Low, OutputConfig::default());
    vext.set_low();
    let mut oled_rst = Output::new(peripherals.GPIO21, Level::High, OutputConfig::default());
    oled_rst.set_low();
    block_ms(20);
    oled_rst.set_high();
    block_ms(20);
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("i2c0")
    .with_sda(peripherals.GPIO17)
    .with_scl(peripherals.GPIO18);
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate0,
    )
    .into_buffered_graphics_mode();
    let oled_ok = display.init().is_ok();
    let text = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);

    // Show "connecting" before we block on association.
    if oled_ok {
        display.clear_buffer();
        let _ = Text::with_baseline("Personal RNS  S3", Point::new(0, 0), text, Baseline::Top)
            .draw(&mut display);
        let mut l: HString<24> = HString::new();
        let _ = write!(l, "node {dest_hex}");
        let _ = Text::with_baseline(&l, Point::new(0, 13), text, Baseline::Top).draw(&mut display);
        let _ = Text::with_baseline("WiFi: connecting", Point::new(0, 26), text, Baseline::Top)
            .draw(&mut display);
        let _ = display.flush();
    }

    // --- WiFi association (esp-radio). ---
    let (mut controller, interfaces) =
        wifi::new(peripherals.WIFI, Default::default()).expect("esp-radio wifi::new");
    let mut sta = StationConfig::default()
        .with_ssid(WIFI_SSID)
        .with_password(WIFI_PASSWORD.into());
    // Pin to one AP unit when WIFI_BSSID is set — mesh networks don't bridge
    // link-local multicast between units, so discovery needs us on the peer's
    // physical AP (see WIFI_BSSID docs).
    if let Some(bssid_str) = WIFI_BSSID {
        match parse_bssid(bssid_str) {
            Some(bssid) => {
                sta = sta.with_bssid(bssid);
                println!("HELTEC_S3 WIFI pinning to bssid {bssid_str}");
            }
            None => println!("HELTEC_S3 WIFI ignoring malformed WIFI_BSSID '{bssid_str}'"),
        }
    }
    controller
        .set_config(&WifiConfig::Station(sta))
        .expect("set STA config");
    // Disable modem power save: a sleeping STA misses AP-buffered multicast,
    // which is exactly how RNS discovery beacons arrive. An always-on receiver
    // is worth the power on a desk-tethered node.
    controller
        .set_power_saving(PowerSaveMode::None)
        .expect("disable wifi power save");

    println!("HELTEC_S3 WIFI connecting (ssid len {})", WIFI_SSID.len());
    let wifi_line = match controller.connect_async().await {
        Ok(_) => {
            println!("HELTEC_S3 WIFI connected");
            "WiFi: UP"
        }
        Err(e) => {
            println!("HELTEC_S3 WIFI connect failed: {e:?}");
            "WiFi: FAIL"
        }
    };

    // Which AP did we land on? On a multi-AP / mesh LAN, cross-AP multicast may
    // not bridge to the node sending announces — so correlate this BSSID with
    // whether inbound discovery works this boot. (Diagnostic.)
    if let Ok(ap) = controller.ap_info() {
        let b = ap.bssid;
        println!(
            "HELTEC_S3 AP bssid {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        );
    }

    // --- M2: IP stack (embassy-net) with SLAAC → IPv6 link-local. ---
    // Capture the STA MAC before the device moves into the stack; we use it to
    // report the link-local (embassy-net assigns it from the MAC via EUI-64, but
    // config_v6() only surfaces static/global addresses, not link-local).
    let sta_mac = interfaces.station.mac_address();

    let net_config = NetConfig::slaac();
    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let resources = RESOURCES.init(StackResources::new());
    let (stack, runner) = embassy_net::new(
        interfaces.station,
        net_config,
        resources,
        0x5eed_1234_c0ff_ee01,
    );
    spawner.spawn(net_task(runner).expect("spawn net_task"));

    stack.wait_link_up().await;
    // The SLAAC link-local (fe80::<EUI-64 of the MAC>) — the source address a
    // peer sees on our beacons, and the address our own peering token hashes.
    let our_link_local = rns_auto::link_local_from_mac(sta_mac);
    println!("HELTEC_S3 NET link up; IPv6 link-local {our_link_local}");
    let mut ip6_line: HString<24> = HString::new();
    // The last two hextets are enough to recognise us on the 21-char OLED line.
    let _ = write!(
        ip6_line,
        "ll ..{:x}:{:x}",
        0xfe00u16 | sta_mac[3] as u16,
        ((sta_mac[4] as u16) << 8) | sta_mac[5] as u16,
    );

    // --- M4: RNS AutoInterface discovery handshake (wire-exact vs RNS 1.3.1). ---
    // Join the discovery multicast group and exchange peering beacons: a beacon
    // is sha256("reticulum" ++ <sender link-local>); a receiver authenticates it
    // against the datagram's source address. We beacon ours and peer with any
    // node whose beacon checks out. (Data plane on the data port is M5.)
    match stack.join_multicast_group(IpAddress::Ipv6(rns_auto::DISCOVERY_GROUP)) {
        Ok(()) => println!("HELTEC_S3 MCAST joined {}", rns_auto::DISCOVERY_GROUP),
        Err(e) => println!("HELTEC_S3 MCAST join failed: {e:?}"),
    }

    let mut rx_meta = [PacketMetadata::EMPTY; 8];
    let mut rx_buf = [0u8; 512];
    let mut tx_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_buf = [0u8; 512];
    // `bind` needs &mut; `recv_from`/`send_to` take &self. This socket carries
    // multicast discovery (beacons in + out) on the discovery port.
    let mut disc = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    disc.bind(rns_auto::DISCOVERY_PORT).expect("bind discovery port");

    // Second socket for the RNS unicast reverse-peering channel (29717). A peer
    // that hears our multicast unicasts its token back here, so we discover it
    // even when our own multicast RX is blocked (e.g. a mesh AP that won't
    // forward the group across its backhaul) — the mechanism that makes the
    // reference AutoInterface robust to one-way multicast.
    let mut udisc_rx_meta = [PacketMetadata::EMPTY; 8];
    let mut udisc_rx_buf = [0u8; 512];
    let mut udisc_tx_meta = [PacketMetadata::EMPTY; 8];
    let mut udisc_tx_buf = [0u8; 512];
    let mut udisc = UdpSocket::new(
        stack,
        &mut udisc_rx_meta,
        &mut udisc_rx_buf,
        &mut udisc_tx_meta,
        &mut udisc_tx_buf,
    );
    udisc
        .bind(rns_auto::UNICAST_DISCOVERY_PORT)
        .expect("bind unicast discovery port");

    // Data socket (42671): inbound RNS packets land here, and the engine's
    // writes are unicast from here to each discovered peer's data port. Buffers
    // hold a full HW_MTU datagram with headroom.
    let mut data_rx_meta = [PacketMetadata::EMPTY; 8];
    let mut data_rx_buf = [0u8; 1280];
    let mut data_tx_meta = [PacketMetadata::EMPTY; 8];
    let mut data_tx_buf = [0u8; 1280];
    let mut data = UdpSocket::new(
        stack,
        &mut data_rx_meta,
        &mut data_rx_buf,
        &mut data_tx_meta,
        &mut data_tx_buf,
    );
    data.bind(rns_auto::DATA_PORT).expect("bind data port");

    // Our peering token — the multicast beacon payload and the unicast
    // reverse-peering payload. Printed for the golden cross-check against
    // `sha256(b"reticulum" + b"<our ll>")` (laptop-computed).
    let our_token = rns_auto::peering_token(&our_link_local);
    println!(
        "HELTEC_S3 token {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}.. for {our_link_local}",
        our_token[0],
        our_token[1],
        our_token[2],
        our_token[3],
        our_token[4],
        our_token[5],
        our_token[6],
        our_token[7],
    );

    let mut peers: rns_auto::PeerTable<8> = rns_auto::PeerTable::new();

    // The engine-facing RNS AutoInterface: the engine writes to it (we fan
    // those writes out to peers below), and inbound datagrams are injected into
    // it for the engine to read. Registering it lets the engine originate its
    // self-announce onto this interface.
    let mut iface = rns_auto::RnsAutoInterface::new();
    state
        .register_routable_interface(&iface)
        .expect("RnsAutoInterface is Connected and transmits");
    println!("HELTEC_S3 IFACE registered, data plane on {}", rns_auto::DATA_PORT);

    // --- Discovery + engine loop. ---
    // `select3` parks on a multicast datagram (29716), a unicast reverse-peering
    // datagram (29717), and a 1.6 s beacon tick at once, handling whichever
    // fires. Inbound datagrams update the peer table (logging which channel found
    // the peer); the tick drives the engine, emits our multicast beacon,
    // reverse-announces to known peers, ages out stale peers, and redraws the OLED.
    let _controller = controller; // keep the radio alive (dropping disconnects)
    let mut rx = [0u8; 256];
    let mut urx = [0u8; 256];
    let mut drx = [0u8; 1280];
    let mut beacons: u32 = 0;
    let mut auth_failures: u32 = 0;
    let mut cycle: u32 = 0;
    let mut beacon_ticker = Ticker::every(Duration::from_millis(1600));
    loop {
        match select4(
            disc.recv_from(&mut rx),
            udisc.recv_from(&mut urx),
            data.recv_from(&mut drx),
            beacon_ticker.next(),
        )
        .await
        {
            // Multicast discovery datagram (29716).
            Either4::First(Ok((n, meta))) => {
                if let Some(src) = ipv6_src(&meta) {
                    note_peer(
                        &rx[..n],
                        src,
                        &our_link_local,
                        &mut peers,
                        now_millis().0,
                        &mut auth_failures,
                        "mcast",
                    );
                }
            }
            Either4::First(Err(e)) => println!("HELTEC_S3 RECV mcast err: {e:?}"),
            // Unicast reverse-peering datagram (29717).
            Either4::Second(Ok((n, meta))) => {
                if let Some(src) = ipv6_src(&meta) {
                    note_peer(
                        &urx[..n],
                        src,
                        &our_link_local,
                        &mut peers,
                        now_millis().0,
                        &mut auth_failures,
                        "ucast",
                    );
                }
            }
            Either4::Second(Err(e)) => println!("HELTEC_S3 RECV ucast err: {e:?}"),
            // Data datagram (42671): an inbound RNS packet. Queue it on the
            // interface and drain it into the engine via `ingest`.
            Either4::Third(Ok((n, meta))) => {
                let src = ipv6_src(&meta);
                if iface.inject_inbound(&drx[..n]) {
                    let now = now_millis();
                    let entropy = entropy_u64();
                    let mut scratch = [0u8; rns_auto::HW_MTU];
                    // `read_inbound` stamps arrived_at + source_interface for us.
                    while let Ok(Some(packet)) = iface.read_inbound(&mut scratch, now) {
                        let out = ingest(&mut state, core::slice::from_ref(&packet), entropy);
                        if out.accepted_announce_count() > 0 {
                            match src {
                                Some(s) => println!(
                                    "HELTEC_S3 RX announce from {s} accepted={} routes={}",
                                    out.accepted_announce_count(),
                                    state.route_count(),
                                ),
                                None => println!(
                                    "HELTEC_S3 RX announce accepted={}",
                                    out.accepted_announce_count(),
                                ),
                            }
                        }
                    }
                } else {
                    println!("HELTEC_S3 RX data {n}B dropped (oversize/full)");
                }
            }
            Either4::Third(Err(e)) => println!("HELTEC_S3 RECV data err: {e:?}"),
            // Beacon tick: drive the engine, originate + fan out the
            // self-announce, emit discovery beacons, age out peers.
            Either4::Fourth(_) => {
                let now = now_millis();
                let entropy = entropy_u64();
                let _ = tick(&mut state, now, entropy);
                cycle = cycle.wrapping_add(1);

                // Self-announce when due: hand the framed packet to the
                // interface; the fanout below unicasts it to every peer.
                let mut announce_buf = [0u8; rns_auto::HW_MTU];
                if !state.registered_interfaces().is_empty() {
                    if let Some(len) =
                        state.write_due_self_announce(now, entropy, &mut announce_buf)
                    {
                        match iface.write(&announce_buf[..len]) {
                            Ok(()) => {
                                println!("HELTEC_S3 SELF-ANNOUNCE {len}B (node {dest_hex})")
                            }
                            Err(e) => println!("HELTEC_S3 SELF-ANNOUNCE write err: {e:?}"),
                        }
                    }
                }

                // Multicast discovery beacon.
                match disc
                    .send_to(
                        &our_token,
                        (IpAddress::Ipv6(rns_auto::DISCOVERY_GROUP), rns_auto::DISCOVERY_PORT),
                    )
                    .await
                {
                    Ok(()) => beacons = beacons.wrapping_add(1),
                    Err(e) => println!("HELTEC_S3 BEACON send err: {e:?}"),
                }

                // Reverse-peering: unicast our token to each known peer's unicast
                // discovery port (RNS AutoInterface.py:394-401,477-489), so a peer
                // that can't hear our multicast still keeps us discovered.
                if cycle % 3 == 0 && peers.len() != 0 {
                    let mut targets: HVec<core::net::Ipv6Addr, 8> = HVec::new();
                    for addr in peers.addrs() {
                        let _ = targets.push(addr);
                    }
                    for addr in targets {
                        let _ = udisc
                            .send_to(
                                &our_token,
                                (IpAddress::Ipv6(addr), rns_auto::UNICAST_DISCOVERY_PORT),
                            )
                            .await;
                    }
                }

                let pruned = peers.prune(now.0);
                if pruned > 0 {
                    println!("HELTEC_S3 pruned {pruned} stale peer(s) (peers={})", peers.len());
                }

                // Fan the engine's outbound packets out as unicast to every
                // discovered peer's data port (RNS `process_outgoing`). With no
                // peers yet the packet is dropped; the 15 s re-announce recovers.
                {
                    let mut targets: HVec<core::net::Ipv6Addr, 8> = HVec::new();
                    for addr in peers.addrs() {
                        let _ = targets.push(addr);
                    }
                    let mut out_scratch = [0u8; rns_auto::HW_MTU];
                    while let Some(len) = iface.take_outbound(&mut out_scratch) {
                        if targets.is_empty() {
                            println!("HELTEC_S3 TX {len}B dropped — no peers yet");
                            continue;
                        }
                        for addr in &targets {
                            if let Err(e) = data
                                .send_to(
                                    &out_scratch[..len],
                                    (IpAddress::Ipv6(*addr), rns_auto::DATA_PORT),
                                )
                                .await
                            {
                                println!("HELTEC_S3 TX err to {addr}: {e:?}");
                            }
                        }
                        println!("HELTEC_S3 TX {len}B to {} peer(s)", targets.len());
                    }
                }

                println!(
                    "HELTEC_S3_CYCLE {cycle} now_ms={} tick={} {wifi_line} beacons={beacons} peers={} authfail={auth_failures}",
                    now.0,
                    state.tick_count(),
                    peers.len(),
                );
                if oled_ok {
                    display.clear_buffer();
                    let _ =
                        Text::with_baseline("Personal RNS  S3", Point::new(0, 0), text, Baseline::Top)
                            .draw(&mut display);
                    let mut l: HString<24> = HString::new();
                    let _ = write!(l, "node {dest_hex}");
                    let _ = Text::with_baseline(&l, Point::new(0, 13), text, Baseline::Top)
                        .draw(&mut display);
                    let _ = Text::with_baseline(wifi_line, Point::new(0, 26), text, Baseline::Top)
                        .draw(&mut display);
                    let _ = Text::with_baseline(&ip6_line, Point::new(0, 39), text, Baseline::Top)
                        .draw(&mut display);
                    l.clear();
                    let _ = write!(l, "peers {} beacons {beacons}", peers.len());
                    let _ = Text::with_baseline(&l, Point::new(0, 52), text, Baseline::Top)
                        .draw(&mut display);
                    let _ = display.flush();
                }
            }
        }
    }
}
