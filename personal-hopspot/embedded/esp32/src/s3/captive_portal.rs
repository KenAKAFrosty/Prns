use super::connectivity::net_task;
use super::*;

pub(super) const HTTP_SERVER_WORKERS: usize = 4;
const EMBASSY_INTERNAL_SOCKET_COUNT: usize = 1;
const WIFI_AUTO_UDP_SOCKET_COUNT: usize = 3;
const CAPTIVE_PORTAL_UDP_SOCKET_COUNT: usize = 2;
const TCP_RENDEZVOUS_SOCKET_COUNT: usize = 1;
const AP_STACK_SOCKET_CAPACITY: usize = EMBASSY_INTERNAL_SOCKET_COUNT
    + WIFI_AUTO_UDP_SOCKET_COUNT
    + CAPTIVE_PORTAL_UDP_SOCKET_COUNT
    + HTTP_SERVER_WORKERS
    + TCP_RENDEZVOUS_SOCKET_COUNT;

/// A random per-boot SoftAP SSID suffix, cached so every `set_config` within a boot reuses the same
/// name (regenerating per call would flap the SSID). 0 = unset. Random rather than MAC-derived so the
/// AP name leaks no device identity; it re-rolls on reboot, which is acceptable (preferred, even).
static AP_SSID_SUFFIX: AtomicU64 = AtomicU64::new(0);

pub(super) fn ap_ssid_suffix() -> u16 {
    let mut suffix = AP_SSID_SUFFIX.load(Ordering::Relaxed);
    if suffix == 0 {
        let mut r = [0u8; 2];
        Rng::new().read(&mut r);
        suffix = u64::from(u16::from_le_bytes(r)) | 1;
        AP_SSID_SUFFIX.store(suffix, Ordering::Relaxed);
    }
    suffix as u16
}

pub(super) fn ap_ssid() -> String {
    alloc::format!("Hopspot-{:04X}", ap_ssid_suffix())
}

pub(super) fn ap_config(channel: Option<u8>) -> AccessPointConfig {
    let config = AccessPointConfig::default()
        .with_ssid(ap_ssid())
        .with_max_connections(4);
    match channel {
        Some(channel) => config.with_channel(channel),
        None => config,
    }
}

/// The Wi-Fi mode to request for a station config. Once discovery resolves the uplink channel,
/// APSTA starts the Hopspot SoftAP on that same channel and keeps it alongside the station.
pub(super) fn station_wifi_mode(
    station: StationConfig,
    ap_enabled: bool,
    channel: Option<u8>,
) -> WifiConfig {
    if ap_enabled {
        return WifiConfig::AccessPointStation(station, ap_config(channel));
    }
    WifiConfig::Station(station)
}

/// Stand a second embassy-net Stack on the AP netif and drive it, so the SoftAP is a real interface
/// (APSTA). Sized like the station's; the AP takes the station MAC + 1 for its link-local (matching
/// the SoftAP's own BSSID) so the two netifs are distinct.
pub(super) fn build_ap_netif(
    spawner: &Spawner,
    ap_iface: WifiStaDevice<'static>,
    mac: [u8; 6],
) -> Stack<'static> {
    let mut ap_mac = mac;
    ap_mac[5] = ap_mac[5].wrapping_add(1);
    let ap_link_local = wifi_auto_contract::link_local_from_mac(MacAddress::new(ap_mac));
    // The SoftAP is the gateway, not a DHCP client: a static IPv4 (192.168.4.1/24) lets it serve DHCP +
    // host the TCP rendezvous, plus the static v6 link-local for Wi-Fi Auto's UDP. (The IPv4 multicast
    // path is moot anyway — the SoftAP can't pass multicast; see the rendezvous DHCP server below.)
    let mut ap_net_config = NetConfig::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 4, 1), 24),
        gateway: None,
        dns_servers: Default::default(),
    });
    ap_net_config.ipv6 = ConfigV6::Static(StaticConfigV6 {
        address: Ipv6Cidr::new(ap_link_local, 64),
        gateway: None,
        dns_servers: Default::default(),
    });
    // Socket-set storage is ordinary software state, not DMA/control memory. Keep it in PSRAM so
    // the radio blobs retain the scarce internal SRAM needed for concurrent AP + station RX.
    let ap_resources =
        crate::storage::allocate_psram(StackResources::<AP_STACK_SOCKET_CAPACITY>::new());
    let ap_seed = {
        let mut b = [0u8; 8];
        Rng::new().read(&mut b);
        u64::from_le_bytes(b)
    };
    let (ap_stack, ap_runner) = embassy_net::new(ap_iface, ap_net_config, ap_resources, ap_seed);
    spawner.spawn(net_task(ap_runner).expect("ap net task fits"));
    ap_stack
}

/// A minimal DHCPv4 server for the SoftAP. A device joining "Hopspot" DISCOVERs/REQUESTs and we lease it
/// 192.168.4.2 with the SoftAP (192.168.4.1) as its router + DNS. The lease is incidental; the *gateway*
/// is the point: once the joiner's default route is the Heltec, its Wi-Fi Auto client auto-dials the TCP
/// rendezvous on the gateway (port 42699), sidestepping the SoftAP's broken multicast entirely. One
/// static lease is enough to start; the wire format is hand-rolled (embassy-net ships only a client).
#[embassy_executor::task]
pub(super) async fn dhcp_server_task(stack: Stack<'static>) -> ! {
    let rx_meta: &'static mut [PacketMetadata] = alloc::vec![PacketMetadata::EMPTY; 4].leak();
    let rx_buf: &'static mut [u8] = alloc::vec![0u8; 1024].leak();
    let tx_meta: &'static mut [PacketMetadata] = alloc::vec![PacketMetadata::EMPTY; 4].leak();
    let tx_buf: &'static mut [u8] = alloc::vec![0u8; 1024].leak();
    let mut sock = UdpSocket::new(stack, rx_meta, rx_buf, tx_meta, tx_buf);
    if let Err(error) = sock.bind(67u16) {
        log::error!("dhcp: bind failed: {error:?}");
        loop {
            Timer::after(Duration::from_secs(3600)).await;
        }
    }
    let req: &'static mut [u8] = alloc::vec![0u8; 600].leak();
    let reply: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    loop {
        let (len, _meta) = match sock.recv_from(&mut req[..]).await {
            Ok(received) => received,
            Err(error) => {
                log::warn!("dhcp: receive failed: {error:?}");
                continue;
            }
        };
        // BOOTREQUEST (op=1) with the DHCP magic cookie + a parseable message-type option.
        if len < 240 || req[0] != 1 || req[236..240] != [0x63, 0x82, 0x53, 0x63] {
            continue;
        }
        let (request_name, reply_name, reply_type) = match dhcp_message_type(&req[240..len]) {
            Some(1) => ("DISCOVER", "OFFER", 2),
            Some(3) => ("REQUEST", "ACK", 5),
            _ => continue,
        };
        let n = build_dhcp_reply(&req[..len], &mut reply[..], reply_type);
        let m = &req[28..34];
        log::info!(
            "dhcp: {request_name} from {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            m[0],
            m[1],
            m[2],
            m[3],
            m[4],
            m[5]
        );
        let delivery = match sock
            .send_to(
                &reply[..n],
                (IpAddress::Ipv4(Ipv4Address::new(255, 255, 255, 255)), 68u16),
            )
            .await
        {
            Ok(()) => Some("limited-broadcast"),
            Err(limited_error) => {
                log::warn!(
                    "dhcp: {reply_name} limited-broadcast transmission failed: {limited_error:?}"
                );
                match sock
                    .send_to(
                        &reply[..n],
                        (IpAddress::Ipv4(Ipv4Address::new(192, 168, 4, 255)), 68u16),
                    )
                    .await
                {
                    Ok(()) => Some("directed-broadcast"),
                    Err(directed_error) => {
                        log::error!(
                            "dhcp: {reply_name} directed-broadcast transmission failed: {directed_error:?}"
                        );
                        None
                    }
                }
            }
        };
        if let Some(delivery) = delivery {
            log::info!(
                "dhcp: {reply_name} 192.168.4.2 sent via {delivery} to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                m[0],
                m[1],
                m[2],
                m[3],
                m[4],
                m[5]
            );
        }
    }
}

/// Scan DHCP options (TLV) for option 53 (message type); returns its value (1=DISCOVER, 3=REQUEST, ...).
fn dhcp_message_type(mut opts: &[u8]) -> Option<u8> {
    while let Some(&code) = opts.first() {
        if code == 255 {
            return None; // end
        }
        if code == 0 {
            opts = &opts[1..]; // pad
            continue;
        }
        let len = *opts.get(1)? as usize;
        let val = opts.get(2..2 + len)?;
        if code == 53 {
            return val.first().copied();
        }
        opts = &opts[2 + len..];
    }
    None
}

/// Build a BOOTREPLY (OFFER/ACK) leasing 192.168.4.2 with the SoftAP (192.168.4.1) as server, router,
/// and DNS; returns the reply length. `msg_type` is 2 (OFFER) or 5 (ACK).
fn build_dhcp_reply(req: &[u8], out: &mut [u8], msg_type: u8) -> usize {
    out.fill(0);
    out[0] = 2; // op = BOOTREPLY
    out[1] = 1; // htype = ethernet
    out[2] = 6; // hlen
    out[4..8].copy_from_slice(&req[4..8]); // xid
    out[10] = 0x80; // flags: broadcast (client has no IP yet)
    out[16..20].copy_from_slice(&[192, 168, 4, 2]); // yiaddr (the lease)
    out[20..24].copy_from_slice(&AP_IPV4); // siaddr (server)
    out[28..44].copy_from_slice(&req[28..44]); // chaddr
    out[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]); // magic cookie
    let mut pos = 240;
    if !write_dhcp_option(out, &mut pos, 53, &[msg_type]) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 54, &AP_IPV4) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 51, &[0, 0, 0x0E, 0x10]) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 1, &[255, 255, 255, 0]) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 3, &AP_IPV4) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 6, &AP_IPV4) {
        return finish_dhcp_options(out, pos);
    }
    if !write_dhcp_option(out, &mut pos, 114, CAPTIVE_PORTAL_API_URL.as_bytes()) {
        return finish_dhcp_options(out, pos);
    }
    finish_dhcp_options(out, pos)
}

fn write_dhcp_option(out: &mut [u8], pos: &mut usize, code: u8, value: &[u8]) -> bool {
    if *pos + 2 + value.len() + 1 > out.len() || value.len() > u8::MAX as usize {
        return false;
    }
    out[*pos] = code;
    out[*pos + 1] = value.len() as u8;
    out[*pos + 2..*pos + 2 + value.len()].copy_from_slice(value);
    *pos += 2 + value.len();
    true
}

fn finish_dhcp_options(out: &mut [u8], pos: usize) -> usize {
    let pos = pos.min(out.len().saturating_sub(1));
    out[pos] = 255; // end
    pos + 1
}

/// Captive DNS for the SoftAP: every A/ANY query resolves to 192.168.4.1, which makes
/// OS connectivity checks and typed hostnames land on the Hopspot HTTP server.
#[embassy_executor::task]
pub(super) async fn dns_server_task(stack: Stack<'static>) -> ! {
    let rx_meta: &'static mut [PacketMetadata] = alloc::vec![PacketMetadata::EMPTY; 4].leak();
    let rx_buf: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    let tx_meta: &'static mut [PacketMetadata] = alloc::vec![PacketMetadata::EMPTY; 4].leak();
    let tx_buf: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    let mut sock = UdpSocket::new(stack, rx_meta, rx_buf, tx_meta, tx_buf);
    if let Err(error) = sock.bind(53u16) {
        log::error!("dns: bind failed: {error:?}");
        loop {
            Timer::after(Duration::from_secs(3600)).await;
        }
    }
    let req: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    let reply: &'static mut [u8] = alloc::vec![0u8; 512].leak();
    loop {
        let (len, meta) = match sock.recv_from(&mut req[..]).await {
            Ok(received) => received,
            Err(error) => {
                log::warn!("dns: receive failed: {error:?}");
                continue;
            }
        };
        let Some(reply_len) = build_dns_reply(&req[..len], &mut reply[..]) else {
            continue;
        };
        match sock.send_to(&reply[..reply_len], meta.endpoint).await {
            Ok(()) => log::info!("dns: response sent to {:?}", meta.endpoint),
            Err(error) => log::warn!("dns: response to {:?} failed: {error:?}", meta.endpoint),
        }
    }
}

fn build_dns_reply(req: &[u8], out: &mut [u8]) -> Option<usize> {
    if req.len() < 12 || req[2] & 0x80 != 0 {
        return None;
    }
    let qdcount = u16::from_be_bytes([req[4], req[5]]);
    if qdcount == 0 {
        return None;
    }
    let (question_end, qtype) = dns_question_end(req)?;
    let answer_a = qtype == 1 || qtype == 255; // A or ANY.
    let reply_len = question_end + if answer_a { 16 } else { 0 };
    if reply_len > out.len() {
        return None;
    }

    out[..question_end].copy_from_slice(&req[..question_end]);
    out[2] = 0x81; // response + recursion desired
    out[3] = 0x80; // recursion available, no error
    out[4..6].copy_from_slice(&1u16.to_be_bytes());
    out[6..8].copy_from_slice(&(answer_a as u16).to_be_bytes());
    out[8..10].copy_from_slice(&0u16.to_be_bytes());
    out[10..12].copy_from_slice(&0u16.to_be_bytes());

    if answer_a {
        let mut pos = question_end;
        out[pos..pos + 2].copy_from_slice(&[0xC0, 0x0C]); // pointer to query name
        pos += 2;
        out[pos..pos + 2].copy_from_slice(&1u16.to_be_bytes()); // A
        pos += 2;
        out[pos..pos + 2].copy_from_slice(&1u16.to_be_bytes()); // IN
        pos += 2;
        out[pos..pos + 4].copy_from_slice(&30u32.to_be_bytes()); // short TTL
        pos += 4;
        out[pos..pos + 2].copy_from_slice(&4u16.to_be_bytes());
        pos += 2;
        out[pos..pos + 4].copy_from_slice(&AP_IPV4);
    }

    Some(reply_len)
}

fn dns_question_end(req: &[u8]) -> Option<(usize, u16)> {
    let mut pos = 12;
    loop {
        let len = *req.get(pos)?;
        if len & 0xC0 != 0 {
            return None;
        }
        pos += 1;
        if len == 0 {
            break;
        }
        pos = pos.checked_add(len as usize)?;
        if pos > req.len() {
            return None;
        }
    }
    if pos + 4 > req.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([req[pos], req[pos + 1]]);
    Some((pos + 4, qtype))
}

const CAPTIVE_PORTAL_PAGE: &[u8] = include_bytes!("../../assets/captive-portal.html");
const HTTP_SOCKET_BUFFER_BYTES: usize = 2048;
const HTTP_REQUEST_BUFFER_BYTES: usize = 1024;

#[embassy_executor::task(pool_size = HTTP_SERVER_WORKERS)]
pub(super) async fn http_server_task(stack: Stack<'static>) -> ! {
    let rx_buffer: &'static mut [u8] = alloc::vec![0u8; HTTP_SOCKET_BUFFER_BYTES].leak();
    let tx_buffer: &'static mut [u8] = alloc::vec![0u8; HTTP_SOCKET_BUFFER_BYTES].leak();
    let request_buffer: &'static mut [u8] = alloc::vec![0u8; HTTP_REQUEST_BUFFER_BYTES].leak();
    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    socket.set_timeout(Some(Duration::from_secs(15)));

    loop {
        if let Err(error) = socket.accept(80u16).await {
            log::warn!("http: accept failed: {error:?}");
            Timer::after(Duration::from_millis(250)).await;
            continue;
        }
        let peer = socket.remote_endpoint();
        let response = serve_site_connection(&mut socket, request_buffer).await;
        socket.close();
        let flush = with_timeout(Duration::from_secs(2), socket.flush()).await;
        socket.abort();
        match response {
            Ok(response) => {
                let complete = response.written && matches!(&flush, Ok(Ok(())));
                if complete {
                    log::info!(
                        "http: method={} path={} response_complete=true peer={peer:?}",
                        response.method,
                        response.path
                    );
                } else {
                    log::warn!(
                        "http: method={} path={} response_complete=false peer={peer:?} write={:?} flush={flush:?}",
                        response.method,
                        response.path,
                        response.written
                    );
                }
            }
            Err(()) => {
                log::warn!("http: request failed before response peer={peer:?} flush={flush:?}")
            }
        }
        Timer::after(Duration::from_millis(50)).await;
    }
}

struct HttpResponseAttempt<'a> {
    method: &'a str,
    path: &'a str,
    written: bool,
}

async fn serve_site_connection<'a>(
    socket: &mut TcpSocket<'static>,
    request_buffer: &'a mut [u8],
) -> Result<HttpResponseAttempt<'a>, ()> {
    let len = read_http_request(socket, request_buffer).await?;
    let request = core::str::from_utf8(&request_buffer[..len]).map_err(|_| ())?;
    let Some(line) = request.lines().next() else {
        return Err(());
    };
    let mut parts = line.split_ascii_whitespace();
    let method = parts.next().unwrap_or("");
    let raw_path = parts.next().unwrap_or("/");
    let is_head = method == "HEAD";
    let response = if method != "GET" && !is_head {
        send_site_response(
            socket,
            SiteResponse {
                status: "405 Method Not Allowed",
                content_type: "text/plain; charset=utf-8",
                body: b"method not allowed\n",
                head_only: is_head,
            },
        )
        .await
    } else {
        let path = normalize_http_path(raw_path);
        if path == "/captive-portal/api" {
            send_captive_portal_api(socket, is_head).await
        } else if is_captive_probe_path(path) {
            send_captive_portal_redirect(socket, is_head).await
        } else if path == "/index.html" {
            send_site_response(
                socket,
                SiteResponse {
                    status: "200 OK",
                    content_type: "text/html; charset=utf-8",
                    body: CAPTIVE_PORTAL_PAGE,
                    head_only: is_head,
                },
            )
            .await
        } else {
            send_site_response(
                socket,
                SiteResponse {
                    status: "404 Not Found",
                    content_type: "text/plain; charset=utf-8",
                    body: b"not found\n",
                    head_only: is_head,
                },
            )
            .await
        }
    };
    Ok(HttpResponseAttempt {
        method,
        path: raw_path,
        written: response.is_ok(),
    })
}

async fn read_http_request(
    socket: &mut TcpSocket<'static>,
    request_buffer: &mut [u8],
) -> Result<usize, ()> {
    let mut len = 0;
    loop {
        if len == request_buffer.len() {
            return Ok(len);
        }
        let timeout = if len == 0 {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(750)
        };
        match with_timeout(timeout, socket.read(&mut request_buffer[len..])).await {
            Ok(Ok(0)) if len > 0 => return Ok(len),
            Ok(Ok(0)) => return Err(()),
            Ok(Ok(read)) => {
                len += read;
                if http_headers_complete(&request_buffer[..len]) {
                    return Ok(len);
                }
            }
            _ if len > 0 => return Ok(len),
            _ => return Err(()),
        }
    }
}

fn http_headers_complete(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|window| window == b"\r\n\r\n")
        || bytes.windows(2).any(|window| window == b"\n\n")
}

fn normalize_http_path(raw_path: &str) -> &str {
    let path = raw_path.split_once('?').map_or(raw_path, |(path, _)| path);
    let path = path.strip_prefix("/.").unwrap_or(path);
    if path.is_empty() || path == "/" {
        "/index.html"
    } else {
        path
    }
}

fn is_captive_probe_path(path: &str) -> bool {
    matches!(
        path,
        "/canonical.html"
            | "/connecttest.txt"
            | "/fwlink"
            | "/generate_204"
            | "/gen_204"
            | "/hotspot-detect.html"
            | "/kindle-wifi/wifistub.html"
            | "/library/test/success.html"
            | "/ncsi.txt"
            | "/redirect"
            | "/success.txt"
    )
}

async fn send_captive_portal_api(
    socket: &mut TcpSocket<'static>,
    head_only: bool,
) -> Result<(), ()> {
    let body = b"{\"captive\":true,\"user-portal-url\":\"http://192.168.4.1/\",\"venue-info-url\":\"http://192.168.4.1/\"}\n";
    send_site_response(
        socket,
        SiteResponse {
            status: "200 OK",
            content_type: "application/captive+json",
            body,
            head_only,
        },
    )
    .await
}

async fn send_captive_portal_redirect(
    socket: &mut TcpSocket<'static>,
    head_only: bool,
) -> Result<(), ()> {
    let body = b"<!doctype html><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Hopspot</title><p><a href=\"http://192.168.4.1/\">Open Hopspot</a></p>\n";
    let header = alloc::format!(
        "HTTP/1.1 302 Found\r\nLocation: {CAPTIVE_PORTAL_URL}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tcp_write_all(socket, header.as_bytes()).await?;
    if !head_only {
        tcp_write_all(socket, body).await?;
    }
    Ok(())
}

struct SiteResponse<'a> {
    status: &'a str,
    content_type: &'a str,
    body: &'a [u8],
    head_only: bool,
}

async fn send_site_response(
    socket: &mut TcpSocket<'static>,
    response: SiteResponse<'_>,
) -> Result<(), ()> {
    let SiteResponse {
        status,
        content_type,
        body,
        head_only,
    } = response;
    let header = alloc::format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tcp_write_all(socket, header.as_bytes()).await?;
    if !head_only {
        tcp_write_all(socket, body).await?;
    }
    Ok(())
}

async fn tcp_write_all(socket: &mut TcpSocket<'static>, mut bytes: &[u8]) -> Result<(), ()> {
    while !bytes.is_empty() {
        let written = socket.write(bytes).await.map_err(|_| ())?;
        if written == 0 {
            return Err(());
        }
        bytes = &bytes[written..];
    }
    Ok(())
}
