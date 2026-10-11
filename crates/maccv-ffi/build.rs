//! Generates `include/maccv.h` from the `#[unsafe(no_mangle)]` surface below.
//!
//! The header is checked in so Xcode builds work without a cbindgen install;
//! regenerate it with `cargo build -p maccv-ffi` whenever the ABI changes.

use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let out = crate_dir.join("include/maccv.h");

    // Watch the whole source directory, not just lib.rs. A new module under
    // `src/` changes the exported surface, and a narrower watch left the
    // generated header stale without any warning — the header then disagreed
    // with the compiled library, and the mismatch only showed up at link time
    // in a C caller.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    let config = cbindgen::Config::from_root_or_default(&crate_dir);
    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&out);
        }
        Err(err) => {
            // Never fail the build for header generation: the header is committed.
            println!("cargo:warning=cbindgen skipped: {err}");
        }
    }
}
