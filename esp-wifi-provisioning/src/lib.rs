//! WiFi provisioning via a captive-portal soft-AP for ESP32 targets.
//!
//! This crate lets an ESP32 serve a small web UI over a temporary access point.
//! Users connect to that AP, are redirected to the portal, pick their home
//! network, enter a password, and the device stores the credentials in NVS and
//! connects — all without hardcoding anything at build time.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use esp_wifi_provisioning::Provisioner;
//!
//! let wifi = /* initialise BlockingWifi<EspWifi> as usual */;
//! let nvs  = /* open the default NVS partition */;
//!
//! let wifi = Provisioner::new(wifi, nvs)
//!     .ap_ssid("MyDevice-Setup")
//!     .provision()
//!     .expect("provisioning failed");
//!
//! // `wifi` is now connected to the network the user chose.
//! ```
//!
//! # Flow
//!
//! 1. [`Provisioner::provision`] checks NVS for stored credentials and tries
//!    them first.  If that succeeds, the portal is never shown.
//! 2. If no credentials exist (or they fail), a soft-AP is started and the
//!    captive portal is served at the IP configured in [`ApConfig`].
//! 3. Once the user submits valid credentials, the AP is torn down, the device
//!    connects in station mode, and the credentials are persisted to NVS.
//! 4. On the next boot the stored credentials are tried directly (step 1),
//!    so the portal only appears when necessary.
//!
//! # Crate layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`ap`] | Soft-AP lifecycle and HTTP portal server |
//! | `dns`  | Minimal DNS responder that triggers OS captive-portal detection |
//! | [`error`] | All error types |
//! | `nvs`  | NVS credential persistence |
//! | `portal` | HTML/CSS/JS asset bundling and JSON serialisation |
//! | `wifi` | Station-mode connection with exponential-backoff retry |

pub(crate) mod ap;
pub(crate) mod dns;
pub mod error;
pub(crate) mod nvs;
pub(crate) mod portal;
pub(crate) mod wifi;

use esp_idf_svc::nvs::{EspNvsPartition, NvsDefault};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};

pub use ap::{ApConfig, ApSecurity};
pub use error::ProvisioningError;
pub use wifi::RetryConfig;

/// Drives the full provisioning lifecycle: NVS credential lookup, captive-portal
/// soft-AP, station connection, and credential persistence.
///
/// Construct with [`Provisioner::new`] and customise with the builder methods
/// before calling [`Provisioner::provision`].
///
/// # Example
///
/// ```rust,no_run
/// # use esp_wifi_provisioning::Provisioner;
/// let wifi = Provisioner::new(wifi, nvs)
///     .ap_ssid("Sensor-Setup")
///     .ap_password("setup1234") // password-protect the setup AP itself
///     .max_retries(3)
///     .provision()?;
/// # Ok::<(), esp_wifi_provisioning::ProvisioningError>(())
/// ```
pub struct Provisioner<'d> {
    wifi: BlockingWifi<EspWifi<'d>>,
    nvs: EspNvsPartition<NvsDefault>,
    ap_config: ApConfig,
    retry_config: RetryConfig,
}

impl<'d> std::fmt::Debug for Provisioner<'d> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provisioner")
            .field("ap_config", &self.ap_config)
            .field("retry_config", &self.retry_config)
            .finish_non_exhaustive()
    }
}

impl<'d> Provisioner<'d> {
    /// Creates a new `Provisioner` with default [`ApConfig`] and [`RetryConfig`].
    ///
    /// The default AP SSID is `"ESP32-Setup"` (open, channel 6, IP
    /// `192.168.4.1`).  Override with the builder methods below.
    pub fn new(wifi: BlockingWifi<EspWifi<'d>>, nvs: EspNvsPartition<NvsDefault>) -> Self {
        Self {
            wifi,
            nvs,
            ap_config: ApConfig::default(),
            retry_config: RetryConfig::default(),
        }
    }

    /// Sets the SSID broadcast by the provisioning soft-AP.
    pub fn ap_ssid(mut self, ssid: impl Into<String>) -> Self {
        self.ap_config.ssid = ssid.into();
        self
    }

    /// Password-protects the provisioning soft-AP with WPA2.
    ///
    /// By default the AP is open so users can connect without prior knowledge
    /// of a password. Call this if you want to restrict who can reach the
    /// setup portal.
    pub fn ap_password(mut self, password: impl Into<String>) -> Self {
        self.ap_config.security = ApSecurity::Wpa2(password.into());
        self
    }

    /// Removes any password from the provisioning soft-AP (the default).
    pub fn ap_open(mut self) -> Self {
        self.ap_config.security = ApSecurity::Open;
        self
    }

    /// Replaces the entire [`ApConfig`] at once.
    ///
    /// Useful when you need to set the channel or IP in addition to the SSID
    /// and security.
    pub fn ap_config(mut self, cfg: ApConfig) -> Self {
        self.ap_config = cfg;
        self
    }

    /// Replaces the entire [`RetryConfig`] at once.
    pub fn retry_config(mut self, cfg: RetryConfig) -> Self {
        self.retry_config = cfg;
        self
    }

    /// Convenience shorthand for setting only the maximum connection attempt
    /// count. Equivalent to `.retry_config(RetryConfig::default().max_attempts(n))`.
    pub fn max_retries(mut self, n: u8) -> Self {
        self.retry_config = self.retry_config.max_attempts(n);
        self
    }

    /// Erases any credentials stored in NVS, forcing the portal to appear on
    /// the next call to [`provision`](Self::provision).
    ///
    /// This is a convenience helper for factory-reset or re-provisioning flows.
    pub fn clear_credentials(&self) -> Result<(), ProvisioningError> {
        nvs::clear_credentials(self.nvs.clone())
    }

    /// Runs provisioning to completion and returns the connected `wifi` driver.
    ///
    /// # Behaviour
    ///
    /// 1. Validates [`RetryConfig`]; returns [`ProvisioningError::InvalidConfig`]
    ///    immediately if it is misconfigured.
    /// 2. Loads any credentials stored in NVS. If found, attempts connection.
    ///    On success, returns without ever opening the portal.
    /// 3. If no stored credentials exist, or the stored ones fail, opens the
    ///    captive-portal soft-AP and blocks until the user submits credentials.
    /// 4. Tries to connect with the submitted credentials. On success, saves
    ///    them to NVS and returns. On failure, re-opens the portal with the
    ///    error surfaced to the user.
    ///
    /// # Errors
    ///
    /// Returns the first unrecoverable [`ProvisioningError`] encountered (WiFi
    /// driver failures, NVS access errors, HTTP server errors, etc.).
    /// Connection failures from user-submitted credentials are *not* returned
    /// as errors, they cause the portal to re-open instead.
    pub fn provision(mut self) -> Result<BlockingWifi<EspWifi<'d>>, ProvisioningError> {
        self.retry_config
            .validate()
            .map_err(ProvisioningError::InvalidConfig)?;

        let mut last_error: Option<String> = None;

        match nvs::load_credentials(self.nvs.clone()) {
            Ok(Some(creds)) => {
                log::info!(
                    "Found stored credentials for '{}', attempting connection",
                    creds.ssid
                );
                match self.try_connect_sta(&creds) {
                    Ok(()) => return Ok(self.wifi),
                    Err(e) => {
                        log::warn!("Stored credentials failed: {e}");
                        last_error = Some(e.to_string());
                    }
                }
            }
            Ok(None) => {
                log::info!("No credentials stored, starting captive portal");
            }
            Err(e) => {
                log::warn!("NVS error ({e}), falling back to captive portal");
                last_error = Some(e.to_string());
            }
        }

        loop {
            let creds = ap::run_portal(&mut self.wifi, &self.ap_config, last_error.as_deref())?;

            if creds.ssid.is_empty() {
                log::warn!("Empty SSID submitted, re-opening portal");
                last_error = Some("SSID cannot be empty.".into());
                continue;
            }

            match self.try_connect_sta(&creds) {
                Ok(()) => {
                    if let Err(e) = nvs::save_credentials(self.nvs.clone(), &creds) {
                        log::warn!("Could not save credentials to NVS: {e}");
                    }
                    return Ok(self.wifi);
                }
                Err(e) => {
                    log::warn!("New credentials failed: {e} | re-opening portal");
                    last_error = Some(e.to_string());
                }
            }
        }
    }

    /// Attempts a station-mode connection using the supplied credentials,
    /// delegating retry logic to [`wifi::connect_with_retry`].
    fn try_connect_sta(&mut self, creds: &nvs::StoredCredentials) -> Result<(), ProvisioningError> {
        wifi::connect_with_retry(&mut self.wifi, creds, &self.retry_config)
    }
}
