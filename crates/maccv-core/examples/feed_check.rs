//! Prints what the updater would choose for this machine, from a feed file.
//!
//! A diagnostic rather than a test: when an update goes wrong on one architecture,
//! this shows exactly which artifact was selected and why, without needing a
//! release. Point it at a feed, or fetch the published one.
//!
//!   cargo run -p maccv-core --example feed_check -- docs/preview.json
//!   curl -s https://yxxbc.github.io/Maccv/latest.json > /tmp/feed.json
//!   cargo run -p maccv-core --example feed_check -- /tmp/feed.json

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: feed_check <feed.json>");
        std::process::exit(2);
    });

    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) => {
            eprintln!("could not read {path}: {error}");
            std::process::exit(1);
        }
    };

    let manifest = match maccv_core::ReleaseManifest::from_json(&body) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{path} is not a usable feed: {error}");
            std::process::exit(1);
        }
    };

    println!("feed           {path}");
    println!("version        {}", manifest.version);
    println!(
        "channel        {}",
        if manifest.prerelease {
            "preview"
        } else {
            "stable"
        }
    );
    println!(
        "minimum macOS  {}",
        manifest
            .minimum_system_version
            .as_deref()
            .unwrap_or("unstated")
    );
    if !manifest.revoked.is_empty() {
        println!("revoked        {}", manifest.revoked.join(", "));
    }

    if manifest.downloads.is_empty() {
        println!("downloads      none listed; a single url is used for every architecture");
    } else {
        println!("downloads      {} listed", manifest.downloads.len());
    }

    // Every spelling a caller might use, so a mismatch is visible here rather than
    // on a user's machine.
    for arch in ["arm64", "aarch64", "x86_64", "amd64", "i386"] {
        let normalised = maccv_core::normalise_arch(arch);
        match manifest.download_for(arch) {
            Some(download) => {
                let name = download.url.rsplit('/').next().unwrap_or(&download.url);
                println!(
                    "  {arch:8} (as {normalised:6}) -> {name}  {} B",
                    download.size
                );
            }
            None => println!("  {arch:8} (as {normalised:6}) -> NO MATCH"),
        }
    }

    let host = maccv_core::host_arch();
    println!("host           {host}");
    match manifest.download_for_host() {
        Some(download) => println!(
            "host would get {}",
            download.url.rsplit('/').next().unwrap_or(&download.url)
        ),
        None => {
            println!("host would get NOTHING — this machine cannot update from this feed");
            std::process::exit(1);
        }
    }
}
