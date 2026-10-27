fn main() {
    // Create a cfg alias for wasm
    println!("cargo::rustc-check-cfg=cfg(web)");
    if std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|s| s == "wasm32") {
        println!("cargo::rustc-cfg=web");
    }
}
