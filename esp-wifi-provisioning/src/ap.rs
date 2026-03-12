use std::sync::{Arc, Mutex};
use std::thread;

use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
use esp_idf_svc::io::Write;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};

use crate::error::{BoxError, ProvisioningError};
use crate::nvs::StoredCredentials;
use crate::portal::{index_html, networks_json};

const AP_IP: &str = "192.168.71.1";

#[derive(Debug, Clone)]
pub struct ApConfig {
    pub ssid: String,
    pub password: Option<String>,
    pub channel: u8,
}

impl Default for ApConfig {
    fn default() -> Self {
        Self {
            ssid: "ESP32-Setup".into(),
            password: None,
            channel: 6,
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

    wifi.stop()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;

    let networks_json_str = Arc::new(networks_json(&networks));

    wifi.set_configuration(&build_ap_config(ap_config)?)
        .map_err(|e| ProvisioningError::ApStart(e.into()))?;
    wifi.start()
        .map_err(|e| ProvisioningError::ApStart(e.into()))?;

    log::info!(
        "Soft-AP '{}' started on channel {} | connect and visit http://{}",
        ap_config.ssid,
        ap_config.channel,
        AP_IP
    );

    let submitted: Arc<Mutex<Option<StoredCredentials>>> = Arc::new(Mutex::new(None));
    let submitted_clone = Arc::clone(&submitted);
    let networks_clone = Arc::clone(&networks_json_str);

    let mut server = EspHttpServer::new(&HttpConfig::default())
        .map_err(|e| ProvisioningError::HttpServer(e.into()))?;

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
                }

                let response_body = match parse_credentials(&body) {
                    Ok(creds) => {
                        *submitted_clone.lock().unwrap() = Some(creds);
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
        AP_IP
    );

    loop {
        thread::sleep(std::time::Duration::from_millis(250));
        if let Some(creds) = submitted.lock().unwrap().take() {
            log::info!("Credentials received for SSID '{}'", creds.ssid);
            drop(server);
            thread::sleep(std::time::Duration::from_millis(500));
            wifi.stop()
                .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;
            return Ok(creds);
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
    if ssid.is_empty() {
        return Err("SSID cannot be empty");
    }
    Ok(StoredCredentials {
        ssid,
        password: password.to_string(),
    })
}
