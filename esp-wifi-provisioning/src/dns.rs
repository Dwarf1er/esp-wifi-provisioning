//! Minimal DNS server and captive-portal HTTP handlers.
//!
//! # DNS responder
//!
//! [`DnsServer`] listens on UDP port 53 and responds to every A-record query
//! with the AP's own IP address. This is the standard "DNS hijack" technique
//! used by captive portals: when a device connects to the AP and tries to
//! resolve any hostname, it gets back the portal IP, which triggers the OS's
//! captive-portal detection UI.
//!
//! The server runs on a background thread and stops automatically when the
//! [`DnsServer`] value is dropped (via the `_stop_tx` sentinel channel).
//!
//! # Captive-portal HTTP handlers
//!
//! Different operating systems probe different URLs to detect captive portals.
//! [`register_captive_portal_handlers`] registers routes for all known probes
//! so that the setup UI appears automatically on Windows, macOS, iOS, Android,
//! and Firefox without the user having to open a browser manually.

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::mpsc;
use std::thread;

use esp_idf_svc::http::server::EspHttpServer;
use esp_idf_svc::io::Write;

use crate::error::{BoxError, ProvisioningError};

/// A running DNS server that answers every A-record query with a fixed IP.
///
/// The server stops when this value is dropped: dropping it closes the
/// `_stop_tx` sender, which the background thread detects and uses as its
/// shutdown signal.
pub struct DnsServer {
    /// Kept alive solely to signal the background thread on drop.
    _stop_tx: mpsc::Sender<()>,
}

impl DnsServer {
    /// Spawns the DNS background thread and starts listening on `0.0.0.0:53`.
    ///
    /// # Errors
    ///
    /// Returns [`ProvisioningError::HttpServer`] if the thread cannot be
    /// spawned. Bind failures are logged by the background thread and do not
    /// propagate back here (the portal will still work; OS detection just may
    /// not trigger automatically).
    pub fn start(ip: Ipv4Addr) -> Result<Self, ProvisioningError> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        thread::Builder::new()
            .stack_size(4096)
            .spawn(move || run(ip, stop_rx))
            .map_err(|e| {
                log::error!("Failed to spawn DNS thread: {e}");
                ProvisioningError::HttpServer(Box::new(e))
            })?;

        Ok(Self { _stop_tx: stop_tx })
    }
}

/// Main loop for the DNS background thread.
///
/// Binds UDP/53, then processes incoming queries until the stop channel is
/// closed. Uses a 250 ms read timeout so the loop can check the stop signal
/// without blocking indefinitely.
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
        match stop.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
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

/// Constructs a DNS response that resolves any A-record query to `ip`.
///
/// Non-A queries (AAAA, MX, etc.) receive a valid response with an empty
/// answer section (ANCOUNT = 0) rather than an error, which is the correct
/// behaviour for a hijacking resolver.
///
/// Returns `None` if the query is malformed (too short or contains a
/// compressed name pointer in the question section, which is illegal).
fn build_response(query: &[u8], ip: Ipv4Addr) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }

    let qname_end = parse_qname_end(query, 12)?;
    let question_end = qname_end.checked_add(4)?; // QTYPE (2) + QCLASS (2)
    if question_end > query.len() {
        return None;
    }

    let qtype = u16::from_be_bytes([query[qname_end], query[qname_end + 1]]);
    let is_a = qtype == 1; // QTYPE A

    let mut resp = Vec::with_capacity(question_end + 16);

    // Header (12 bytes)
    resp.push(query[0]);
    resp.push(query[1]); // Transaction ID
    resp.push(0x81); // QR=1, AA=1
    resp.push(0x80); // RCODE=0
    resp.push(query[4]);
    resp.push(query[5]); // QDCOUNT (echo)
    resp.push(0x00);
    resp.push(if is_a { 0x01 } else { 0x00 }); // ANCOUNT
    resp.push(0x00);
    resp.push(0x00); // NSCOUNT
    resp.push(0x00);
    resp.push(0x00); // ARCOUNT

    // Question section, only the question, not trailing additional sections
    resp.extend_from_slice(&query[12..question_end]);

    // Answer, only for A queries
    if is_a {
        resp.push(0xc0);
        resp.push(0x0c); // Name pointer → offset 12
        resp.push(0x00);
        resp.push(0x01); // Type A
        resp.push(0x00);
        resp.push(0x01); // Class IN
        resp.push(0x00);
        resp.push(0x00);
        resp.push(0x00);
        resp.push(0x3c); // TTL 60s
        resp.push(0x00);
        resp.push(0x04); // RDLENGTH 4
        resp.extend_from_slice(&ip.octets());
    }

    Some(resp)
}

/// Returns the byte offset of the first byte *after* the QNAME field, i.e.
/// the position of the QTYPE field.
///
/// Returns `None` if the QNAME is truncated, exceeds the buffer, or contains
/// a compression pointer (which is illegal in the question section).
fn parse_qname_end(buf: &[u8], mut offset: usize) -> Option<usize> {
    loop {
        let len = *buf.get(offset)? as usize;
        if len == 0 {
            return Some(offset + 1);
        }
        if len & 0xc0 == 0xc0 {
            return None;
        }
        offset = offset.checked_add(1)?.checked_add(len)?;
        if offset > buf.len() {
            return None;
        }
    }
}

/// Registers HTTP routes on `server` that handle OS captive-portal detection
/// probes, redirecting them to the portal UI at `http://{ip}`.
///
/// Each major OS (Windows 10/11, macOS/iOS, Android, Firefox) uses a different
/// URL to test for internet connectivity. By responding to these probes
/// correctly, the OS automatically presents the "sign in to network" UI,
/// sparing the user from having to open a browser manually.
///
/// # Errors
///
/// Returns a [`BoxError`] if any handler registration fails.
pub fn register_captive_portal_handlers(
    server: &mut EspHttpServer,
    ip: Ipv4Addr,
) -> Result<(), BoxError> {
    let portal_url = format!("http://{ip}");

    // Windows 11
    server.fn_handler(
        "/connecttest.txt",
        esp_idf_svc::http::Method::Get,
        |req| -> Result<(), BoxError> { redirect(req, "http://logout.net") },
    )?;

    // Windows 10, 404 prevents it from spamming the device
    server.fn_handler(
        "/wpad.dat",
        esp_idf_svc::http::Method::Get,
        |req| -> Result<(), BoxError> { not_found(req) },
    )?;

    // Android
    let url = portal_url.clone();
    server.fn_handler(
        "/gen_204",
        esp_idf_svc::http::Method::Get,
        move |req| -> Result<(), BoxError> { redirect(req, &url) },
    )?;
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

    // Firefox (200), must be non-empty and must NOT contain "success"
    server.fn_handler(
        "/success.txt",
        esp_idf_svc::http::Method::Get,
        |req| -> Result<(), BoxError> {
            req.into_ok_response()?.write_all(b"ok")?;
            Ok(())
        },
    )?;

    // Windows NCSI
    let url = portal_url.clone();
    server.fn_handler(
        "/ncsi.txt",
        esp_idf_svc::http::Method::Get,
        move |req| -> Result<(), BoxError> { redirect(req, &url) },
    )?;

    // Favicon, 404 to suppress unnecessary traffic
    server.fn_handler(
        "/favicon.ico",
        esp_idf_svc::http::Method::Get,
        |req| -> Result<(), BoxError> { not_found(req) },
    )?;

    Ok(())
}

/// Sends an HTTP 302 redirect to `url`.
fn redirect<'a>(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection<'a>>,
    url: &str,
) -> Result<(), BoxError> {
    req.into_response(302, None, &[("Location", url)])?
        .write_all(b"")?;
    Ok(())
}

/// Sends an HTTP 404 response with an empty body.
fn not_found<'a>(
    req: esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection<'a>>,
) -> Result<(), BoxError> {
    req.into_response(404, None, &[])?.write_all(b"")?;
    Ok(())
}
