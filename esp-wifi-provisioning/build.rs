use std::{env, fs, path::Path};

fn main() {
    embuild::espidf::sysenv::output();

    let out_dir = env::var("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir);

    let html = fs::read_to_string("./src/web/index.html").unwrap();
    let css = fs::read_to_string("./src/web/style.css").unwrap();
    let js = fs::read_to_string("./src/web/app.js").unwrap();

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
