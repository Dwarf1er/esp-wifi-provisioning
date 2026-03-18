use crate::error::ProvisioningError;
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use esp_idf_svc::wifi::AuthMethod;

const NVS_NAMESPACE: &str = "wifi_prov";
const KEY_SSID: &str = "ssid";
const KEY_PASSWORD: &str = "password";
const KEY_AUTH_METHOD: &str = "auth_method";

pub(crate) const SSID_LEN_MAX: usize = 32;
pub(crate) const WPA_PASS_LEN_MAX: usize = 63;
pub(crate) const WPA_PASS_LEN_MIN: usize = 8;

#[derive(Clone)]
pub(crate) struct StoredCredentials {
    pub(crate) ssid: String,
    pub(crate) password: String,
    pub(crate) auth_method: AuthMethod,
}

impl std::fmt::Debug for StoredCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCredentials")
            .field("ssid", &self.ssid)
            .field("password", &"[redacted]")
            .finish()
    }
}

/// Opens the NVS namespace, returning `Ok(None)` if it doesn't exist yet
/// (normal on first boot). Any other error is returned as `NvsAccess`.
///
/// `save_credentials` and `clear_credentials` open with `read_write: true`,
/// which creates the namespace if absent — `NOT_FOUND` should never fire
/// for them. Only `load_credentials` uses `read_write: false` and needs the
/// `None` path.
fn open_nvs(
    partition: EspNvsPartition<NvsDefault>,
    read_write: bool,
) -> Result<Option<EspNvs<NvsDefault>>, ProvisioningError> {
    match EspNvs::new(partition, NVS_NAMESPACE, read_write) {
        Ok(nvs) => Ok(Some(nvs)),
        Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_NVS_NOT_FOUND => Ok(None),
        Err(e) => Err(ProvisioningError::NvsAccess(e.into())),
    }
}

pub(crate) fn load_credentials(
    partition: EspNvsPartition<NvsDefault>,
) -> Result<Option<StoredCredentials>, ProvisioningError> {
    let nvs = match open_nvs(partition, false)? {
        Some(nvs) => nvs,
        None => return Ok(None),
    };

    // Buffers sized for the max value length plus NVS's required null terminator.
    let mut ssid_buf = [0u8; SSID_LEN_MAX + 1];
    let mut pass_buf = [0u8; WPA_PASS_LEN_MAX + 1];

    let ssid_opt = match nvs.get_str(KEY_SSID, &mut ssid_buf) {
        Ok(v) => v,
        Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_NVS_NOT_FOUND => return Ok(None),
        Err(e) => return Err(ProvisioningError::NvsAccess(e.into())),
    };

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

pub(crate) fn save_credentials(
    partition: EspNvsPartition<NvsDefault>,
    creds: &StoredCredentials,
) -> Result<(), ProvisioningError> {
    if creds.ssid.is_empty() || creds.ssid.len() > SSID_LEN_MAX {
        return Err(ProvisioningError::InvalidCredentials);
    }
    if creds.password.len() > WPA_PASS_LEN_MAX {
        return Err(ProvisioningError::InvalidCredentials);
    }

    let mut nvs = open_nvs(partition, true)?.ok_or_else(|| {
        ProvisioningError::NvsAccess("namespace not found after write open".into())
    })?;

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

pub(crate) fn clear_credentials(
    partition: EspNvsPartition<NvsDefault>,
) -> Result<(), ProvisioningError> {
    let mut nvs = open_nvs(partition, true)?.ok_or_else(|| {
        ProvisioningError::NvsAccess("namespace not found after write open".into())
    })?;

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
        #[allow(unreachable_patterns)]
        _ => {
            log::warn!("Unknown AuthMethod variant, storing as WPA2Personal");
            3
        }
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
        _ => {
            log::warn!("Unrecognised auth_method byte {v} in NVS, defaulting to WPA2Personal");
            AuthMethod::WPA2Personal
        }
    }
}
