fn main() {
    embuild::espidf::sysenv::output();
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/style.css");
    println!("cargo:rerun-if-changed=web/app.js");
}
