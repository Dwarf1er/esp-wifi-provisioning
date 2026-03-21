//! Error types for the provisioning library.
//!
//! All fallible public APIs return [`ProvisioningError`]. Internal helpers
//! that only fail in ways that map to a single variant use that variant
//! directly rather than going through this type.
use core::fmt;

use esp_idf_svc::sys::EspError;

/// Convenience alias for a heap-allocated, sendable, `'static` error used as
/// the associated error type in `esp-idf-svc` HTTP handler closures.
pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Describes why a WiFi connection attempt failed.
///
/// Carried inside [`ProvisioningError::ConnectionFailed`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ConnectionFailureCause {
    /// The connection was started but the device did not associate within the
    /// configured [`RetryConfig::connect_timeout`](crate::RetryConfig).
    Timeout,
    /// The underlying `esp-idf` WiFi driver returned an error.
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

/// All errors that can be returned by this crate.
///
/// The enum is `#[non_exhaustive]` so that new variants can be added in minor
/// releases without breaking downstream `match` arms.
#[derive(Debug)]
#[non_exhaustive]
pub enum ProvisioningError {
    /// Reading from or writing to NVS storage failed.
    NvsAccess(EspError),
    /// Credentials were found in NVS but are in an inconsistent state
    /// (e.g. a non-`None` auth method stored without a corresponding password).
    /// The portal will be opened so the user can re-enter them.
    NvsCorrupt,
    /// The WiFi driver returned an error during a start, stop, scan, or
    /// configuration operation.
    WifiDriver(EspError),
    /// All connection attempts were exhausted without successfully associating.
    ConnectionFailed {
        /// Number of attempts made before giving up.
        attempts: u8,
        /// The error from the final attempt.
        cause: ConnectionFailureCause,
    },
    /// The soft-AP or its network interface could not be started.
    ApStart(EspError),
    /// The captive-portal HTTP server encountered an error.
    HttpServer(BoxError),
    /// A credential value (SSID or password) could not be converted into the
    /// fixed-size string type that `esp-idf-svc` requires.
    InvalidCredentials,
    /// A configuration value was rejected before any I/O was attempted.
    /// The contained string describes which field is invalid and why.
    InvalidConfig(&'static str),
}

impl ProvisioningError {
    /// Returns `true` if this error represents exhausted connection attempts.
    ///
    /// Useful for callers that want to distinguish "we tried and failed to
    /// connect" from other, potentially more severe, error categories.
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
