# esp-wifi-provisioning

WiFi provisioning via a captive-portal soft-AP for ESP32 targets, built on [`esp-idf-svc`](https://github.com/esp-rs/esp-idf-svc). When a device has no stored credentials, it broadcasts a setup access point and serves a small web UI. Users connect, pick their network, enter a password, and the device saves the credentials to NVS and connects. No hardcoded SSIDs, no serial flashing required.

## How it works

1. On boot, stored NVS credentials are tried first. If they work, the portal never appears.
2. If there are no credentials (or they fail), a soft-AP is started and a captive portal is served.
3. The user connects to the AP, is redirected to the setup page automatically, and submits their WiFi credentials.
4. The device connects in station mode, persists the credentials to NVS, and returns the connected WiFi driver.
5. On the next boot, step 1 succeeds and provisioning completes silently.

## Usage

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
esp-wifi-provisioning = "0.1"
```

Then call [`Provisioner::provision`] in your application entry point:

```rust,no_run
use esp_wifi_provisioning::Provisioner;

// wifi: BlockingWifi<EspWifi<'static>>
// nvs:  EspNvsPartition<NvsDefault>

let wifi = Provisioner::new(wifi, nvs)
    .ap_ssid("MyDevice-Setup")
    .provision()
    .expect("provisioning failed");

// `wifi` is now connected, use it as normal.
```

### Customisation

```rust,no_run
use std::time::Duration;
use esp_wifi_provisioning::{Provisioner, ApConfig, ApSecurity, RetryConfig};

let wifi = Provisioner::new(wifi, nvs)
    // Soft-AP settings
    .ap_ssid("Sensor-Setup")
    .ap_password("secret123") // omit for an open AP (default)

    // Or replace the entire AP config for channel/IP control:
    // .ap_config(ApConfig { channel: 11, ..ApConfig::default() })

    // Connection retry settings
    .retry_config(
        RetryConfig::default()
            .max_attempts(3)
            .connect_timeout(Duration::from_secs(15)),
    )
    .provision()?;
```

### Factory reset / re-provisioning

```rust,no_run
let provisioner = Provisioner::new(wifi, nvs);
provisioner.clear_credentials()?; // wipe NVS, portal will appear on next boot
```

## Requirements

- Target: ESP32 (tested via `esp-idf-svc`)
- Rust: 1.85 or later (edition 2024)
- `esp-idf-svc` and `esp-idf-hal` must be present in your workspace or dependency tree

## License

This software is licensed under the [MIT license](https://github.com/Dwarf1er/esp-wifi-provisioning/blob/master/LICENSE)
