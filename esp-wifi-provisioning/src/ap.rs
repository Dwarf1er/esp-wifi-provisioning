use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
use esp_idf_svc::io::Write;
use esp_idf_svc::ipv4;
use esp_idf_svc::netif::{EspNetif, NetifConfiguration};
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};

use crate::error::{BoxError, ProvisioningError};
use crate::nvs::MAX_PASS_LEN;
use crate::nvs::MAX_SSID_LEN;
use crate::nvs::StoredCredentials;
use crate::portal::{index_html, networks_json};

const MAX_BODY_SIZE: usize = MAX_SSID_LEN + MAX_PASS_LEN + 2;

#[derive(Debug, Clone)]
pub struct ApConfig {
    pub ssid: String,
    pub password: Option<String>,
    pub channel: u8,
    pub ip: Ipv4Addr,
    /// When true, starts a DNS server that resolves all domains to the AP IP
    /// and registers OS-specific captive portal detection endpoints, triggering
    /// the "Sign in to network" popup on connecting devices.
    /// Only available when compiled with the `captive-portal` feature.
    #[cfg(feature = "captive-portal")]
    pub captive_portal: bool,
}

impl Default for ApConfig {
    fn default() -> Self {
        Self {
            ssid: "ESP32-Setup".into(),
            password: None,
            channel: 6,
            ip: Ipv4Addr::new(192, 168, 4, 1),
            #[cfg(feature = "captive-portal")]
            captive_portal: true,
        }
    }
}

pub fn run_portal(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
    ap_config: &ApConfig,
) -> Result<StoredCredentials, ProvisioningError> {
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;
    wifi.start()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;

    let networks = crate::wifi::scan_networks(wifi).unwrap_or_else(|e| {
        log::warn!("Scan failed ({e}), network list will be empty");
        vec![]
    });
    log::info!("Scan found {} networks", networks.len());

    wifi.stop().map_err(|e| {
        let _ = wifi.stop();
        ProvisioningError::WifiDriver(e.into())
    })?;

    let networks_json_str: Arc<str> = networks_json(&networks).into();

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
        key: "WIFI_AP_PROV".try_into().unwrap(),
        ..NetifConfiguration::wifi_default_router()
    })
    .map_err(|e| ProvisioningError::ApStart(e.into()))?;

    wifi.wifi_mut()
        .swap_netif_ap(ap_netif)
        .map_err(|e| ProvisioningError::ApStart(e.into()))?;

    wifi.set_configuration(&build_ap_config(ap_config)?)
        .map_err(|e| ProvisioningError::ApStart(e.into()))?;
    wifi.start()
        .map_err(|e| ProvisioningError::ApStart(e.into()))?;

    log::info!(
        "Soft-AP '{}' started on channel {} | connect and visit http://{}",
        ap_config.ssid,
        ap_config.channel,
        ap_config.ip,
    );

    #[cfg(feature = "captive-portal")]
    let _dns = if ap_config.captive_portal {
        Some(crate::dns::DnsServer::start(ap_config.ip)?)
    } else {
        None
    };

    let (tx, rx) = mpsc::channel::<StoredCredentials>();
    let networks_clone = Arc::clone(&networks_json_str);

    let mut server = EspHttpServer::new(&HttpConfig::default())
        .map_err(|e| ProvisioningError::HttpServer(e.into()))?;

    #[cfg(feature = "captive-portal")]
    if ap_config.captive_portal {
        crate::dns::register_captive_portal_handlers(&mut server, ap_config.ip)
            .map_err(|e| ProvisioningError::HttpServer(e.into()))?;
    }

    server
        .fn_handler(
            "/",
            esp_idf_svc::http::Method::Get,
            move |req| -> Result<(), BoxError> {
                req.into_ok_response()?.write_all(index_html().as_bytes())?;
                Ok(())
            },
        )
        .map_err(|e| ProvisioningError::HttpServer(e.into()))?;

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
        .map_err(|e| ProvisioningError::HttpServer(e.into()))?;

    server
        .fn_handler(
            "/connect",
            esp_idf_svc::http::Method::Post,
            move |mut req| -> Result<(), BoxError> {
                let mut body = Vec::new();
                let mut buf = [0u8; 256];
                loop {
                    let n = req.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&buf[..n]);
                    if body.len() > MAX_BODY_SIZE {
                        req.into_response(400, Some("Bad Request"), &[])?
                            .write_all(b"")?;
                        return Ok(());
                    }
                }

                let response_body = match parse_credentials(&body) {
                    Ok(creds) => {
                        tx.send(creds).ok();
                        r#"{"success":true}"#.to_string()
                    }
                    Err(msg) => format!(r#"{{"success":false,"message":"{}"}}"#, msg),
                };

                req.into_ok_response()?
                    .write_all(response_body.as_bytes())?;
                Ok(())
            },
        )
        .map_err(|e| ProvisioningError::HttpServer(e.into()))?;

    log::info!(
        "Setup portal running at http://{} | waiting for credentials…",
        ap_config.ip,
    );

    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(creds) => {
                log::info!("Credentials received for SSID '{}'", creds.ssid);
                drop(server);
                thread::sleep(std::time::Duration::from_millis(500));
                wifi.stop()
                    .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;
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
    let (auth, password) = match &cfg.password {
        Some(p) if !p.is_empty() => (
            AuthMethod::WPA2Personal,
            p.as_str()
                .try_into()
                .map_err(|_| ProvisioningError::InvalidCredentials)?,
        ),
        _ => (AuthMethod::None, Default::default()),
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

fn parse_credentials(body: &[u8]) -> Result<StoredCredentials, &'static str> {
    let s = std::str::from_utf8(body).map_err(|_| "Invalid UTF-8")?;
    let (ssid, password) = s.split_once('\n').unwrap_or((s, ""));
    let ssid = ssid.trim().to_string();
    let password = password.trim_end_matches(['\r', '\n']).to_string();
    if ssid.is_empty() {
        return Err("SSID cannot be empty");
    }
    let auth_method = if password.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };
    Ok(StoredCredentials {
        ssid,
        password,
        auth_method,
    })
}
