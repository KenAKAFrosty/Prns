use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use prns_core::interfaces::browser_rendezvous as contract;

use super::catalog::Catalog;

const MAX_REQUEST_HEAD_LEN: usize = 8 * 1024;
const REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(2);

pub(super) enum RequestRoute {
    Upgrade(TcpStream),
    Handled,
}

pub(super) async fn route(
    mut stream: TcpStream,
    local: SocketAddr,
    catalog: &Catalog,
) -> std::io::Result<RequestRoute> {
    let head = peek_head(&stream).await?;
    let Some((method, target)) = request_line(&head) else {
        consume_head(&mut stream, head.len()).await?;
        write_response(&mut stream, "400 Bad Request", &[], b"").await?;
        return Ok(RequestRoute::Handled);
    };
    if method == "GET" && target == contract::PATH {
        return Ok(RequestRoute::Upgrade(stream));
    }
    consume_head(&mut stream, head.len()).await?;
    if target != contract::CATALOG_PATH {
        write_response(&mut stream, "404 Not Found", &[], b"").await?;
        return Ok(RequestRoute::Handled);
    }
    match method {
        "GET" => {
            let body = catalog.render(local).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("catalog serialization failed: {error}"),
                )
            })?;
            write_response(
                &mut stream,
                "200 OK",
                &[
                    ("Content-Type", "application/json"),
                    ("Access-Control-Allow-Origin", "*"),
                    ("Cache-Control", "no-store"),
                ],
                &body,
            )
            .await?;
        }
        "OPTIONS" => {
            write_response(
                &mut stream,
                "204 No Content",
                &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Access-Control-Allow-Methods", "GET, OPTIONS"),
                    ("Access-Control-Allow-Headers", "Content-Type"),
                    ("Access-Control-Allow-Private-Network", "true"),
                    ("Access-Control-Allow-Local-Network", "true"),
                    ("Cache-Control", "no-store"),
                ],
                b"",
            )
            .await?;
        }
        _ => {
            write_response(
                &mut stream,
                "405 Method Not Allowed",
                &[("Allow", "GET, OPTIONS"), ("Cache-Control", "no-store")],
                b"",
            )
            .await?;
        }
    }
    Ok(RequestRoute::Handled)
}

async fn peek_head(stream: &TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buffer = [0u8; MAX_REQUEST_HEAD_LEN];
    loop {
        let len = stream.peek(&mut buffer).await?;
        if len == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed before an HTTP request",
            ));
        }
        if let Some(end) = header_end(&buffer[..len]) {
            return Ok(buffer[..end].to_vec());
        }
        if len == buffer.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request headers exceed the rendezvous cap",
            ));
        }
        tokio::time::sleep(REQUEST_POLL_INTERVAL).await;
    }
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn request_line(head: &[u8]) -> Option<(&str, &str)> {
    let line_end = head.windows(2).position(|window| window == b"\r\n")?;
    let line = std::str::from_utf8(&head[..line_end]).ok()?;
    let mut parts = line.split(' ');
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if parts.next().is_some() || (version != "HTTP/1.1" && version != "HTTP/1.0") {
        return None;
    }
    Some((method, target))
}

async fn consume_head(stream: &mut TcpStream, len: usize) -> std::io::Result<()> {
    let mut consumed = vec![0u8; len];
    stream.read_exact(&mut consumed).await?;
    Ok(())
}

async fn write_response(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_routes_are_classified() {
        assert_eq!(
            request_line(b"GET /prns HTTP/1.1\r\n\r\n"),
            Some(("GET", "/prns"))
        );
        assert_eq!(
            request_line(b"GET /.well-known/prns-transport?x=1 HTTP/1.1\r\n\r\n"),
            Some(("GET", "/.well-known/prns-transport?x=1"))
        );
        assert_eq!(request_line(b"GET  /prns HTTP/1.1\r\n\r\n"), None);
    }
}
