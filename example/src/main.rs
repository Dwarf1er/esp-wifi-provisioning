use esp_idf_hal::gpio::PinDriver;
use esp_idf_hal::prelude::Peripherals;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use esp_wifi_provisioning::Provisioner;

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    let wifi_driver = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs.clone())).unwrap(),
        sysloop,
    )
    .unwrap();

    let reset_pin = PinDriver::input(peripherals.pins.gpio0).unwrap();

    let provisioner = Provisioner::new(wifi_driver, nvs).ap_ssid("Example-Setup");

    if reset_pin.is_low() {
        log::info!("Factory reset pin held | clearing stored credentials");
        provisioner.clear_credentials().unwrap();
    }

    provisioner.provision().unwrap();

    log::info!("WiFi ready!");
}
