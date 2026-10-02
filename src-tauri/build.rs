fn main() {
    // Hauptthread unter Windows (MSVC): 8 MB Stapel wie unter Linux statt 1 MB.
    // Tauri-Befehle ohne `async` laufen auf diesem Thread.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
    tauri_build::build()
}
