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
    pub fn new(wifi: BlockingWifi<EspWifi<'d>>, nvs: EspNvsPartition<NvsDefault>) -> Self {
        Self {
            wifi,
            nvs,
            ap_config: ApConfig::default(),
            retry_config: RetryConfig::default(),
        }
    }

    pub fn ap_ssid(mut self, ssid: impl Into<String>) -> Self {
        self.ap_config.ssid = ssid.into();
        self
    }

    /// Secure the setup AP with a WPA2 password. If not called, the AP is
    /// open. See [`ApSecurity::Open`] for the security implications.
    pub fn ap_password(mut self, password: impl Into<String>) -> Self {
        self.ap_config.security = ApSecurity::Wpa2(password.into());
        self
    }

    /// Explicitly open the setup AP with no password. This is the default
    /// behaviour, call this only when you want to be explicit about it.
    pub fn ap_open(mut self) -> Self {
        self.ap_config.security = ApSecurity::Open;
        self
    }

    pub fn ap_config(mut self, cfg: ApConfig) -> Self {
        self.ap_config = cfg;
        self
    }

    pub fn retry_config(mut self, cfg: RetryConfig) -> Self {
        self.retry_config = cfg;
        self
    }

    pub fn max_retries(mut self, n: u8) -> Self {
        self.retry_config.max_attempts = n;
        self
    }

    pub fn clear_credentials(&self) -> Result<(), ProvisioningError> {
        nvs::clear_credentials(self.nvs.clone())
    }

    /// Run the provisioning flow, returning the connected `BlockingWifi` on
    /// success.
    ///
    /// # Errors
    ///
    /// Returns a [`ProvisioningError`] if the WiFi driver, NVS, or HTTP server
    /// encounters an unrecoverable error. Connection failures (wrong password,
    /// timeout) are retried and then cause the portal to reopen, they do not
    /// bubble up as errors from this function.
    pub fn provision(mut self) -> Result<BlockingWifi<EspWifi<'d>>, ProvisioningError> {
        self.retry_config
            .validate()
            .map_err(|msg| ProvisioningError::HttpServer(msg.into()))?;

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
                // NVS is unreadable or corrupt. Log the full error and
                // surface it in the portal's "last error" banner so the
                // user knows why the portal opened unexpectedly.
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

    fn try_connect_sta(&mut self, creds: &nvs::StoredCredentials) -> Result<(), ProvisioningError> {
        wifi::connect_with_retry(&mut self.wifi, creds, &self.retry_config)
    }
}
