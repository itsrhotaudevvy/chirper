// The plugin exposes no JavaScript commands: the Rust side calls the Kotlin and Swift code through
// `run_mobile_plugin`, so there is nothing to put into the permission manifests.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
