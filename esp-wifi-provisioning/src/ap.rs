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

const MAX_BODY_SIZE: usize = SSID_LEN_MAX + WPA_PASS_LEN_MAX + 2;
static NETIF_KEY_CTR: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Debug)]
pub enum ApSecurity {
    Open,
    Wpa2(String),
}

#[derive(Clone, Debug)]
pub struct ApConfig {
    pub ssid: String,
    pub security: ApSecurity,
    pub channel: u8,
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

fn http_err(e: impl std::error::Error + Send + Sync + 'static) -> ProvisioningError {
    ProvisioningError::HttpServer(Box::new(e))
}

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
