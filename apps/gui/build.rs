fn main() {
    if matches!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("windows" | "linux" | "macos")
    ) {
        tauri_build::build();
    }
}
