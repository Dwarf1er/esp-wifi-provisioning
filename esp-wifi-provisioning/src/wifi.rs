//! Station-mode WiFi connection with exponential-backoff retry.
//!
//! The public surface is [`RetryConfig`], which controls how hard the device
//! tries to connect before giving up.  The actual connection logic lives in
//! [`connect_with_retry`] and [`try_connect`], both of which are crate-private.

use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{ConnectionFailureCause, ProvisioningError};
use crate::nvs::StoredCredentials;

/// Controls how the device retries a WiFi station connection.
///
/// The retry schedule uses truncated exponential backoff: each failed attempt
/// doubles the wait time up to [`max_backoff`](Self::max_backoff).
///
/// # Example
///
/// ```rust,no_run
/// use std::time::Duration;
/// use esp_wifi_provisioning::RetryConfig;
///
/// let cfg = RetryConfig::default()
///     .max_attempts(3)
///     .connect_timeout(Duration::from_secs(15))
///     .initial_backoff(Duration::from_secs(2))
///     .max_backoff(Duration::from_secs(30));
/// ```
#[derive(Debug, Clone)]
pub struct RetryConfig {
    max_attempts: u8,
    connect_timeout: Duration,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl RetryConfig {
    /// Sets the maximum number of connection attempts before returning
    /// [`ProvisioningError::ConnectionFailed`].
    ///
    /// Must be at least `1`; validated by
    /// [`Provisioner::provision`](crate::Provisioner::provision).
    pub fn max_attempts(mut self, n: u8) -> Self {
        self.max_attempts = n;
        self
    }

    /// Sets how long to wait for the device to associate on each individual
    /// attempt before declaring it a timeout.
    ///
    /// Must be non-zero; validated by
    /// [`Provisioner::provision`](crate::Provisioner::provision).
    pub fn connect_timeout(mut self, d: Duration) -> Self {
        self.connect_timeout = d;
        self
    }

    /// Sets the wait time before the *second* attempt (first retry).
    /// Subsequent waits double up to [`max_backoff`](Self::max_backoff).
    ///
    /// Must be non-zero; validated by
    /// [`Provisioner::provision`](crate::Provisioner::provision).
    pub fn initial_backoff(mut self, d: Duration) -> Self {
        self.initial_backoff = d;
        self
    }

    /// Caps the backoff so it never exceeds this duration regardless of how
    /// many retries have occurred.
    pub fn max_backoff(mut self, d: Duration) -> Self {
        self.max_backoff = d;
        self
    }

    /// Checks that all fields are within acceptable ranges.
    ///
    /// Called by [`Provisioner::provision`](crate::Provisioner::provision)
    /// before any I/O is attempted, so misconfiguration surfaces immediately
    /// rather than at the first connection attempt.
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
    /// Returns a conservative default suitable for most home/office networks:
    /// 5 attempts, 30 s per-attempt timeout, 5 s initial backoff, 120 s cap.
    fn default() -> Self {
        Self {
            max_attempts: 5,
            connect_timeout: Duration::from_secs(30),
            initial_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(120),
        }
    }
}

/// A single network returned by a WiFi scan.
///
/// Duplicate SSIDs (same name, multiple BSSIDs) are deduplicated by
/// [`scan_networks`], keeping only the entry with the strongest signal.
#[derive(Debug, Clone)]
pub(crate) struct ScannedNetwork {
    pub ssid: String,
    /// Received signal strength in dBm. Higher (less negative) is better.
    pub rssi: i8,
    pub auth_method: AuthMethod,
}

/// Scans for visible networks and returns them sorted by signal strength
/// (strongest first), with duplicate SSIDs collapsed to their strongest entry.
///
/// A scan failure is non-fatal at the call site: [`run_portal`](crate::ap::run_portal)
/// logs a warning and continues with an empty list rather than aborting.
///
/// # Errors
///
/// Returns [`ProvisioningError::WifiDriver`] if the underlying `scan()` call
/// fails.
pub(crate) fn scan_networks(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
) -> Result<Vec<ScannedNetwork>, ProvisioningError> {
    let mut networks: Vec<ScannedNetwork> = Vec::new();

    for ap in wifi
        .wifi_mut()
        .scan()
        .map_err(ProvisioningError::WifiDriver)?
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

/// Configures the WiFi driver for station mode and attempts to connect using
/// the supplied credentials, retrying on failure according to `config`.
///
/// On a driver error (as opposed to a timeout), the WiFi stack is cycled
/// (stop + start) before the next attempt, which clears any internal state
/// that may have caused the failure.
///
/// # Errors
///
/// Returns [`ProvisioningError::ConnectionFailed`] after all attempts are
/// exhausted, carrying the attempt count and the cause of the last failure.
/// Returns [`ProvisioningError::WifiDriver`] if the driver itself fails in a
/// way that prevents retrying (e.g. `set_configuration` or `start` fails).
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
        .map_err(ProvisioningError::WifiDriver)?;

    if !wifi.is_started().map_err(ProvisioningError::WifiDriver)? {
        wifi.start().map_err(ProvisioningError::WifiDriver)?;
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
                if matches!(cause, ConnectionFailureCause::DriverError(_)) {
                    log::info!("Driver error | cycling WiFi stack before retry");
                    let _ = wifi.stop();
                    wifi.start().map_err(ProvisioningError::WifiDriver)?;
                }

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

/// Makes a single connection attempt and polls until associated or timed out.
///
/// Any previous association is disconnected first to ensure a clean start.
///
/// # Errors
///
/// Returns [`ConnectionFailureCause::DriverError`] if `connect()` itself fails,
/// or [`ConnectionFailureCause::Timeout`] if the device does not associate
/// within `timeout`.
fn try_connect(
    wifi: &mut BlockingWifi<EspWifi<'_>>,
    timeout: Duration,
) -> Result<(), ConnectionFailureCause> {
    let _ = wifi.disconnect();

    wifi.connect()
        .map_err(ConnectionFailureCause::DriverError)?;

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
