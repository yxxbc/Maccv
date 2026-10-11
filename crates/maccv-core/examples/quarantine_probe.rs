//! Manual probe: confirms quarantine detection and removal against a real file.
//!
//! Run with a path that carries `com.apple.quarantine`:
//!   cargo run -p maccv-core --example quarantine_probe -- /tmp/some/File.app

fn main() {
    let Some(argument) = std::env::args().nth(1) else {
        eprintln!("usage: quarantine_probe <path>");
        std::process::exit(2);
    };
    let path = std::path::Path::new(&argument);

    println!("path                        = {}", path.display());
    println!(
        "has_quarantine (before)     = {}",
        maccv_core::has_quarantine(path)
    );

    if let Some(parent) = path.parent() {
        let flagged = maccv_core::quarantine_paths(parent);
        println!("quarantine_paths (siblings) = {}", flagged.len());
    }

    match maccv_core::remove_quarantine(path) {
        Ok(()) => println!("remove_quarantine           = ok"),
        Err(error) => println!("remove_quarantine           = ERR {error}"),
    }
    println!(
        "has_quarantine (after)      = {}",
        maccv_core::has_quarantine(path)
    );
}
