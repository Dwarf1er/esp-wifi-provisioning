pub mod ap;
pub mod error;
pub mod nvs;
pub mod portal;
pub mod wifi;

use esp_idf_svc::nvs::{EspNvsPartition, NvsDefault};
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};

pub use ap::ApConfig;
pub use error::ProvisioningError;
pub use nvs::StoredCredentials;
pub use wifi::RetryConfig;

pub struct Provisioner<'d> {
    wifi: BlockingWifi<EspWifi<'d>>,
    nvs: EspNvsPartition<NvsDefault>,
    ap_config: ApConfig,
    retry_config: RetryConfig,
    force_ap: bool,
}

impl<'d> Provisioner<'d> {
    pub fn new(wifi: BlockingWifi<EspWifi<'d>>, nvs: EspNvsPartition<NvsDefault>) -> Self {
        Self {
            wifi,
            nvs,
            ap_config: ApConfig::default(),
            retry_config: RetryConfig::default(),
            force_ap: false,
        }
    }

    pub fn ap_ssid(mut self, ssid: impl Into<String>) -> Self {
        self.ap_config.ssid = ssid.into();
        self
    }

    pub fn ap_password(mut self, password: impl Into<String>) -> Self {
        self.ap_config.password = Some(password.into());
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

    pub fn force_ap_mode(mut self, force: bool) -> Self {
        self.force_ap = force;
        self
    }

    pub fn provision(mut self) -> Result<(), ProvisioningError> {
        if !self.force_ap {
            match nvs::load_credentials(self.nvs.clone()) {
                Ok(Some(creds)) => {
                    log::info!(
                        "Found stored credentials for '{}', attempting connection",
                        creds.ssid
                    );
                    match self.try_connect_sta(&creds) {
                        Ok(()) => return Ok(()),
                        Err(e) => log::warn!("Stored credentials failed: {e}"),
                    }
                }
                Ok(None) => {
                    log::info!("No credentials stored, starting captive portal");
                }
                Err(e) => {
                    log::warn!("NVS error ({e}), falling back to captive portal");
                }
            }
        } else {
            log::info!("force_ap_mode set — skipping NVS lookup");
        }

        loop {
            let creds = ap::run_portal(&mut self.wifi, &self.ap_config)?;

            if creds.ssid.is_empty() {
                log::warn!("Empty SSID submitted, re-opening portal");
                continue;
            }

            match self.try_connect_sta(&creds) {
                Ok(()) => {
                    if let Err(e) = nvs::save_credentials(self.nvs.clone(), &creds) {
                        log::warn!("Could not save credentials to NVS: {e}");
                    }
                    return Ok(());
                }
                Err(e) => {
                    log::warn!("New credentials failed: {e} — re-opening portal");
                }
            }
        }
    }

    fn try_connect_sta(&mut self, creds: &StoredCredentials) -> Result<(), ProvisioningError> {
        wifi::connect_with_retry(&mut self.wifi, creds, &self.retry_config)
    }

    pub fn into_wifi(self) -> BlockingWifi<EspWifi<'d>> {
        self.wifi
    }
}
