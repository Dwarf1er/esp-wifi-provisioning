use crate::error::ProvisioningError;
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use esp_idf_svc::wifi::AuthMethod;

const NVS_NAMESPACE: &str = "wifi_prov";
const KEY_SSID: &str = "ssid";
const KEY_PASSWORD: &str = "password";
const KEY_AUTH_METHOD: &str = "auth_method";
pub const MAX_SSID_LEN: usize = 32;
pub const MAX_PASS_LEN: usize = 64;

#[derive(Clone)]
pub struct StoredCredentials {
    pub ssid: String,
    pub password: String,
    pub auth_method: AuthMethod,
}

impl std::fmt::Debug for StoredCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCredentials")
            .field("ssid", &self.ssid)
            .field("password", &"[redacted]")
            .finish()
    }
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

    // Buffers sized for the max value length plus NVS's required null terminator.
    let mut ssid_buf = [0u8; MAX_SSID_LEN + 1];
    let mut pass_buf = [0u8; MAX_PASS_LEN + 1];

    let ssid_opt = nvs
        .get_str(KEY_SSID, &mut ssid_buf)
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;

    let password = match nvs.get_str(KEY_PASSWORD, &mut pass_buf) {
        Ok(Some(p)) => p.to_string(),
        Ok(None) | Err(_) => String::new(),
    };

    let auth_method = match nvs.get_u8(KEY_AUTH_METHOD) {
        Ok(Some(v)) => auth_method_from_u8(v),
        Ok(None) | Err(_) => AuthMethod::WPA2Personal,
    };

    Ok(ssid_opt.map(|ssid| StoredCredentials {
        ssid: ssid.to_string(),
        password,
        auth_method,
    }))
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
        nvs.remove(KEY_PASSWORD)
            .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;
    } else {
        nvs.set_str(KEY_PASSWORD, &creds.password)
            .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;
    }

    nvs.set_u8(KEY_AUTH_METHOD, auth_method_to_u8(creds.auth_method))
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;

    Ok(())
}

pub fn clear_credentials(partition: EspNvsPartition<NvsDefault>) -> Result<(), ProvisioningError> {
    let mut nvs = open_nvs(partition, true)?;

    nvs.remove(KEY_SSID)
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;
    nvs.remove(KEY_PASSWORD)
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;
    nvs.remove(KEY_AUTH_METHOD)
        .map_err(|e| ProvisioningError::NvsAccess(e.into()))?;

    Ok(())
}

fn auth_method_to_u8(m: AuthMethod) -> u8 {
    match m {
        AuthMethod::None => 0,
        AuthMethod::WEP => 1,
        AuthMethod::WPA => 2,
        AuthMethod::WPA2Personal => 3,
        AuthMethod::WPAWPA2Personal => 4,
        AuthMethod::WPA2Enterprise => 5,
        AuthMethod::WPA3Personal => 6,
        AuthMethod::WPA2WPA3Personal => 7,
        AuthMethod::WAPIPersonal => 8,
    }
}

fn auth_method_from_u8(v: u8) -> AuthMethod {
    match v {
        0 => AuthMethod::None,
        1 => AuthMethod::WEP,
        2 => AuthMethod::WPA,
        3 => AuthMethod::WPA2Personal,
        4 => AuthMethod::WPAWPA2Personal,
        5 => AuthMethod::WPA2Enterprise,
        6 => AuthMethod::WPA3Personal,
        7 => AuthMethod::WPA2WPA3Personal,
        8 => AuthMethod::WAPIPersonal,
        _ => AuthMethod::WPA2Personal,
    }
}
