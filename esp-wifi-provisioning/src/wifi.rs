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
}

pub fn scan_networks(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
) -> Result<Vec<ScannedNetwork>, ProvisioningError> {
    let mut networks: Vec<ScannedNetwork> = wifi
        .wifi_mut()
        .scan()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?
        .into_iter()
        .filter(|ap| !ap.ssid.is_empty())
        .map(|ap| ScannedNetwork {
            ssid: ap.ssid.as_str().to_string(),
            rssi: ap.signal_strength,
        })
        .collect();

    networks.sort_unstable_by(|a, b| a.ssid.cmp(&b.ssid).then(b.rssi.cmp(&a.rssi)));
    networks.dedup_by(|a, b| a.ssid == b.ssid);
    networks.sort_unstable_by(|a, b| b.rssi.cmp(&a.rssi));

    Ok(networks)
}

pub fn connect_with_retry(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
    creds: &StoredCredentials,
    config: &RetryConfig,
) -> Result<(), ProvisioningError> {
    let auth = if creds.password.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };

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
        auth_method: auth,
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

        match try_connect(wifi, config.connect_timeout) {
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
    })
}

fn try_connect(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
    timeout: Duration,
) -> Result<(), ProvisioningError> {
    let _ = wifi.disconnect();
    wifi.connect()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        if wifi
            .is_up()
            .map_err(|e| ProvisioningError::WifiDriver(e.into()))?
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            let _ = wifi.disconnect();
            return Err(ProvisioningError::ConnectionTimeout);
        }
        thread::sleep(Duration::from_millis(200));
    }
}
