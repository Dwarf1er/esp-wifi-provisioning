use esp_idf_svc::wifi::AuthMethod;

static INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.min.html"));

pub(crate) fn index_html() -> &'static str {
    INDEX_HTML
}

pub(crate) fn networks_json(networks: &[crate::wifi::ScannedNetwork]) -> String {
    use std::fmt::Write;
    let mut out = String::from("[");
    for (i, n) in networks.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }

        let secure = match n.auth_method {
            AuthMethod::None => false,
            _ => true,
        };

        write!(
            out,
            r#"{{"ssid":"{}","rssi":{},"secure":{}}}"#,
            json_escape_str(&n.ssid),
            n.rssi,
            secure
        )
        .unwrap();
    }
    out.push(']');
    out
}

pub(crate) fn json_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}
