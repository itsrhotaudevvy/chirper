use std::env;
use std::path::PathBuf;

fn main() {
    if env::var_os("CARGO_FEATURE_STATIC_LINK").is_none() {
        return;
    }
    println!("cargo:rerun-if-env-changed=CHIRPER_GO_LIB_DIR");
    let target = env::var("TARGET").unwrap_or_default();
    let dir = match env::var_os("CHIRPER_GO_LIB_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default())
            .join("../../go/out")
            .join(&target),
    };
    let archive = dir.join("libchirpercore.a");
    println!("cargo:rerun-if-changed={}", archive.display());
    if !archive.exists() {
        println!(
            "cargo:warning=missing {}; build it with scripts/build-go.sh {target}",
            archive.display()
        );
    }
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static=chirpercore");
    if target.contains("apple-darwin") || target.contains("apple-ios") {
        for framework in ["CoreFoundation", "Security"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        println!("cargo:rustc-link-lib=resolv");
    }
}
