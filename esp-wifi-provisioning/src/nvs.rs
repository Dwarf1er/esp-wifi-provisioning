//! NVS credential persistence.
//!
//! Credentials are stored under the `"wifi_prov"` namespace using three keys:
//!
//! | NVS key       | Type | Content                              |
//! |---------------|------|--------------------------------------|
//! | `ssid`        | str  | Network name (max 32 bytes)          |
//! | `password`    | str  | WPA2 password; absent for open nets  |
//! | `auth_method` | u8   | [`AuthMethod`] discriminant (see below) |
//!
//! The `auth_method` byte uses a stable, manually assigned mapping so that
//! credentials written by one firmware version remain readable by another even
//! if the `esp-idf-svc` enum order changes.  See [`auth_method_to_u8`] for the
//! mapping.

use crate::error::ProvisioningError;
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use esp_idf_svc::wifi::AuthMethod;

const NVS_NAMESPACE: &str = "wifi_prov";
const KEY_SSID: &str = "ssid";
const KEY_PASSWORD: &str = "password";
const KEY_AUTH_METHOD: &str = "auth_method";

/// Maximum SSID length in bytes, matching the 802.11 specification.
pub(crate) const SSID_LEN_MAX: usize = 32;
/// Maximum WPA/WPA2 passphrase length in bytes.
pub(crate) const WPA_PASS_LEN_MAX: usize = 63;
/// Minimum WPA/WPA2 passphrase length in bytes.
pub(crate) const WPA_PASS_LEN_MIN: usize = 8;

/// WiFi credentials as stored in NVS and used for station-mode connection.
///
/// The `Debug` implementation deliberately redacts the password field.
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

/// Opens the `wifi_prov` NVS namespace.
///
/// Returns `Ok(None)` if the namespace does not exist yet (first boot), which
/// the caller treats identically to "no credentials stored".
///
/// # Errors
///
/// Returns [`ProvisioningError::NvsAccess`] for any NVS error other than
/// `ESP_ERR_NVS_NOT_FOUND`.
fn open_nvs(
    partition: EspNvsPartition<NvsDefault>,
    read_write: bool,
) -> Result<Option<EspNvs<NvsDefault>>, ProvisioningError> {
    match EspNvs::new(partition, NVS_NAMESPACE, read_write) {
        Ok(nvs) => Ok(Some(nvs)),
        Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_NVS_NOT_FOUND => Ok(None),
        Err(e) => Err(ProvisioningError::NvsAccess(e)),
    }
}

/// Reads credentials from NVS.
///
/// Returns `Ok(None)` when no credentials have been stored yet (the namespace
/// or the SSID key is absent). Returns
/// [`ProvisioningError::NvsCorrupt`] when the stored data is internally
/// inconsistent (e.g. auth method says WPA2 but no password key exists).
///
/// # Errors
///
/// Returns [`ProvisioningError::NvsAccess`] on low-level NVS read failures.
pub(crate) fn load_credentials(
    partition: EspNvsPartition<NvsDefault>,
) -> Result<Option<StoredCredentials>, ProvisioningError> {
    let nvs = match open_nvs(partition, false)? {
        Some(nvs) => nvs,
        None => return Ok(None),
    };

    let mut ssid_buf = [0u8; SSID_LEN_MAX + 1];
    let mut pass_buf = [0u8; WPA_PASS_LEN_MAX + 1];

    let ssid = match nvs.get_str(KEY_SSID, &mut ssid_buf) {
        Ok(Some(s)) => s.to_string(),
        Ok(None) => return Ok(None),
        Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_NVS_NOT_FOUND => return Ok(None),
        Err(e) => return Err(ProvisioningError::NvsAccess(e)),
    };

    let auth_method = match nvs.get_u8(KEY_AUTH_METHOD) {
        Ok(Some(v)) => auth_method_from_u8(v)?,
        Ok(None) => AuthMethod::WPA2Personal,
        Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_NVS_NOT_FOUND => AuthMethod::WPA2Personal,
        Err(e) => return Err(ProvisioningError::NvsAccess(e)),
    };

    let password = match nvs.get_str(KEY_PASSWORD, &mut pass_buf) {
        Ok(Some(p)) => p.to_string(),
        Ok(None) | Err(_) => {
            if !matches!(auth_method, AuthMethod::None) {
                log::warn!(
                    "Stored credentials for '{}' have auth_method {:?} but no password \
                     | treating as corrupt",
                    ssid,
                    auth_method
                );
                return Err(ProvisioningError::NvsCorrupt);
            }
            String::new()
        }
    };

    Ok(Some(StoredCredentials {
        ssid,
        password,
        auth_method,
    }))
}

/// Persists credentials to NVS, overwriting any previously stored values.
///
/// If the password is empty (open network), the `password` key is *removed*
/// from NVS rather than written as an empty string, so that
/// [`load_credentials`] can reliably detect the open-network case.
///
/// # Errors
///
/// Returns [`ProvisioningError::InvalidCredentials`] if the SSID or password
/// lengths are out of range. Returns [`ProvisioningError::NvsAccess`] on
/// write failures.
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

    let mut nvs = open_nvs(partition, true)?.ok_or_else(|| ProvisioningError::NvsCorrupt)?;

    nvs.set_str(KEY_SSID, &creds.ssid)
        .map_err(ProvisioningError::NvsAccess)?;

    if creds.password.is_empty() {
        match nvs.remove(KEY_PASSWORD) {
            Ok(_) => {}
            Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_NVS_NOT_FOUND => {}
            Err(e) => return Err(ProvisioningError::NvsAccess(e)),
        }
    } else {
        nvs.set_str(KEY_PASSWORD, &creds.password)
            .map_err(ProvisioningError::NvsAccess)?;
    }

    nvs.set_u8(KEY_AUTH_METHOD, auth_method_to_u8(creds.auth_method))
        .map_err(ProvisioningError::NvsAccess)?;

    Ok(())
}

/// Removes all credential keys from NVS.
///
/// A no-op if no credentials have been stored yet. Ignores
/// `ESP_ERR_NVS_NOT_FOUND` on individual keys so that a partial write from a
/// previous interrupted save is also cleaned up correctly.
///
/// # Errors
///
/// Returns [`ProvisioningError::NvsAccess`] on deletion failures.
pub(crate) fn clear_credentials(
    partition: EspNvsPartition<NvsDefault>,
) -> Result<(), ProvisioningError> {
    let mut nvs = match open_nvs(partition, true)? {
        Some(nvs) => nvs,
        None => return Ok(()),
    };

    for key in [KEY_SSID, KEY_PASSWORD, KEY_AUTH_METHOD] {
        match nvs.remove(key) {
            Ok(_) => {}
            Err(e) if e.code() == esp_idf_svc::sys::ESP_ERR_NVS_NOT_FOUND => {}
            Err(e) => return Err(ProvisioningError::NvsAccess(e)),
        }
    }
    Ok(())
}

/// Converts an [`AuthMethod`] to its stable NVS byte representation.
///
/// The mapping is fixed and must not change between firmware versions:
///
/// | Byte | Variant |
/// |------|---------|
/// | 0 | `None` |
/// | 1 | `WEP` |
/// | 2 | `WPA` |
/// | 3 | `WPA2Personal` |
/// | 4 | `WPAWPA2Personal` |
/// | 5 | `WPA2Enterprise` |
/// | 6 | `WPA3Personal` |
/// | 7 | `WPA2WPA3Personal` |
/// | 8 | `WAPIPersonal` |
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

/// Converts a NVS byte back to an [`AuthMethod`].
///
/// An unrecognised byte (written by a newer firmware) logs a warning and falls
/// back to `WPA2Personal` rather than failing, so that a firmware downgrade
/// does not brick a provisioned device.
///
/// # Errors
///
/// Currently infallible (the fallback handles unknown bytes), but returns
/// `Result` for forward-compatibility should stricter handling be needed later.
fn auth_method_from_u8(v: u8) -> Result<AuthMethod, ProvisioningError> {
    match v {
        0 => Ok(AuthMethod::None),
        1 => Ok(AuthMethod::WEP),
        2 => Ok(AuthMethod::WPA),
        3 => Ok(AuthMethod::WPA2Personal),
        4 => Ok(AuthMethod::WPAWPA2Personal),
        5 => Ok(AuthMethod::WPA2Enterprise),
        6 => Ok(AuthMethod::WPA3Personal),
        7 => Ok(AuthMethod::WPA2WPA3Personal),
        8 => Ok(AuthMethod::WAPIPersonal),
        _ => {
            log::warn!(
                "Unrecognised auth_method byte {v} in NVS (written by newer firmware?), \
                 falling back to WPA2Personal"
            );
            Ok(AuthMethod::WPA2Personal)
        }
    }
}
