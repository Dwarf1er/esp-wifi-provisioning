//! Portal asset bundling and JSON serialisation helpers.
//!
//! The HTML, CSS, and JS for the setup portal are combined and minified at
//! build time by `build.rs` into a single `index.min.html` file embedded in
//! the binary via `include_str!`. This keeps flash usage low and eliminates
//! any runtime file I/O.
//!
//! JSON serialisation is hand-rolled rather than using `serde_json` for two
//! reasons:
//! 1. **Binary size**: `serde` and `serde_json` add meaningful overhead on an
//!    ESP32 where flash is limited.
//! 2. **Minimal dependencies**: this crate intentionally keeps its dependency
//!    tree small.

use esp_idf_svc::wifi::AuthMethod;

/// The minified portal HTML, inlined at compile time.
static INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.min.html"));

/// Returns the minified portal HTML as a static string slice.
pub(crate) fn index_html() -> &'static str {
    INDEX_HTML
}

/// Serialises a slice of scanned networks to a JSON array string.
///
/// Each network is represented as:
/// ```json
/// {"ssid":"<escaped ssid>","rssi":<dBm>,"secure":<bool>}
/// ```
///
/// A network is considered "secure" if its auth method is anything other than
/// [`AuthMethod::None`].
///
/// If the shape of `ScannedNetwork` changes, this function must be updated
/// to match.
pub(crate) fn networks_json(networks: &[crate::wifi::ScannedNetwork]) -> String {
    use std::fmt::Write;
    let mut out = String::from("[");
    for (i, n) in networks.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }

        let secure = !matches!(n.auth_method, AuthMethod::None);

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

/// Escapes a string for safe embedding inside a JSON string value.
///
/// Handles the mandatory JSON escapes (`"`, `\`, control characters) plus
/// HTML-sensitive characters (`<`, `>`, `&`) using Unicode escape sequences,
/// so the output is safe to embed directly in an HTML `<script>` block without
/// additional sanitisation.
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
