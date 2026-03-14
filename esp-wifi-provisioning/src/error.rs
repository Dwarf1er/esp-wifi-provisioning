use core::fmt;

pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug)]
pub enum ConnectionFailureCause {
    Timeout,
    DriverError(BoxError),
}

#[derive(Debug)]
pub enum ProvisioningError {
    NvsAccess(BoxError),
    NvsCorrupt,
    WifiDriver(BoxError),
    ConnectionFailed {
        attempts: u8,
        cause: ConnectionFailureCause,
    },
    ApStart(BoxError),
    HttpServer(BoxError),
    InvalidCredentials,
}

impl fmt::Display for ProvisioningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NvsAccess(e) => write!(f, "NVS access error: {e}"),
            Self::NvsCorrupt => write!(f, "stored WiFi credentials are corrupt"),
            Self::WifiDriver(e) => write!(f, "WiFi driver error: {e}"),
            Self::ConnectionFailed { attempts, cause } => match cause {
                ConnectionFailureCause::Timeout => {
                    write!(f, "WiFi connection timed out after {attempts} attempt(s)")
                }
                ConnectionFailureCause::DriverError(e) => {
                    write!(f, "WiFi connection failed after {attempts} attempt(s): {e}")
                }
            },
            Self::ApStart(e) => write!(f, "failed to start soft-AP: {e}"),
            Self::HttpServer(e) => write!(f, "HTTP server error: {e}"),
            Self::InvalidCredentials => write!(f, "submitted credentials are invalid"),
        }
    }
}

impl std::error::Error for ProvisioningError {}
