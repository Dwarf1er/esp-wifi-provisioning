use crate::error::ProvisioningError;
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};

const NVS_NAMESPACE: &str = "wifi_prov";
const KEY_SSID: &str = "ssid";
const KEY_PASSWORD: &str = "password";
const MAX_SSID_LEN: usize = 32;
const MAX_PASS_LEN: usize = 64;

#[derive(Debug, Clone)]
pub struct StoredCredentials {
    pub ssid: String,
    pub password: String,
}

fn open_nvs(
    partition: EspNvsPartition<NvsDefault>,
    read_write: bool,
) -> Result<EspNvs<NvsDefault>, ProvisioningError> {
    EspNvs::new(partition, NVS_NAMESPACE, read_write)
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))
}

pub fn load_credentials(
    partition: EspNvsPartition<NvsDefault>,
) -> Result<Option<StoredCredentials>, ProvisioningError> {
    let nvs = open_nvs(partition, false)?;

    let mut ssid_buf = [0u8; MAX_SSID_LEN + 1];
    let mut pass_buf = [0u8; MAX_PASS_LEN + 1];

    let ssid_opt = nvs
        .get_str(KEY_SSID, &mut ssid_buf)
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;
    let pass_opt = nvs
        .get_str(KEY_PASSWORD, &mut pass_buf)
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;

    match ssid_opt {
        None => Ok(None),
        Some(ssid) => Ok(Some(StoredCredentials {
            ssid: ssid.to_string(),
            password: pass_opt.unwrap_or("").to_string(),
        })),
    }
}

pub fn save_credentials(
    partition: EspNvsPartition<NvsDefault>,
    creds: &StoredCredentials,
) -> Result<(), ProvisioningError> {
    if creds.ssid.is_empty() || creds.ssid.len() > MAX_SSID_LEN {
        return Err(ProvisioningError::InvalidCredentials);
    }
    if creds.password.len() > MAX_PASS_LEN {
        return Err(ProvisioningError::InvalidCredentials);
    }

    let mut nvs = open_nvs(partition, true)?;

    nvs.set_str(KEY_SSID, &creds.ssid)
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;

    if creds.password.is_empty() {
        let _ = nvs.remove(KEY_PASSWORD);
    } else {
        nvs.set_str(KEY_PASSWORD, &creds.password)
            .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;
    }

    Ok(())
}

pub fn clear_credentials(partition: EspNvsPartition<NvsDefault>) -> Result<(), ProvisioningError> {
    let mut nvs = open_nvs(partition, true)?;
    let _ = nvs.remove(KEY_SSID);
    let _ = nvs.remove(KEY_PASSWORD);
    Ok(())
}
