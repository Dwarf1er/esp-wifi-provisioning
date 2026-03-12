use std::{env, fs, path::Path};

fn main() {
    embuild::espidf::sysenv::output();

    let out_dir = env::var("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir);

    let html = fs::read_to_string("./src/web/index.html").unwrap();
    let css = fs::read_to_string("./src/web/style.css").unwrap();
    let js = fs::read_to_string("./src/web/app.js").unwrap();

    let js = strip_dev_block(&js, "const DEV_NETWORKS", "];");

    let combined = html
        .replace(
            r#"<link rel="stylesheet" href="style.css" />"#,
            &format!("<style>{css}</style>"),
        )
        .replace(
            r#"<script src="app.js"></script>"#,
            &format!("<script>{js}</script>"),
        );

    let minified = minify_html::minify(
        combined.as_bytes(),
        &minify_html::Cfg {
            minify_css: true,
            minify_js: true,
            ..Default::default()
        },
    );

    fs::write(out_dir.join("index.min.html"), &minified).unwrap();

    println!("cargo:rerun-if-changed=./src/web/index.html");
    println!("cargo:rerun-if-changed=./src/web/style.css");
    println!("cargo:rerun-if-changed=./src/web/app.js");
}

fn strip_dev_block(src: &str, start_marker: &str, end_marker: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut skipping = false;
    for line in src.lines() {
        if !skipping && line.trim_start().starts_with(start_marker) {
            skipping = true;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
        if skipping && line.trim_end().ends_with(end_marker) {
            skipping = false;
        }
    }
    out
}
