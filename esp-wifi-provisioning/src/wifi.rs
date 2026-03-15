use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

use crate::error::ProvisioningError;
use crate::nvs::StoredCredentials;

#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: u8,
    pub connect_timeout: Duration,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            connect_timeout: Duration::from_secs(30),
            initial_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScannedNetwork {
    pub ssid: String,
    pub rssi: i8,
    pub auth_method: AuthMethod,
}

pub fn scan_networks(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
) -> Result<Vec<ScannedNetwork>, ProvisioningError> {
    let mut best: HashMap<String, ScannedNetwork> = HashMap::new();

    for ap in wifi
        .wifi_mut()
        .scan()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?
    {
        if ap.ssid.is_empty() {
            continue;
        }

        let ssid = ap.ssid.as_str().to_string();
        let rssi = ap.signal_strength;
        let auth = ap.auth_method.unwrap_or(AuthMethod::None);

        match best.get_mut(&ssid) {
            Some(existing) => {
                if rssi > existing.rssi {
                    existing.rssi = rssi;
                    existing.auth_method = auth;
                }
            }
            None => {
                best.insert(
                    ssid.clone(),
                    ScannedNetwork {
                        ssid,
                        rssi,
                        auth_method: auth,
                    },
                );
            }
        }
    }

    let mut networks: Vec<ScannedNetwork> = best.into_values().collect();

    networks.sort_unstable_by(|a, b| b.rssi.cmp(&a.rssi));

    Ok(networks)
}

pub fn connect_with_retry(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
    creds: &StoredCredentials,
    config: &RetryConfig,
) -> Result<(), ProvisioningError> {
    let sta_config = Configuration::Client(ClientConfiguration {
        ssid: creds
            .ssid
            .as_str()
            .try_into()
            .map_err(|_| ProvisioningError::InvalidCredentials)?,
        password: creds
            .password
            .as_str()
            .try_into()
            .map_err(|_| ProvisioningError::InvalidCredentials)?,
        auth_method: creds.auth_method,
        ..Default::default()
    });

    wifi.set_configuration(&sta_config)
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;

    if !wifi
        .is_started()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?
    {
        wifi.start()
            .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;
    }

    let mut backoff = config.initial_backoff;

    for attempt in 1..=config.max_attempts {
        log::info!("WiFi connect attempt {}/{}", attempt, config.max_attempts);

        match try_connect(wifi) {
            Ok(()) => {
                log::info!("WiFi connected on attempt {}", attempt);
                return Ok(());
            }
            Err(e) => {
                log::warn!("Attempt {} failed: {}", attempt, e);
                if attempt < config.max_attempts {
                    log::info!("Backing off for {}ms", backoff.as_millis());
                    thread::sleep(backoff);
                    backoff = (backoff * 2).min(config.max_backoff);
                }
            }
        }
    }

    Err(ProvisioningError::ConnectionFailed {
        attempts: config.max_attempts,
        cause: crate::error::ConnectionFailureCause::DriverError(
            "exhausted all connection attempts".into(),
        ),
    })
}

fn try_connect(wifi: &mut BlockingWifi<EspWifi<'_>>) -> Result<(), ProvisioningError> {
    let _ = wifi.disconnect();
    wifi.connect()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;
    wifi.wait_netif_up()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;
    Ok(())
}
