use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(cannelloni_embedded)");

    let src = Path::new("bin/cannelloni");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let dst = Path::new(&out_dir).join("cannelloni_embedded");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=bin/cannelloni");

    if src.exists() {
        fs::copy(src, &dst).expect("failed to copy bin/cannelloni into OUT_DIR for embedding");
        println!("cargo:rustc-cfg=cannelloni_embedded");
    } else {
        println!(
            "cargo:warning=bin/cannelloni not found — remote mode will need cannelloni beside the executable or in /usr/local/bin"
        );
    }
}
