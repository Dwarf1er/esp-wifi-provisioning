//! Soft-AP lifecycle and captive-portal HTTP server.
//!
//! The entry point is [`run_portal`], which:
//!
//! 1. Starts the WiFi driver in station mode briefly to scan visible networks.
//! 2. Switches to AP mode using the caller-supplied [`ApConfig`].
//! 3. Starts a minimal DNS responder (see [`crate::dns`]) so that OS captive-
//!    portal detection redirects the user to the setup page automatically.
//! 4. Serves four HTTP endpoints: `/` (the portal UI), `/status` (last error),
//!    `/networks` (scan results as JSON), and `/connect` (credential submission).
//! 5. Blocks until valid credentials arrive over `/connect`, then tears
//!    everything down and returns them.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
use esp_idf_svc::io::Write;
use esp_idf_svc::ipv4;
use esp_idf_svc::netif::{EspNetif, NetifConfiguration};
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};

use crate::error::{BoxError, ProvisioningError};
use crate::nvs::{SSID_LEN_MAX, StoredCredentials, WPA_PASS_LEN_MAX, WPA_PASS_LEN_MIN};
use crate::portal::{index_html, networks_json};

/// Maximum allowed body size for a `/connect` POST request: SSID + password +
/// the separating newline and a trailing newline, with one byte of margin.
const MAX_BODY_SIZE: usize = SSID_LEN_MAX + WPA_PASS_LEN_MAX + 2;

/// Monotonic counter used to generate a unique `EspNetif` key each time
/// `run_portal` is called, avoiding key collisions on repeated invocations.
static NETIF_KEY_CTR: AtomicU32 = AtomicU32::new(0);

/// Security configuration for the provisioning soft-AP.
///
/// In practice almost all deployments use [`ApSecurity::Open`] (the default)
/// so that users can connect to the setup network without any prior credentials.
/// [`ApSecurity::Wpa2`] is available for deployments that require the setup AP
/// itself to be password-protected.
///
/// # Note
///
/// These variants describe the *soft-AP* security only. The target network
/// that the user is provisioning *onto* is handled separately and supports the
/// full range of auth methods that `esp-idf-svc` exposes.
#[derive(Clone, Debug)]
pub enum ApSecurity {
    /// No password, anyone in range can connect to the setup AP.
    Open,
    /// WPA2-Personal with the given password.
    ///
    /// The password must be 8–63 ASCII characters (standard WPA2 constraint).
    Wpa2(String),
}

/// Configuration for the provisioning soft-AP.
///
/// Build one explicitly when you need control over the channel or IP address,
/// otherwise the builder methods on [`Provisioner`](crate::Provisioner) cover
/// the common SSID and security cases without touching this struct directly.
#[derive(Clone, Debug)]
pub struct ApConfig {
    /// SSID broadcast by the soft-AP. Defaults to `"ESP32-Setup"`.
    pub ssid: String,
    /// Security mode for the soft-AP. Defaults to [`ApSecurity::Open`].
    pub security: ApSecurity,
    /// 802.11 channel for the soft-AP. Defaults to `6`.
    pub channel: u8,
    /// IP address assigned to the soft-AP interface (also used as the DHCP
    /// gateway and the DNS server address for captive-portal detection).
    /// Defaults to `192.168.4.1`.
    pub ip: Ipv4Addr,
}

impl Default for ApConfig {
    fn default() -> Self {
        Self {
            ssid: "ESP32-Setup".into(),
            security: ApSecurity::Open,
            channel: 6,
            ip: Ipv4Addr::new(192, 168, 4, 1),
        }
    }
}

/// Wraps an arbitrary `std::error::Error` into a [`ProvisioningError::HttpServer`].
fn http_err(e: impl std::error::Error + Send + Sync + 'static) -> ProvisioningError {
    ProvisioningError::HttpServer(Box::new(e))
}

/// Starts the captive-portal soft-AP, blocks until the user submits valid WiFi
/// credentials, then tears down the AP and returns those credentials.
///
/// # Arguments
///
/// * `wifi`, mutable reference to the WiFi driver (must not be started).
/// * `ap_config`, channel, IP, SSID, and security for the soft-AP.
/// * `last_error`, optional error string from a previous failed connection
///   attempt; shown as a banner in the portal UI so the user understands why
///   the portal reappeared.
///
/// # Errors
///
/// Returns a [`ProvisioningError`] for unrecoverable failures such as being
/// unable to start the AP, bind the HTTP server, or communicate over the WiFi
/// driver. Validation errors from the user (bad SSID, wrong password length)
/// are surfaced in the portal UI and do *not* cause this function to return an
/// error.
pub fn run_portal(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
    ap_config: &ApConfig,
    last_error: Option<&str>,
) -> Result<StoredCredentials, ProvisioningError> {
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        auth_method: AuthMethod::None,
        ..Default::default()
    }))
    .map_err(ProvisioningError::WifiDriver)?;

    wifi.start().map_err(ProvisioningError::WifiDriver)?;

    let networks = crate::wifi::scan_networks(wifi).unwrap_or_else(|e| {
        log::warn!("Scan failed ({e}), network list will be empty");
        vec![]
    });
    log::info!("Scan found {} networks", networks.len());

    wifi.stop().map_err(ProvisioningError::WifiDriver)?;

    let networks_json_str: Arc<str> = networks_json(&networks).into();

    // Each invocation needs a unique netif key or esp-idf will refuse to
    // create a second AP interface.
    let key_n = NETIF_KEY_CTR.fetch_add(1, Ordering::Relaxed);
    let key = format!("AP_{key_n}");

    let ap_netif = EspNetif::new_with_conf(&NetifConfiguration {
        ip_configuration: Some(ipv4::Configuration::Router(ipv4::RouterConfiguration {
            subnet: ipv4::Subnet {
                gateway: ap_config.ip,
                mask: ipv4::Mask(24),
            },
            dhcp_enabled: true,
            dns: Some(ap_config.ip),
            secondary_dns: None,
        })),
        key: key.as_str().try_into().unwrap(),
        ..NetifConfiguration::wifi_default_router()
    })
    .map_err(ProvisioningError::ApStart)?;

    wifi.wifi_mut()
        .swap_netif_ap(ap_netif)
        .map_err(ProvisioningError::ApStart)?;

    wifi.set_configuration(&build_ap_config(ap_config)?)
        .map_err(ProvisioningError::ApStart)?;
    wifi.start().map_err(ProvisioningError::ApStart)?;

    log::info!(
        "Soft-AP '{}' started on channel {} | connect and visit http://{}",
        ap_config.ssid,
        ap_config.channel,
        ap_config.ip,
    );

    let _dns = crate::dns::DnsServer::start(ap_config.ip)?;

    let (tx, rx) = mpsc::channel::<StoredCredentials>();
    let networks_clone = Arc::clone(&networks_json_str);
    let last_error_json: Arc<str> = match last_error {
        None => r#"{"error":null}"#.into(),
        Some(msg) => format!(r#"{{"error":"{}"}}"#, crate::portal::json_escape_str(msg)).into(),
    };

    let mut server = EspHttpServer::new(&HttpConfig::default()).map_err(http_err)?;

    crate::dns::register_captive_portal_handlers(&mut server, ap_config.ip)
        .map_err(|e| ProvisioningError::HttpServer(e))?;

    server
        .fn_handler(
            "/",
            esp_idf_svc::http::Method::Get,
            move |req| -> Result<(), BoxError> {
                req.into_ok_response()?.write_all(index_html().as_bytes())?;
                Ok(())
            },
        )
        .map_err(http_err)?;

    let status_json = Arc::clone(&last_error_json);
    server
        .fn_handler(
            "/status",
            esp_idf_svc::http::Method::Get,
            move |req| -> Result<(), BoxError> {
                let mut resp =
                    req.into_response(200, None, &[("Content-Type", "application/json")])?;
                resp.write_all(status_json.as_bytes())?;
                Ok(())
            },
        )
        .map_err(http_err)?;

    server
        .fn_handler(
            "/networks",
            esp_idf_svc::http::Method::Get,
            move |req| -> Result<(), BoxError> {
                let mut resp =
                    req.into_response(200, None, &[("Content-Type", "application/json")])?;
                resp.write_all(networks_clone.as_bytes())?;
                Ok(())
            },
        )
        .map_err(http_err)?;

    server
        .fn_handler("/connect", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), BoxError> {
            let mut body     = Vec::new();
            let mut buf      = [0u8; 256];
            let mut oversize = false;

            loop {
                let n = req.read(&mut buf)?;
                if n == 0 { break; }
                body.extend_from_slice(&buf[..n]);
                if body.len() > MAX_BODY_SIZE {
                    oversize = true;
                    drain_request(&mut req);
                    break;
                }
            }

            if oversize {
                req.into_response(400, Some("Bad Request"), &[])?.write_all(b"")?;
                return Ok(());
            }

            let response_body = match parse_credentials(&body) {
                Ok(creds) => match tx.send(creds) {
                    Ok(())  => r#"{"success":true}"#.to_string(),
                    Err(_)  => r#"{"success":false,"message":"Portal is closing, please reconnect and try again."}"#.to_string(),
                },
                Err(msg) => format!(r#"{{"success":false,"message":"{}"}}"#, msg),
            };

            req.into_ok_response()?.write_all(response_body.as_bytes())?;
            Ok(())
        })
        .map_err(http_err)?;

    log::info!(
        "Setup portal running at http://{} | waiting for credentials…",
        ap_config.ip,
    );

    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(creds) => {
                log::info!("Credentials received for SSID '{}'", creds.ssid);
                drop(server);
                drop(_dns);

                thread::sleep(std::time::Duration::from_millis(500));

                wifi.stop().map_err(ProvisioningError::WifiDriver)?;
                return Ok(creds);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(ProvisioningError::HttpServer(
                    "handler channel closed unexpectedly".into(),
                ));
            }
        }
    }
}

/// Converts an [`ApConfig`] into the `esp-idf-svc` [`Configuration`] type
/// needed to call `wifi.set_configuration`.
///
/// Only [`ApSecurity::Open`] and [`ApSecurity::Wpa2`] are supported by the
/// esp-idf soft-AP driver; other variants are not reachable because `ApSecurity`
/// only exposes those two.
fn build_ap_config(cfg: &ApConfig) -> Result<Configuration, ProvisioningError> {
    let (auth, password) = match &cfg.security {
        ApSecurity::Open => (AuthMethod::None, Default::default()),
        ApSecurity::Wpa2(p) => (
            AuthMethod::WPA2Personal,
            p.as_str()
                .try_into()
                .map_err(|_| ProvisioningError::InvalidCredentials)?,
        ),
    };

    Ok(Configuration::AccessPoint(AccessPointConfiguration {
        ssid: cfg
            .ssid
            .as_str()
            .try_into()
            .map_err(|_| ProvisioningError::InvalidCredentials)?,
        auth_method: auth,
        password,
        channel: cfg.channel,
        ..Default::default()
    }))
}

/// Parses a `/connect` request body (`"<ssid>\n<password>"`) into a
/// [`StoredCredentials`] struct, returning a user-facing error string on any
/// validation failure.
///
/// The password field is optional: an absent or empty password is interpreted
/// as an open (unauthenticated) network.
fn parse_credentials(body: &[u8]) -> Result<StoredCredentials, String> {
    let s = std::str::from_utf8(body).map_err(|_| "Invalid UTF-8".to_string())?;
    let (ssid, password) = s.split_once('\n').unwrap_or((s, ""));
    let ssid = ssid.trim().to_string();

    let password = password.trim_matches(['\r', '\n']).to_string();

    if ssid.is_empty() {
        return Err("SSID cannot be empty".to_string());
    }
    if ssid.len() > SSID_LEN_MAX {
        return Err(format!(
            "SSID is too long ({} characters, max {})",
            ssid.len(),
            SSID_LEN_MAX
        ));
    }

    let auth_method = if password.is_empty() {
        AuthMethod::None
    } else {
        if password.len() < WPA_PASS_LEN_MIN {
            return Err(format!(
                "Password must be at least {} characters ({} provided)",
                WPA_PASS_LEN_MIN,
                password.len()
            ));
        }
        if password.len() > WPA_PASS_LEN_MAX {
            return Err(format!(
                "Password must be {} characters or fewer ({} provided)",
                WPA_PASS_LEN_MAX,
                password.len()
            ));
        }
        AuthMethod::WPA2Personal
    };

    Ok(StoredCredentials {
        ssid,
        password,
        auth_method,
    })
}

/// Reads and discards the remainder of an oversized request body.
///
/// `esp-idf-svc` requires the full request body to be consumed before a
/// response can be sent; this helper drains it when we've already decided to
/// reject the request.
fn drain_request(
    req: &mut esp_idf_svc::http::server::Request<
        &mut esp_idf_svc::http::server::EspHttpConnection<'_>,
    >,
) {
    let mut sink = [0u8; 256];
    loop {
        match req.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}
