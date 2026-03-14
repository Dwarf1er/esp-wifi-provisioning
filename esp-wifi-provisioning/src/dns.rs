use std::net::{Ipv4Addr, UdpSocket};
use std::sync::mpsc;
use std::thread;

use esp_idf_svc::http::server::EspHttpServer;
use esp_idf_svc::io::Write;

use crate::error::{BoxError, ProvisioningError};

pub struct DnsServer {
    // Dropping this sender signals the DNS thread to stop
    _stop_tx: mpsc::Sender<()>,
}

impl DnsServer {
    pub fn start(ip: Ipv4Addr) -> Result<Self, ProvisioningError> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        thread::Builder::new()
            .stack_size(4096)
            .spawn(move || run(ip, stop_rx))
            .map_err(|e| ProvisioningError::HttpServer(e.into()))?;

        Ok(Self { _stop_tx: stop_tx })
    }
}

fn run(ap_ip: Ipv4Addr, stop: mpsc::Receiver<()>) {
    let socket = match UdpSocket::bind("0.0.0.0:53") {
        Ok(s) => s,
        Err(e) => {
            log::error!("DNS server failed to bind: {e}");
            return;
        }
    };

    socket
        .set_read_timeout(Some(std::time::Duration::from_millis(250)))
        .ok();

    log::info!("DNS server listening on port 53");

    let mut buf = [0u8; 512];
    loop {
        if stop.try_recv().is_ok() {
            break;
        }

        let (len, src) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => {
                log::warn!("DNS recv error: {e}");
                continue;
            }
        };

        if len < 12 {
            continue;
        }

        match build_response(&buf[..len], ap_ip) {
            Some(response) => {
                if let Err(e) = socket.send_to(&response, src) {
                    log::warn!("DNS send error: {e}");
                }
            }
            None => log::warn!("Failed to parse DNS query from {src}"),
        }
    }

    log::info!("DNS server stopped");
}

/// Builds a DNS response that resolves any A record query to `ip`.
/// Returns None if the query is malformed.
fn build_response(query: &[u8], ip: Ipv4Addr) -> Option<Vec<u8>> {
    if query.len() < 12 || query.len() > 512 {
        return None;
    }

    let mut resp = Vec::with_capacity(query.len() + 16);

    // Header (12 bytes)
    resp.push(query[0]); // Transaction ID
    resp.push(query[1]);
    resp.push(0x81); // Flags: QR=1 (response), AA=1 (authoritative)
    resp.push(0x80); // RCODE=0 (no error)
    resp.push(query[4]); // QDCOUNT — echo question count
    resp.push(query[5]);
    resp.push(0x00); // ANCOUNT — 1 answer
    resp.push(0x01);
    resp.push(0x00); // NSCOUNT — zero
    resp.push(0x00);
    resp.push(0x00); // ARCOUNT — zero
    resp.push(0x00);

    // Question section — copy verbatim from query
    resp.extend_from_slice(&query[12..]);

    // Answer section
    resp.push(0xc0); // Name — pointer to offset 12 (0xc00c)
    resp.push(0x0c);
    resp.push(0x00); // Type A
    resp.push(0x01);
    resp.push(0x00); // Class IN
    resp.push(0x01);
    resp.push(0x00); // TTL — 60 seconds
    resp.push(0x00);
    resp.push(0x00);
    resp.push(0x3c);
    resp.push(0x00); // RDLENGTH — 4 bytes
    resp.push(0x04);
    resp.extend_from_slice(&ip.octets()); // RDATA — the AP IP

    Some(resp)
}

/// Registers OS-specific captive portal detection endpoints on the provided
/// server. Each OS probes different URLs to detect internet connectivity —
/// responding correctly here triggers the "Sign in to network" popup.
///
/// NOTE: iOS Safari will NOT show the popup if any response body contains
/// the word "Success" — avoid it in all response bodies.
pub fn register_captive_portal_handlers(
    server: &mut EspHttpServer,
    ip: Ipv4Addr,
) -> Result<(), BoxError> {
    let portal_url = format!("http://{ip}");

    // Windows 11 — expects a redirect to http://logout.net specifically
    server.fn_handler(
        "/connecttest.txt",
        esp_idf_svc::http::Method::Get,
        |req| -> Result<(), BoxError> { redirect(req, "http://logout.net") },
    )?;

    // Windows 10 — 404 stops it from hammering the device
    server.fn_handler(
        "/wpad.dat",
        esp_idf_svc::http::Method::Get,
        |req| -> Result<(), BoxError> { not_found(req) },
    )?;

    // Android
    let url = portal_url.clone();
    server.fn_handler(
        "/generate_204",
        esp_idf_svc::http::Method::Get,
        move |req| -> Result<(), BoxError> { redirect(req, &url) },
    )?;

    // Microsoft
    let url = portal_url.clone();
    server.fn_handler(
        "/redirect",
        esp_idf_svc::http::Method::Get,
        move |req| -> Result<(), BoxError> { redirect(req, &url) },
    )?;

    // Apple
    let url = portal_url.clone();
    server.fn_handler(
        "/hotspot-detect.html",
        esp_idf_svc::http::Method::Get,
        move |req| -> Result<(), BoxError> { redirect(req, &url) },
    )?;

    // Firefox (redirect)
    let url = portal_url.clone();
    server.fn_handler(
        "/canonical.html",
        esp_idf_svc::http::Method::Get,
        move |req| -> Result<(), BoxError> { redirect(req, &url) },
    )?;

    // Firefox (200 response)
    server.fn_handler(
        "/success.txt",
        esp_idf_svc::http::Method::Get,
        |req| -> Result<(), BoxError> {
            req.into_ok_response()?.write_all(b"ok")?;
            Ok(())
        },
    )?;

    // Windows
    let url = portal_url.clone();
    server.fn_handler(
        "/ncsi.txt",
        esp_idf_svc::http::Method::Get,
        move |req| -> Result<(), BoxError> { redirect(req, &url) },
    )?;

    // Favicon — 404 to avoid unnecessary traffic
    server.fn_handler(
        "/favicon.ico",
        esp_idf_svc::http::Method::Get,
        |req| -> Result<(), BoxError> { not_found(req) },
    )?;

    Ok(())
}

fn redirect<'a>(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection<'a>>,
    url: &str,
) -> Result<(), BoxError> {
    req.into_response(302, None, &[("Location", url)])?
        .write_all(b"")?;
    Ok(())
}

fn not_found<'a>(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection<'a>>,
) -> Result<(), BoxError> {
    req.into_response(404, None, &[])?.write_all(b"")?;
    Ok(())
}
