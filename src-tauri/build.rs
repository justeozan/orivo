fn main() {
    // `crash_reporting` reads the DSN with `option_env!`, which cargo only
    // re-evaluates when told the variable matters.
    println!("cargo:rerun-if-env-changed=VITE_SENTRY_DSN");
    tauri_build::build()
}
