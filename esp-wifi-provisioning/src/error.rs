use core::fmt;

use esp_idf_svc::sys::EspError;

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug)]
#[non_exhaustive]
pub enum ConnectionFailureCause {
    Timeout,
    DriverError(EspError),
}

impl fmt::Display for ConnectionFailureCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(f, "connection timed out"),
            Self::DriverError(e) => write!(f, "driver error: {e}"),
        }
    }
}

impl std::error::Error for ConnectionFailureCause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DriverError(e) => Some(e),
            Self::Timeout => None,
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum ProvisioningError {
    NvsAccess(EspError),
    NvsCorrupt,
    WifiDriver(EspError),
    ConnectionFailed {
        attempts: u8,
        cause: ConnectionFailureCause,
    },
    ApStart(EspError),
    HttpServer(BoxError),
    InvalidCredentials,
    InvalidConfig(&'static str),
}

impl ProvisioningError {
    pub fn is_connection_failure(&self) -> bool {
        matches!(self, Self::ConnectionFailed { .. })
    }
}

impl fmt::Display for ProvisioningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NvsAccess(e) => write!(f, "NVS access error: {e}"),
            Self::NvsCorrupt => write!(f, "stored WiFi credentials are corrupt"),
            Self::WifiDriver(e) => write!(f, "WiFi driver error: {e}"),
            Self::ApStart(e) => write!(f, "failed to start soft-AP: {e}"),
            Self::HttpServer(e) => write!(f, "HTTP server error: {e}"),
            Self::InvalidCredentials => write!(f, "submitted credentials are invalid"),
            Self::InvalidConfig(msg) => write!(f, "invalid configuration: {msg}"),
            Self::ConnectionFailed { attempts, cause } => write!(
                f,
                "WiFi connection failed after {attempts} attempt(s): {cause}"
            ),
        }
    }
}

impl std::error::Error for ProvisioningError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NvsAccess(e) => Some(e),
            Self::WifiDriver(e) => Some(e),
            Self::ApStart(e) => Some(e),
            Self::HttpServer(e) => Some(e.as_ref()),
            Self::ConnectionFailed { cause, .. } => Some(cause),
            _ => None,
        }
    }
}
