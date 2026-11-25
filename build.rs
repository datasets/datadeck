fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/web_drop.m");
        cc::Build::new()
            .file("src/web_drop.m")
            .flag("-fobjc-arc")
            .compile("datadeck_web_drop");
    }
}
