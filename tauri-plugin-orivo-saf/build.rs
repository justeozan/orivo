/// This plugin exposes no command to the WebView: Orivo's Rust host is its only
/// caller. The builder still runs, because `android_path` is what publishes the
/// Gradle library project to the app's `tauri-build`.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
