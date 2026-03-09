static INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.min.html"));

pub fn index_html() -> &'static str {
    INDEX_HTML
}

pub fn networks_json(networks: &[crate::wifi::ScannedNetwork]) -> String {
    use std::fmt::Write;
    let mut out = String::from("[");
    for (i, n) in networks.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let escaped_ssid = n.ssid.replace('\\', "\\\\").replace('"', "\\\"");
        write!(out, r#"{{"ssid":"{}","rssi":{}}}"#, escaped_ssid, n.rssi).unwrap();
    }
    out.push(']');
    out
}
