use core::fmt;

#[derive(Debug)]
pub enum ProvisioningError {
    NvsAccess(anyhow::Error),
    NvsCorrupt,
    WifiDriver(anyhow::Error),
    ConnectionTimeout,
    ConnectionFailed { attempts: u8 },
    ApStart(anyhow::Error),
    HttpServer(anyhow::Error),
    InvalidCredentials,
}

impl fmt::Display for ProvisioningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NvsAccess(e) => write!(f, "NVS access error: {e}"),
            Self::NvsCorrupt => write!(f, "stored WiFi credentials are corrupt"),
            Self::WifiDriver(e) => write!(f, "WiFi driver error: {e}"),
            Self::ConnectionTimeout => write!(f, "WiFi connection timed out"),
            Self::ConnectionFailed { attempts } => {
                write!(f, "WiFi connection failed after {attempts} attempt(s)")
            }
            Self::ApStart(e) => write!(f, "failed to start soft-AP: {e}"),
            Self::HttpServer(e) => write!(f, "HTTP server error: {e}"),
            Self::InvalidCredentials => write!(f, "submitted credentials are invalid"),
        }
    }
}

impl std::error::Error for ProvisioningError {}
