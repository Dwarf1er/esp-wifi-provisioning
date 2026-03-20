use std::net::{Ipv4Addr, UdpSocket};
use std::sync::mpsc;
use std::thread;

use esp_idf_svc::http::server::EspHttpServer;
use esp_idf_svc::io::Write;

use crate::error::{BoxError, ProvisioningError};

pub struct DnsServer {
    _stop_tx: mpsc::Sender<()>,
}

impl DnsServer {
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

    // Question section — only the question, not trailing additional sections
    resp.extend_from_slice(&query[12..question_end]);

    // Answer — only for A queries
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

    // Windows 10, 404 prevents it from hammering the device
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
