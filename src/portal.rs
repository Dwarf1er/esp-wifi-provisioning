static INDEX_HTML: &str = include_str!("./web/index.html");
static STYLE_CSS: &str = include_str!("./web/style.css");
static APP_JS: &str = include_str!("./web/app.js");

pub fn index_html() -> &'static str {
    INDEX_HTML
}
pub fn style_css() -> &'static str {
    STYLE_CSS
}
pub fn app_js() -> &'static str {
    APP_JS
}

pub fn networks_json(networks: &[crate::wifi::ScannedNetwork]) -> String {
    use std::fmt::Write;
    let mut out = String::from("[");
    for (i, n) in networks.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(
            out,
            r#"{{"ssid":{},"rssi":{}}}"#,
            serde_json::to_string(&n.ssid).unwrap_or_default(),
            n.rssi
        )
        .unwrap();
    }
    out.push(']');
    out
}
