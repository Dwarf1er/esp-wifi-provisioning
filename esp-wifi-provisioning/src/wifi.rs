use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{ConnectionFailureCause, ProvisioningError};
use crate::nvs::StoredCredentials;

#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: u8,
    pub connect_timeout: Duration,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl RetryConfig {
    /// Validates the config, returning an error message if any field is
    /// out of range. Called by [`Provisioner::provision`] before the first
    /// connection attempt so misconfiguration is caught early.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.max_attempts == 0 {
            return Err("max_attempts must be at least 1");
        }
        if self.connect_timeout.is_zero() {
            return Err("connect_timeout must be greater than zero");
        }
        if self.initial_backoff.is_zero() {
            return Err("initial_backoff must be greater than zero");
        }
        Ok(())
    }
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
pub(crate) struct ScannedNetwork {
    pub ssid: String,
    pub rssi: i8,
    pub auth_method: AuthMethod,
}

pub(crate) fn scan_networks(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
) -> Result<Vec<ScannedNetwork>, ProvisioningError> {
    let mut networks: Vec<ScannedNetwork> = Vec::new();

    for ap in wifi
        .wifi_mut()
        .scan()
        .map_err(|e| ProvisioningError::WifiDriver(e.into()))?
    {
        if ap.ssid.is_empty() {
            continue;
        }

        let ssid = ap.ssid.as_str();
        let rssi = ap.signal_strength;
        let auth = ap.auth_method.unwrap_or(AuthMethod::None);

        if let Some(existing) = networks.iter_mut().find(|n| n.ssid == ssid) {
            if rssi > existing.rssi {
                existing.rssi = rssi;
                existing.auth_method = auth;
            }
        } else {
            networks.push(ScannedNetwork {
                ssid: ssid.to_string(),
                rssi,
                auth_method: auth,
            });
        }
    }

    networks.sort_unstable_by(|a, b| b.rssi.cmp(&a.rssi));
    Ok(networks)
}

pub(crate) fn connect_with_retry(
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

        match try_connect(wifi, config.connect_timeout) {
            Ok(()) => {
                log::info!("WiFi connected");
                return Ok(());
            }
            Err(cause) => {
                log::warn!("Connection failed: {:?}", cause);

                if attempt == config.max_attempts {
                    return Err(ProvisioningError::ConnectionFailed {
                        attempts: attempt,
                        cause,
                    });
                }

                log::info!("Retrying in {} ms", backoff.as_millis());
                thread::sleep(backoff);
                backoff = (backoff * 2).min(config.max_backoff);
            }
        }
    }

    unreachable!()
}

fn try_connect(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
    timeout: Duration,
) -> Result<(), ConnectionFailureCause> {
    let _ = wifi.disconnect();

    wifi.connect()
        .map_err(|e| ConnectionFailureCause::DriverError(e.into()))?;

    let deadline = Instant::now() + timeout;

    loop {
        if wifi.is_connected().unwrap_or(false) {
            return Ok(());
        }

        if Instant::now() >= deadline {
            let _ = wifi.disconnect();
            return Err(ConnectionFailureCause::Timeout);
        }

        thread::sleep(Duration::from_millis(100));
    }
}
